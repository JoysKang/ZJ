//! The client: one supervisor thread per agent, which starts the process on demand, runs the
//! ACP connection, stops the process when idle and restarts it on the next prompt.

use crate::{
    events::{self, AgentEvent, AgentInfo, ExitReason, Modes, PermissionId, TurnId, TurnOutcome},
    fs::{BufferProvider, Workspace, read_disk, window, write_atomic},
    process::{AgentProcess, describe},
    registry::{AgentPreset, SearchPath},
    shadow::ShadowStore,
};
use agent_client_protocol::{
    self as sdk, Agent, ByteStreams, Client, ConnectionTo, Responder,
    schema::{ProtocolVersion, v1 as acp},
};
use async_io::{Async, Timer};
use futures::{AsyncRead, FutureExt, channel::oneshot, future::Either};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll},
    thread,
    time::Duration,
};

/// How `fs/write_text_file` reaches the disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteMode {
    /// Write immediately (default); the editor reloads the file.
    #[default]
    Direct,
    /// Keep the agent's version in the [`ShadowStore`] until the user accepts it.
    /// Shell commands the agent runs itself still see and write the real disk.
    AcceptFirst,
}

pub struct ClientOptions {
    pub preset: AgentPreset,
    /// The agent's `cwd`; every `fs/*` path must stay inside it.
    pub workspace_root: PathBuf,
    pub write_mode: WriteMode,
    /// Stop the process after this long without a turn, prompt or permission request.
    pub idle_timeout: Duration,
    /// Initialize / session setup; the first `npx` run downloads the adapter.
    pub handshake_timeout: Duration,
    /// Values from the settings file, e.g. `ANTHROPIC_AUTH_TOKEN` for the DeepSeek preset.
    pub env_overrides: BTreeMap<String, String>,
    pub buffers: Option<Arc<dyn BufferProvider>>,
    /// Defaults to [`SearchPath::from_env`].
    pub search_path: Option<SearchPath>,
    /// An ACP session id from an earlier run (history); restored with `session/load` when the
    /// agent supports it, otherwise a new session starts.
    pub resume_session: Option<String>,
}

impl ClientOptions {
    pub fn new(preset: AgentPreset, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            preset,
            workspace_root: workspace_root.into(),
            write_mode: WriteMode::Direct,
            idle_timeout: Duration::from_secs(10 * 60),
            handshake_timeout: Duration::from_secs(180),
            env_overrides: BTreeMap::new(),
            buffers: None,
            search_path: None,
            resume_session: None,
        }
    }
}

/// Context attached to a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptPart {
    Text(String),
    /// A file reference (`resource_link`); the agent reads it itself.
    File(PathBuf),
    /// Lines `start_line..=end_line` (1-based). With `text` and an agent that accepts embedded
    /// context, the selection is sent inline; otherwise as a link with a line fragment.
    Selection {
        path: PathBuf,
        start_line: u32,
        end_line: u32,
        text: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// A turn is already running; queue it in the UI or cancel first.
    Busy,
    Stopped,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ClientError::Busy => "Agent 正在回复，请等这一轮结束或先中断",
            ClientError::Stopped => "Agent 客户端已关闭",
        })
    }
}

impl std::error::Error for ClientError {}

const EVENT_CAPACITY: usize = 512;
const COMMAND_CAPACITY: usize = 64;
/// One JSON-RPC line from the agent (a tool call can carry two copies of a large file).
const MAX_LINE: usize = 32 * 1024 * 1024;
/// Files remembered for Direct-mode reviews per client.
const MAX_SNAPSHOTS: usize = 256;
/// How long a read waits for the editor's unsaved buffer.
const BUFFER_TIMEOUT: Duration = Duration::from_secs(5);

enum Command {
    Connect,
    Prompt {
        turn: TurnId,
        parts: Vec<PromptPart>,
    },
    Cancel,
    SetMode(String),
    Shutdown,
}

struct Shared {
    preset: AgentPreset,
    workspace: Workspace,
    idle_timeout: Duration,
    handshake_timeout: Duration,
    resume_session: Option<String>,
    env_overrides: BTreeMap<String, String>,
    buffers: Option<Arc<dyn BufferProvider>>,
    /// Closed by [`AgentClient::stop`]: a read waiting for the editor gives up instead of
    /// holding the supervisor (and the UI thread joining it) until the timeout.
    stopping: async_channel::Receiver<()>,
    search: SearchPath,
    write_mode: Mutex<WriteMode>,
    events: async_channel::Sender<AgentEvent>,
    /// Wakes the session loop when a turn finishes (re-arms the idle timer).
    turn_done: async_channel::Sender<()>,
    shadow: ShadowStore,
    permissions: Mutex<HashMap<PermissionId, oneshot::Sender<Option<String>>>>,
    next_permission: AtomicU64,
    turn: Mutex<Option<TurnId>>,
    next_turn: AtomicU64,
    /// `session/load` replays history as updates; the UI already has it.
    replaying: AtomicBool,
    embedded_context: AtomicBool,
    pid: Mutex<Option<u32>>,
    /// Direct mode: each file's content before the agent first touched it in this client
    /// (`None` = the file did not exist), for "working tree vs. before the agent" reviews.
    snapshots: Mutex<BTreeMap<PathBuf, Option<String>>>,
}

impl Shared {
    /// The editor's unsaved text for `path`, if any.
    async fn buffer_text(&self, path: &Path) -> io::Result<Option<String>> {
        let Some(buffers) = &self.buffers else {
            return Ok(None);
        };
        let read = buffers.buffer_text(path);
        let stopping = std::pin::pin!(self.stopping.recv());
        match with_timeout(futures::future::select(read, stopping), BUFFER_TIMEOUT).await {
            Some(Either::Left((text, _))) => Ok(text),
            Some(Either::Right(_)) => Err(io::Error::other("Agent 客户端已关闭")),
            None => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "编辑器没有及时给出未保存的内容",
            )),
        }
    }

    /// Records `path` as it is now, once: the unsaved buffer when the editor has one (that is
    /// what the agent read and edits), otherwise the disk. Unreadable or oversized files are
    /// skipped.
    async fn snapshot(&self, path: &Path) {
        let known = |s: &Self| {
            let snapshots = s.snapshots.lock().unwrap();
            snapshots.contains_key(path) || snapshots.len() >= MAX_SNAPSHOTS
        };
        if known(self) {
            return;
        }
        let before = match self.buffer_text(path).await {
            Ok(Some(text)) => Some(text),
            Ok(None) | Err(_) => match read_disk(path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(_) => return,
            },
        };
        if !known(self) {
            self.snapshots
                .lock()
                .unwrap()
                .insert(path.to_path_buf(), before);
        }
    }
}

impl Shared {
    async fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event).await;
    }

    /// Ends the current turn exactly once (from the prompt task or the supervisor).
    async fn finish_turn(&self, only: Option<TurnId>, outcome: TurnOutcome) {
        let turn = {
            let mut current = self.turn.lock().unwrap();
            match *current {
                Some(t) if only.is_none_or(|o| o == t) => current.take(),
                _ => None,
            }
        };
        if let Some(turn) = turn {
            eprintln!(
                "event=agent_turn_end agent={} turn={turn} outcome={}",
                self.preset.id,
                outcome_tag(&outcome)
            );
            self.emit(AgentEvent::TurnEnded { turn, outcome }).await;
        }
    }

    fn cancel_permissions(&self) {
        // Dropping the senders answers every pending request with `cancelled`.
        self.permissions.lock().unwrap().clear();
    }
}

fn outcome_tag(outcome: &TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::EndTurn => "end_turn",
        TurnOutcome::MaxTokens => "max_tokens",
        TurnOutcome::MaxTurnRequests => "max_turn_requests",
        TurnOutcome::Refusal => "refusal",
        TurnOutcome::Cancelled => "cancelled",
        TurnOutcome::Failed(_) => "failed",
    }
}

pub struct AgentClient {
    shared: Arc<Shared>,
    commands: async_channel::Sender<Command>,
    events: async_channel::Receiver<AgentEvent>,
    stopping: async_channel::Sender<()>,
    supervisor: Option<thread::JoinHandle<()>>,
}

impl AgentClient {
    /// Starts the supervisor thread. The agent process itself starts on the first
    /// [`prompt`](Self::prompt) or [`connect`](Self::connect).
    pub fn start(options: ClientOptions) -> io::Result<Self> {
        let workspace = Workspace::new(&options.workspace_root)?;
        let (events_tx, events_rx) = async_channel::bounded(EVENT_CAPACITY);
        let (commands_tx, commands_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let (done_tx, done_rx) = async_channel::unbounded();
        let (stopping_tx, stopping_rx) = async_channel::bounded(1);
        let shared = Arc::new(Shared {
            preset: options.preset,
            workspace,
            idle_timeout: options.idle_timeout,
            handshake_timeout: options.handshake_timeout,
            resume_session: options.resume_session,
            env_overrides: options.env_overrides,
            buffers: options.buffers,
            stopping: stopping_rx,
            search: options.search_path.unwrap_or_else(SearchPath::from_env),
            write_mode: Mutex::new(options.write_mode),
            events: events_tx,
            turn_done: done_tx,
            shadow: ShadowStore::default(),
            permissions: Mutex::new(HashMap::new()),
            next_permission: AtomicU64::new(1),
            turn: Mutex::new(None),
            next_turn: AtomicU64::new(1),
            replaying: AtomicBool::new(false),
            embedded_context: AtomicBool::new(false),
            pid: Mutex::new(None),
            snapshots: Mutex::new(BTreeMap::new()),
        });
        let supervisor_shared = shared.clone();
        let supervisor = thread::Builder::new()
            .name(format!("agent-{}", shared.preset.id))
            .spawn(move || {
                async_io::block_on(supervise(supervisor_shared, commands_rx, done_rx));
            })?;
        Ok(Self {
            shared,
            commands: commands_tx,
            events: events_rx,
            stopping: stopping_tx,
            supervisor: Some(supervisor),
        })
    }

    /// Events in protocol order. Bounded: a UI that stops reading eventually pauses the agent.
    pub fn events(&self) -> async_channel::Receiver<AgentEvent> {
        self.events.clone()
    }

    /// Starts the process and session now (e.g. when the panel opens) instead of on the first
    /// prompt.
    pub fn connect(&self) {
        let _ = self.commands.try_send(Command::Connect);
    }

    pub fn prompt(&self, parts: Vec<PromptPart>) -> Result<TurnId, ClientError> {
        let turn = {
            let mut current = self.shared.turn.lock().unwrap();
            if current.is_some() {
                return Err(ClientError::Busy);
            }
            let turn = self.shared.next_turn.fetch_add(1, Ordering::Relaxed);
            *current = Some(turn);
            turn
        };
        if let Err(e) = self.commands.try_send(Command::Prompt { turn, parts }) {
            *self.shared.turn.lock().unwrap() = None;
            return Err(if e.is_full() {
                ClientError::Busy
            } else {
                ClientError::Stopped
            });
        }
        Ok(turn)
    }

    /// `session/cancel`; pending permission requests are answered `cancelled`. The turn ends
    /// with [`TurnOutcome::Cancelled`] once the agent stops.
    pub fn cancel(&self) {
        self.shared.cancel_permissions();
        let _ = self.commands.try_send(Command::Cancel);
    }

    /// Answers a permission request with one of its option ids (`None` = dismissed, sent as
    /// `cancelled`). Returns `false` if the request is no longer pending.
    pub fn respond_permission(&self, id: PermissionId, option_id: Option<String>) -> bool {
        match self.shared.permissions.lock().unwrap().remove(&id) {
            Some(tx) => tx.send(option_id).is_ok(),
            None => false,
        }
    }

    /// Switches the session mode; modes the preset forbids (bypass / full access) are refused
    /// and `false` is returned.
    pub fn set_mode(&self, mode_id: impl Into<String>) -> bool {
        let mode_id = mode_id.into();
        if !self.shared.preset.modes.allows(&mode_id) {
            eprintln!("event=agent_mode_refused agent={}", self.shared.preset.id);
            return false;
        }
        self.commands.try_send(Command::SetMode(mode_id)).is_ok()
    }

    /// Paths the agent changed (or announced an edit for) in Direct mode, with their content
    /// before that first change.
    pub fn snapshot_paths(&self) -> Vec<PathBuf> {
        self.shared
            .snapshots
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// `Some(None)`: the file did not exist before the agent created it.
    pub fn snapshot(&self, path: &Path) -> Option<Option<String>> {
        self.shared.snapshots.lock().unwrap().get(path).cloned()
    }

    /// Replaces a file's "before" version after a partial review (`None` forgets the file:
    /// everything the agent changed in it was accepted or reverted).
    pub fn set_snapshot(&self, path: &Path, before: Option<Option<String>>) {
        let mut snapshots = self.shared.snapshots.lock().unwrap();
        match before {
            Some(before) => {
                snapshots.insert(path.to_path_buf(), before);
            }
            None => {
                snapshots.remove(path);
            }
        }
    }

    /// Forgets the "before" versions (after the user reviewed them, or for a new session).
    pub fn clear_snapshots(&self) {
        self.shared.snapshots.lock().unwrap().clear();
    }

    pub fn write_mode(&self) -> WriteMode {
        *self.shared.write_mode.lock().unwrap()
    }

    /// Takes effect for the next write; proposals already pending stay in the shadow store.
    pub fn set_write_mode(&self, mode: WriteMode) {
        *self.shared.write_mode.lock().unwrap() = mode;
    }

    pub fn shadow(&self) -> &ShadowStore {
        &self.shared.shadow
    }

    /// Process id while the agent runs (for the status bar's RSS sample).
    pub fn pid(&self) -> Option<u32> {
        *self.shared.pid.lock().unwrap()
    }

    pub fn is_busy(&self) -> bool {
        self.shared.turn.lock().unwrap().is_some()
    }

    /// Stops the agent (process group included) and waits for the supervisor.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.cancel_permissions();
        // A full queue must not keep the supervisor alive: closing it ends `recv`.
        let _ = self.commands.try_send(Command::Shutdown);
        self.commands.close();
        // Nobody may read events any more; unblock a supervisor waiting to send one.
        self.events.close();
        self.stopping.close();
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        self.stop();
    }
}

enum End {
    Idle,
    Shutdown,
    /// The agent closed stdout (exited or crashed).
    Closed,
    /// Handshake or session setup failed; already reported.
    Failed,
}

async fn supervise(
    shared: Arc<Shared>,
    commands: async_channel::Receiver<Command>,
    turn_done: async_channel::Receiver<()>,
) {
    // Kept across restarts so a crashed or idle-stopped agent can `session/load` it.
    let mut resume: Option<acp::SessionId> = shared.resume_session.clone().map(acp::SessionId::new);
    while let Ok(command) = commands.recv().await {
        let first = match command {
            Command::Shutdown => return,
            Command::Connect => None,
            Command::Prompt { turn, parts } => Some((turn, parts)),
            // No process: nothing to cancel or switch.
            Command::Cancel | Command::SetMode(_) => continue,
        };
        while turn_done.try_recv().is_ok() {}
        if let End::Shutdown = run_process(&shared, &commands, &turn_done, first, &mut resume).await
        {
            return;
        }
    }
}

async fn run_process(
    shared: &Arc<Shared>,
    commands: &async_channel::Receiver<Command>,
    turn_done: &async_channel::Receiver<()>,
    first: Option<(TurnId, Vec<PromptPart>)>,
    resume: &mut Option<acp::SessionId>,
) -> End {
    let name = shared.preset.display_name.clone();
    let launch = match shared.preset.resolve(&shared.search, &shared.env_overrides) {
        Ok(launch) => launch,
        Err(e) => {
            eprintln!("event=agent_launch_unavailable agent={}", shared.preset.id);
            let message = e.to_string();
            shared
                .emit(AgentEvent::Error {
                    message: message.clone(),
                })
                .await;
            shared.finish_turn(None, TurnOutcome::Failed(message)).await;
            return End::Failed;
        }
    };
    let mut process = match AgentProcess::spawn(&launch, shared.workspace.root()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("event=agent_spawn_failed agent={}", shared.preset.id);
            let message = format!("启动「{name}」失败：{e}");
            shared
                .emit(AgentEvent::Error {
                    message: message.clone(),
                })
                .await;
            shared.finish_turn(None, TurnOutcome::Failed(message)).await;
            return End::Failed;
        }
    };
    let pid = process.pid();
    *shared.pid.lock().unwrap() = Some(pid);
    eprintln!("event=agent_spawn agent={} pid={pid}", shared.preset.id);
    shared.emit(AgentEvent::Starting { pid }).await;

    let end = match transport(&mut process) {
        Ok(transport) => {
            match connect(
                shared.clone(),
                transport,
                commands,
                turn_done,
                first,
                resume,
            )
            .await
            {
                Ok(end) => end,
                Err(e) => {
                    eprintln!("event=agent_connection_error agent={}", shared.preset.id);
                    // A plain EOF is reported by the exit event (with the stderr tail).
                    if !sdk::is_incoming_transport_closed(&e) {
                        shared
                            .emit(AgentEvent::Error {
                                message: format!("与「{name}」的连接出错：{}", error_text(&e)),
                            })
                            .await;
                    }
                    End::Closed
                }
            }
        }
        Err(e) => {
            shared
                .emit(AgentEvent::Error {
                    message: format!("无法连接「{name}」的标准输入输出：{e}"),
                })
                .await;
            End::Failed
        }
    };

    shared.cancel_permissions();
    *shared.pid.lock().unwrap() = None;
    let status = process.terminate();
    let (code, signal) = describe(status);
    let reason = match end {
        End::Idle => ExitReason::Idle,
        End::Shutdown => ExitReason::Shutdown,
        End::Failed => ExitReason::SetupFailed {
            stderr_tail: process.stderr_tail(),
        },
        End::Closed => ExitReason::Crashed {
            code,
            signal,
            stderr_tail: process.stderr_tail(),
        },
    };
    eprintln!(
        "event=agent_exit agent={} pid={pid} reason={} code={code:?} signal={signal:?}",
        shared.preset.id,
        match reason {
            ExitReason::Idle => "idle",
            ExitReason::Shutdown => "shutdown",
            ExitReason::SetupFailed { .. } => "setup_failed",
            ExitReason::Crashed { .. } => "crashed",
        }
    );
    if !matches!(reason, ExitReason::Idle) {
        shared
            .finish_turn(None, TurnOutcome::Failed(format!("「{name}」进程已退出")))
            .await;
    }
    shared.emit(AgentEvent::Exited { reason }).await;
    end
}

type Transport =
    ByteStreams<Async<std::process::ChildStdin>, BoundedLines<Async<std::process::ChildStdout>>>;

fn transport(process: &mut AgentProcess) -> io::Result<Transport> {
    let stdin = process
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("stdin 不可用"))?;
    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("stdout 不可用"))?;
    Ok(ByteStreams::new(
        Async::new(stdin)?,
        BoundedLines {
            inner: Async::new(stdout)?,
            since_newline: 0,
        },
    ))
}

/// The agent's message plus its `data` detail (often the real reason), shortened. Shown to
/// the user, never logged.
fn error_text(e: &sdk::Error) -> String {
    let mut text = e.message.clone();
    if let Some(data) = &e.data {
        let detail = match data {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        text.push('：');
        text.extend(detail.chars().take(400));
    }
    text
}

fn fs_error(e: io::Error) -> sdk::Error {
    match e.kind() {
        io::ErrorKind::NotFound => sdk::Error::resource_not_found(None).data(e.to_string()),
        _ => sdk::Error::new(-32603, e.to_string()),
    }
}

async fn connect(
    shared: Arc<Shared>,
    transport: Transport,
    commands: &async_channel::Receiver<Command>,
    turn_done: &async_channel::Receiver<()>,
    first: Option<(TurnId, Vec<PromptPart>)>,
    resume: &mut Option<acp::SessionId>,
) -> Result<End, sdk::Error> {
    let on_update = shared.clone();
    let on_permission = shared.clone();
    let on_read = shared.clone();
    let on_write = shared.clone();
    let session_shared = shared.clone();
    Client
        .builder()
        .name("zj")
        .on_receive_notification(
            async move |notification: acp::SessionNotification, _cx| {
                handle_update(&on_update, notification).await;
                Ok(())
            },
            sdk::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: acp::RequestPermissionRequest,
                        responder: Responder<acp::RequestPermissionResponse>,
                        cx: ConnectionTo<Agent>| {
                handle_permission(&on_permission, request, responder, &cx).await
            },
            sdk::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::ReadTextFileRequest,
                        responder: Responder<acp::ReadTextFileResponse>,
                        _cx: ConnectionTo<Agent>| {
                responder.respond_with_result(handle_read(&on_read, &request).await)
            },
            sdk::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::WriteTextFileRequest,
                        responder: Responder<acp::WriteTextFileResponse>,
                        _cx: ConnectionTo<Agent>| {
                let result = handle_write(&on_write, request).await;
                responder.respond_with_result(result)
            },
            sdk::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            run_session(session_shared, cx, commands, turn_done, first, resume).await
        })
        .await
}

async fn with_timeout<T>(future: impl Future<Output = T>, limit: Duration) -> Option<T> {
    let future = std::pin::pin!(future);
    match futures::future::select(future, Timer::after(limit)).await {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
}

async fn run_session(
    shared: Arc<Shared>,
    cx: ConnectionTo<Agent>,
    commands: &async_channel::Receiver<Command>,
    turn_done: &async_channel::Receiver<()>,
    first: Option<(TurnId, Vec<PromptPart>)>,
    resume: &mut Option<acp::SessionId>,
) -> Result<End, sdk::Error> {
    let name = shared.preset.display_name.clone();
    let timeout = shared.handshake_timeout;
    let fail = |shared: Arc<Shared>, message: String| async move {
        eprintln!("event=agent_handshake_failed agent={}", shared.preset.id);
        shared
            .emit(AgentEvent::Error {
                message: message.clone(),
            })
            .await;
        shared.finish_turn(None, TurnOutcome::Failed(message)).await;
        Ok(End::Failed)
    };

    let capabilities = acp::ClientCapabilities::new()
        .fs(acp::FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        // Not advertised in v1: agents run commands with their own tools, behind permission
        // requests, instead of in an editor-owned terminal.
        .terminal(false);
    let request = acp::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(capabilities)
        .client_info(acp::Implementation::new("zj", env!("CARGO_PKG_VERSION")).title("ZJ"));
    let init = match with_timeout(cx.send_request(request).block_task(), timeout).await {
        Some(Ok(init)) => init,
        Some(Err(e)) => {
            return fail(shared, format!("「{name}」握手失败：{}", error_text(&e))).await;
        }
        None => {
            return fail(
                shared,
                format!("「{name}」在 {} 秒内没有响应握手", timeout.as_secs()),
            )
            .await;
        }
    };
    let caps = &init.agent_capabilities;
    shared
        .embedded_context
        .store(caps.prompt_capabilities.embedded_context, Ordering::Relaxed);
    let info = AgentInfo {
        name: init.agent_info.as_ref().map(|i| i.name.clone()),
        title: init.agent_info.as_ref().and_then(|i| i.title.clone()),
        version: init.agent_info.as_ref().map(|i| i.version.clone()),
        load_session: caps.load_session,
        embedded_context: caps.prompt_capabilities.embedded_context,
        image: caps.prompt_capabilities.image,
        auth_methods: init
            .auth_methods
            .iter()
            .map(|m| m.id().to_string())
            .collect(),
    };
    shared.emit(AgentEvent::Ready(info)).await;

    let root = shared.workspace.root().to_path_buf();
    let policy = shared.preset.modes.clone();
    // Forbidden modes never reach the UI.
    let modes = |m: Option<acp::SessionModeState>| {
        m.map(|m| Modes {
            current: m.current_mode_id.to_string(),
            available: m
                .available_modes
                .iter()
                .filter(|mode| policy.allows(&mode.id.to_string()))
                .map(|mode| (mode.id.to_string(), mode.name.clone()))
                .collect(),
        })
    };
    let meta = shared
        .preset
        .session_meta
        .as_ref()
        .and_then(|m| m.as_object().cloned());
    let mut started_modes: Option<Modes> = None;
    let mut session = None;
    if let Some(previous) = resume.clone()
        && caps.load_session
    {
        shared.replaying.store(true, Ordering::Relaxed);
        let request =
            acp::LoadSessionRequest::new(previous.clone(), root.clone()).meta(meta.clone());
        let loaded = with_timeout(cx.send_request(request).block_task(), timeout).await;
        shared.replaying.store(false, Ordering::Relaxed);
        if let Some(Ok(response)) = loaded {
            let raw_current = response
                .modes
                .as_ref()
                .map(|m| m.current_mode_id.to_string());
            started_modes = modes(response.modes);
            if let (Some(m), Some(current)) = (started_modes.as_mut(), raw_current) {
                m.current = current;
            }
            shared
                .emit(AgentEvent::SessionStarted {
                    session_id: previous.to_string(),
                    resumed: true,
                    modes: started_modes.clone(),
                })
                .await;
            session = Some(previous);
        } else {
            eprintln!("event=agent_session_load_failed agent={}", shared.preset.id);
        }
    }
    let session = match session {
        Some(session) => session,
        None => {
            let request = acp::NewSessionRequest::new(root)
                .mcp_servers(Vec::new())
                .meta(meta.clone());
            match with_timeout(cx.send_request(request).block_task(), timeout).await {
                Some(Ok(response)) => {
                    let raw_current = response
                        .modes
                        .as_ref()
                        .map(|m| m.current_mode_id.to_string());
                    started_modes = modes(response.modes);
                    if let (Some(m), Some(current)) = (started_modes.as_mut(), raw_current) {
                        m.current = current;
                    }
                    shared
                        .emit(AgentEvent::SessionStarted {
                            session_id: response.session_id.to_string(),
                            resumed: false,
                            modes: started_modes.clone(),
                        })
                        .await;
                    response.session_id
                }
                Some(Err(e)) => {
                    return fail(
                        shared,
                        format!("「{name}」无法新建会话：{}", error_text(&e)),
                    )
                    .await;
                }
                None => return fail(shared, format!("「{name}」新建会话超时")).await,
            }
        }
    };
    *resume = Some(session.clone());
    enforce_mode(&shared, &cx, &session, started_modes.as_ref()).await;

    if let Some((turn, parts)) = first {
        start_prompt(&shared, &cx, &session, turn, parts)?;
    }
    loop {
        let busy = shared.turn.lock().unwrap().is_some();
        let idle_timeout = shared.idle_timeout;
        let idle = async move {
            if busy {
                futures::future::pending::<()>().await;
            } else {
                Timer::after(idle_timeout).await;
            }
        };
        let command = commands.recv().fuse();
        let done = turn_done.recv().fuse();
        let idle = idle.fuse();
        let closed = cx.incoming_closed().fuse();
        futures::pin_mut!(command, done, idle, closed);
        futures::select! {
            command = command => match command {
                Err(_) | Ok(Command::Shutdown) => return Ok(End::Shutdown),
                Ok(Command::Connect) => {}
                Ok(Command::Prompt { turn, parts }) => start_prompt(&shared, &cx, &session, turn, parts)?,
                Ok(Command::Cancel) => {
                    shared.cancel_permissions();
                    cx.send_notification(acp::CancelNotification::new(session.clone()))?;
                }
                Ok(Command::SetMode(mode)) => {
                    let request = cx.send_request(acp::SetSessionModeRequest::new(session.clone(), mode));
                    let shared = shared.clone();
                    cx.spawn(async move {
                        if let Err(e) = request.block_task().await {
                            shared.emit(AgentEvent::Error { message: format!("切换模式失败：{}", error_text(&e)) }).await;
                        }
                        Ok(())
                    })?;
                }
            },
            _ = done => {}
            _ = idle => return Ok(End::Idle),
            _ = closed => return Ok(End::Closed),
        }
    }
}

fn start_prompt(
    shared: &Arc<Shared>,
    cx: &ConnectionTo<Agent>,
    session: &acp::SessionId,
    turn: TurnId,
    parts: Vec<PromptPart>,
) -> Result<(), sdk::Error> {
    let blocks = prompt_blocks(&parts, shared.embedded_context.load(Ordering::Relaxed));
    let request = cx.send_request(acp::PromptRequest::new(session.clone(), blocks));
    let shared = shared.clone();
    cx.spawn(async move {
        let outcome = match request.block_task().await {
            Ok(response) => events::turn_outcome(&response.stop_reason),
            Err(e) => TurnOutcome::Failed(error_text(&e)),
        };
        shared.finish_turn(Some(turn), outcome).await;
        let _ = shared.turn_done.send(()).await;
        Ok(())
    })
}

/// Starts in the preset's asking mode, whatever the agent's own settings chose; a forbidden
/// current mode with no allowed alternative is reported.
async fn enforce_mode(
    shared: &Arc<Shared>,
    cx: &ConnectionTo<Agent>,
    session: &acp::SessionId,
    modes: Option<&Modes>,
) {
    let Some(modes) = modes else {
        return;
    };
    let policy = &shared.preset.modes;
    let target = match &policy.initial {
        Some(initial) if modes.available.iter().any(|(id, _)| id == initial) => {
            Some(initial.clone())
        }
        _ if !policy.allows(&modes.current) => modes.available.first().map(|(id, _)| id.clone()),
        _ => None,
    };
    match target {
        Some(target) if target != modes.current => {
            let request = acp::SetSessionModeRequest::new(session.clone(), target.clone());
            match with_timeout(
                cx.send_request(request).block_task(),
                shared.handshake_timeout,
            )
            .await
            {
                Some(Ok(_)) => {
                    eprintln!(
                        "event=agent_mode_set agent={} mode={target}",
                        shared.preset.id
                    );
                    shared
                        .emit(AgentEvent::ModeChanged { mode_id: target })
                        .await;
                }
                _ => {
                    shared
                        .emit(AgentEvent::Error {
                            message: format!("无法把 Agent 切换到询问模式（{target}）"),
                        })
                        .await;
                }
            }
        }
        None if !policy.allows(&modes.current) => {
            shared
                .emit(AgentEvent::Error {
                    message: "Agent 处于跳过审批的模式，且没有可切换的询问模式".into(),
                })
                .await;
        }
        _ => {}
    }
}

async fn handle_update(shared: &Shared, notification: acp::SessionNotification) {
    let Some(mut event) = events::from_update(&notification.update) else {
        return;
    };
    // Direct mode reviews diff against the file as it was before the first edit; an edit tool
    // call announces its paths before it runs.
    if let AgentEvent::ToolCall(call) = &event
        && matches!(
            call.kind,
            events::ToolKind::Edit | events::ToolKind::Delete | events::ToolKind::Move
        )
        && *shared.write_mode.lock().unwrap() == WriteMode::Direct
    {
        for location in &call.locations {
            if let Ok(path) = shared.workspace.resolve(&location.path) {
                shared.snapshot(&path).await;
            }
        }
    }
    // A mode the preset forbids is never shown as current without a warning.
    if let AgentEvent::ModeChanged { mode_id } = &event
        && !shared.preset.modes.allows(mode_id)
    {
        event = AgentEvent::Error {
            message: format!("Agent 切换到了跳过审批的模式（{mode_id}），请在模式菜单里改回询问"),
        };
    }
    let history = matches!(
        event,
        AgentEvent::UserMessageChunk { .. }
            | AgentEvent::MessageChunk { .. }
            | AgentEvent::ThoughtChunk { .. }
            | AgentEvent::ToolCall(_)
            | AgentEvent::ToolCallUpdate(_)
            | AgentEvent::Plan(_)
    );
    if history && shared.replaying.load(Ordering::Relaxed) {
        return;
    }
    shared.emit(event).await;
}

/// Holds the dispatch loop only to register and announce the request; the answer is awaited
/// in a spawned task.
async fn handle_permission(
    shared: &Arc<Shared>,
    request: acp::RequestPermissionRequest,
    responder: Responder<acp::RequestPermissionResponse>,
    cx: &ConnectionTo<Agent>,
) -> Result<(), sdk::Error> {
    let id = shared.next_permission.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = oneshot::channel();
    shared.permissions.lock().unwrap().insert(id, tx);
    shared
        .emit(AgentEvent::PermissionRequested(events::PermissionRequest {
            id,
            tool_call: events::tool_patch(&request.tool_call),
            options: events::permission_options(&request.options),
        }))
        .await;
    cx.spawn(async move {
        let outcome = match rx.await {
            Ok(Some(option)) => {
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(option))
            }
            _ => acp::RequestPermissionOutcome::Cancelled,
        };
        responder.respond(acp::RequestPermissionResponse::new(outcome))
    })
}

async fn handle_read(
    shared: &Shared,
    request: &acp::ReadTextFileRequest,
) -> Result<acp::ReadTextFileResponse, sdk::Error> {
    let path = shared.workspace.resolve(&request.path).map_err(fs_error)?;
    let text = match shared.shadow.proposed_text(&path) {
        Some(text) => text,
        None => match shared.buffer_text(&path).await.map_err(fs_error)? {
            Some(text) => text,
            None => read_disk(&path).map_err(fs_error)?,
        },
    };
    Ok(acp::ReadTextFileResponse::new(window(
        text,
        request.line,
        request.limit,
    )))
}

async fn handle_write(
    shared: &Shared,
    request: acp::WriteTextFileRequest,
) -> Result<acp::WriteTextFileResponse, sdk::Error> {
    let path = shared.workspace.resolve(&request.path).map_err(fs_error)?;
    let mode = *shared.write_mode.lock().unwrap();
    match mode {
        WriteMode::Direct => {
            shared.snapshot(&path).await;
            write_atomic(&path, &request.content).map_err(fs_error)?;
            shared.emit(AgentEvent::FileWritten { path }).await;
        }
        WriteMode::AcceptFirst => {
            shared
                .shadow
                .propose(&path, request.content)
                .map_err(fs_error)?;
            shared.emit(AgentEvent::EditProposed { path }).await;
        }
    }
    Ok(acp::WriteTextFileResponse::new())
}

fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub(crate) fn prompt_blocks(parts: &[PromptPart], embedded: bool) -> Vec<acp::ContentBlock> {
    parts
        .iter()
        .map(|part| match part {
            PromptPart::Text(text) => acp::ContentBlock::Text(acp::TextContent::new(text.clone())),
            PromptPart::File(path) => acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
                file_name(path),
                file_uri(path),
            )),
            PromptPart::Selection {
                path,
                start_line,
                end_line,
                text,
            } => {
                let uri = format!("{}#L{start_line}:{end_line}", file_uri(path));
                match text {
                    Some(text) if embedded => {
                        acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                            acp::EmbeddedResourceResource::TextResourceContents(
                                acp::TextResourceContents::new(text.clone(), uri),
                            ),
                        ))
                    }
                    _ => acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
                        format!("{} ({start_line}-{end_line})", file_name(path)),
                        uri,
                    )),
                }
            }
        })
        .collect()
}

/// Fails the read once a single line exceeds [`MAX_LINE`], so a runaway agent cannot grow the
/// line buffer without bound.
pub(crate) struct BoundedLines<R> {
    inner: R,
    since_newline: usize,
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedLines<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let n = match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(n)) => n,
            other => return other,
        };
        match buf[..n].iter().rposition(|&b| b == b'\n') {
            Some(pos) => this.since_newline = n - pos - 1,
            None => this.since_newline += n,
        }
        if this.since_newline > MAX_LINE {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Agent 输出的单条消息超过 32 MB",
            )));
        }
        Poll::Ready(Ok(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_blocks_link_or_embed() {
        let parts = vec![
            PromptPart::Text("看看".into()),
            PromptPart::File("/w/src/a b.rs".into()),
            PromptPart::Selection {
                path: "/w/src/main.rs".into(),
                start_line: 3,
                end_line: 9,
                text: Some("fn main() {}".into()),
            },
        ];
        let linked = prompt_blocks(&parts, false);
        match &linked[1] {
            acp::ContentBlock::ResourceLink(link) => {
                assert_eq!(link.uri, "file:///w/src/a%20b.rs");
                assert_eq!(link.name, "a b.rs");
            }
            other => panic!("{other:?}"),
        }
        match &linked[2] {
            acp::ContentBlock::ResourceLink(link) => {
                assert_eq!(link.uri, "file:///w/src/main.rs#L3:9")
            }
            other => panic!("{other:?}"),
        }
        let embedded = prompt_blocks(&parts, true);
        assert!(matches!(embedded[2], acp::ContentBlock::Resource(_)));
    }
}
