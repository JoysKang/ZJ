//! A session with an agent: its commands, turns, permission requests and review snapshots.
//! It runs as a task on the agent's process (`host.rs`), which starts on demand, may be shared
//! with other sessions of the same agent, and restarts on the next prompt after it stopped.

use crate::{
    events::{
        self, AgentEvent, AgentInfo, AuthChoice, ConfigOption, ExitReason, Modes, PermissionId,
        TurnId, TurnOutcome,
    },
    fs::{BufferProvider, Workspace, read_disk, window, write_atomic},
    host::{AgentPool, Host, HostInner, HostKey},
    login,
    process::AgentProcess,
    provision,
    registry::{AgentPreset, ResolvedLaunch, SearchPath},
};
use agent_client_protocol::{
    self as sdk, Agent, ByteStreams, ConnectionTo, Responder, schema::v1 as acp,
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
    time::Duration,
};

#[path = "steering.rs"]
mod steering;

pub struct ClientOptions {
    pub preset: AgentPreset,
    /// The agent's `cwd`; every `fs/*` path must stay inside it.
    pub workspace_root: PathBuf,
    /// Stop the process after this long without a turn, prompt or permission request.
    pub idle_timeout: Duration,
    /// How long a cancelled turn may take to end before it is ended without the agent.
    pub cancel_grace: Duration,
    /// Initialize / session setup.
    pub handshake_timeout: Duration,
    /// Values from the settings file, e.g. `ANTHROPIC_AUTH_TOKEN` for the DeepSeek preset.
    pub env_overrides: BTreeMap<String, String>,
    pub buffers: Option<Arc<dyn BufferProvider>>,
    /// Defaults to [`SearchPath::from_env`].
    pub search_path: Option<SearchPath>,
    /// Where npm adapters and ZJ's Node.js are installed on first use; `None` only uses
    /// commands already on this machine. Defaults to [`provision::default_root`].
    pub install_root: Option<PathBuf>,
    /// An ACP session id from an earlier run (history); restored with `session/load` when the
    /// agent supports it, otherwise a new session starts.
    pub resume_session: Option<String>,
    /// Shares the agent's process with the pool's other sessions of the same agent and
    /// variables; `None` gives this session a process of its own.
    pub pool: Option<Arc<AgentPool>>,
    /// A text-generation session: refuse ACP file access and tool approval requests.
    pub text_only: bool,
}

impl ClientOptions {
    pub fn new(preset: AgentPreset, workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            preset,
            workspace_root: workspace_root.into(),
            idle_timeout: Duration::from_secs(10 * 60),
            cancel_grace: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(180),
            env_overrides: BTreeMap::new(),
            buffers: None,
            search_path: None,
            install_root: provision::default_root(),
            resume_session: None,
            pool: None,
            text_only: false,
        }
    }
}

/// Context attached to a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptPart {
    Text(String),
    /// Base64 image bytes, as ACP's image content block.
    Image {
        data: Arc<str>,
        mime_type: String,
    },
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
    /// This adapter did not advertise the steering extension.
    SteeringUnavailable,
    ImagesUnavailable,
    Stopped,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ClientError::Busy => "Agent 正在回复，请等这一轮结束或先中断",
            ClientError::SteeringUnavailable => {
                "当前 Agent 尚不能接收运行中的补充指令，可等待完成或先停止"
            }
            ClientError::Stopped => "Agent 客户端已关闭",
            ClientError::ImagesUnavailable => "当前 Agent 不支持图片输入，请更换支持图片的 Agent",
        })
    }
}

impl std::error::Error for ClientError {}

const EVENT_CAPACITY: usize = 512;
const COMMAND_CAPACITY: usize = 64;
/// One JSON-RPC line from the agent (a tool call can carry two copies of a large file).
const MAX_LINE: usize = 32 * 1024 * 1024;
/// Files remembered for reviews per session, and their total size.
const MAX_SNAPSHOTS: usize = 256;
const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
/// How long a read waits for the editor's unsaved buffer.
const BUFFER_TIMEOUT: Duration = Duration::from_secs(5);
/// `authenticate` may wait for the user to finish signing in in the browser.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// `session/close` when a session goes idle or away.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) enum Command {
    Connect,
    Prompt {
        turn: TurnId,
        parts: Vec<PromptPart>,
    },
    Steer {
        turn: TurnId,
        parts: Vec<PromptPart>,
    },
    Cancel,
    SetMode(String),
    SetConfig {
        id: String,
        value: String,
    },
    Login(String),
    RetryLogin(BTreeMap<String, String>),
    Shutdown,
}

/// What the agent said about itself in `initialize`, for every session on its process.
pub(crate) struct Init {
    pub info: AgentInfo,
    pub load_session: bool,
    pub close_session: bool,
    pub embedded_context: bool,
    pub steering: bool,
    pub auth_methods: Vec<acp::AuthMethod>,
}

/// One session: its commands, turn, permission requests and review snapshots. It runs as a
/// task on its agent's process ([`Host`]), which it may share with other sessions.
pub(crate) struct SessionState {
    preset: AgentPreset,
    workspace: Workspace,
    text_only: bool,
    idle_timeout: Duration,
    cancel_grace: Duration,
    handshake_timeout: Duration,
    /// The model settings last offered: [`AgentClient::set_config_option`] only picks from
    /// these, so an option ZJ does not show (e.g. a mode) cannot be set through it.
    configs: Mutex<Vec<ConfigOption>>,
    buffers: Option<Arc<dyn BufferProvider>>,
    /// Closed by [`AgentClient::stop`]: a read waiting for the editor gives up instead of
    /// holding the session until the timeout.
    stopping: async_channel::Receiver<()>,
    events: async_channel::Sender<AgentEvent>,
    commands: async_channel::Receiver<Command>,
    /// Wakes the session loop when a turn finishes (re-arms the idle timer).
    turn_done: async_channel::Sender<()>,
    turn_done_rx: async_channel::Receiver<()>,
    /// A prompt the agent refused with "auth required" (sent before `turn_done`): the turn
    /// stays open, the session loop asks for a login and sends it again.
    parked: Mutex<Option<(TurnId, Vec<PromptPart>)>>,
    permissions: Mutex<HashMap<PermissionId, oneshot::Sender<Option<String>>>>,
    next_permission: AtomicU64,
    turn: Mutex<Option<TurnId>>,
    steering: Mutex<steering::Steering>,
    steering_supported: AtomicBool,
    image_support: Mutex<Option<bool>>,
    next_turn: AtomicU64,
    /// `session/load` replays history as updates; the UI already has it.
    replaying: AtomicBool,
    /// Each file's content before the agent first touched it in this session
    /// (`None` = the file did not exist), for "working tree vs. before the agent" reviews.
    snapshots: Mutex<BTreeMap<PathBuf, Option<String>>>,
    /// The "too many snapshots" notice was shown.
    snapshots_full: AtomicBool,
    /// Files of edit tool calls still running, by call id: an agent that writes them itself
    /// (Codex) is reported as having written them once the call completes.
    edits: Mutex<HashMap<String, Vec<PathBuf>>>,
    /// The agent's id for this session, kept across restarts so `session/load` can restore it.
    resume: Mutex<Option<acp::SessionId>>,
    /// The (allowed) mode the session was last in: a restart returns to it.
    mode: Mutex<Option<String>>,
    /// A prompt to send as soon as the session runs again (after moving to another process).
    pending_first: Mutex<Option<(TurnId, Vec<PromptPart>)>>,
    /// Running on a process, or waiting to.
    running: AtomicBool,
    /// Idle on an agent that cannot close sessions: the process may stop.
    idle: AtomicBool,
    /// This run on the process already ended with an `Exited` event.
    exit_told: AtomicBool,
    /// Cleared by [`AgentClient::stop`]; the last client of a process stops it.
    host: Mutex<Option<Arc<Host>>>,
    pool: Option<Arc<AgentPool>>,
}

impl SessionState {
    /// Waits for a reply at most `limit`, and not at all once the client stops.
    async fn bounded<T>(&self, future: impl Future<Output = T>, limit: Duration) -> Option<T> {
        let future = std::pin::pin!(future);
        let stopping = std::pin::pin!(self.stopping.recv());
        match with_timeout(futures::future::select(future, stopping), limit).await {
            Some(Either::Left((value, _))) => Some(value),
            _ => None,
        }
    }

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
    /// A file the agent's change cannot be reviewed for is said so in the thread, not
    /// silently left out of the changed files.
    async fn snapshot(&self, path: &Path) {
        if !self.snapshot_wanted(path).await {
            return;
        }
        let before = match self.buffer_text(path).await {
            Ok(Some(text)) => Some(text),
            Ok(None) | Err(_) => match read_disk(path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.emit(AgentEvent::Error {
                        message: format!("没能记录 {name} 改动前的内容（{e}），它不能对比或还原"),
                    })
                    .await;
                    return;
                }
            },
        };
        self.keep_snapshot(path, before).await;
    }

    /// `path` has no snapshot yet and there is room for one (the notice says when there is not).
    async fn snapshot_wanted(&self, path: &Path) -> bool {
        let (known, full) = {
            let snapshots = self.snapshots.lock().unwrap();
            let known = snapshots.contains_key(path);
            (known, !known && snapshots.len() >= MAX_SNAPSHOTS)
        };
        if full && !self.snapshots_full.swap(true, Ordering::Relaxed) {
            self.emit(AgentEvent::Error {
                message: format!(
                    "这个会话已记录 {MAX_SNAPSHOTS} 个文件改动前的内容，之后改动的文件不能对比或还原"
                ),
            })
            .await;
        }
        !known && !full
    }

    /// The file before an edit the agent applies itself (Codex writes its patches without ZJ):
    /// when its notice arrives the file may already be written, so the text before is rebuilt
    /// from the edit's hunks. `diffs` are the call's `(old, new)` texts for `path` (`old`
    /// `None`: a new file).
    async fn snapshot_edit(&self, path: &Path, diffs: &[(Option<String>, String)]) {
        if !self.snapshot_wanted(path).await {
            return;
        }
        let current = match read_disk(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            // Says why in the thread.
            Err(_) => return self.snapshot(path).await,
        };
        let before = match (&current, diffs) {
            // Created: unless the file was there with other text (overwritten).
            (Some(current), [(None, new)]) if current != new => Some(current.clone()),
            (_, [(None, _)]) => None,
            // Deleted already: a delete's old text is the whole file.
            (None, [(Some(old), new)]) if new.is_empty() => Some(old.clone()),
            (None, _) => None,
            (Some(current), _) => {
                let hunks: Option<Vec<(&str, &str)>> = diffs
                    .iter()
                    .map(|(old, new)| Some((old.as_deref()?, new.as_str())))
                    .collect();
                // Not told apart: an agent writing through ZJ is snapshotted when it writes;
                // one that wrote it itself is said so once the edit completes.
                match hunks.and_then(|hunks| crate::review::text_before_hunks(current, &hunks)) {
                    Some(before) => Some(before),
                    None => return,
                }
            }
        };
        self.keep_snapshot(path, before).await;
    }

    /// Keeps `before` as `path`'s snapshot, within the session's limits.
    async fn keep_snapshot(&self, path: &Path, before: Option<String>) {
        let over = {
            let mut snapshots = self.snapshots.lock().unwrap();
            if snapshots.contains_key(path) || snapshots.len() >= MAX_SNAPSHOTS {
                return;
            }
            let held: usize = snapshots.values().flatten().map(String::len).sum();
            let over = held + before.as_ref().map_or(0, String::len) > MAX_SNAPSHOT_BYTES;
            if !over {
                snapshots.insert(path.to_path_buf(), before);
            }
            over
        };
        if over {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            self.emit(AgentEvent::Error {
                message: format!(
                    "这个会话记录的改动前内容已超过 {} MB，{name} 不能对比或还原",
                    MAX_SNAPSHOT_BYTES / 1024 / 1024
                ),
            })
            .await;
        }
    }

    pub(crate) async fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event).await;
    }

    /// From the process's thread outside of async code (install progress).
    pub(crate) fn emit_blocking(&self, event: AgentEvent) {
        let _ = self.events.send_blocking(event);
    }

    /// Remembers the model settings the agent offers and tells the UI.
    async fn offer_configs(&self, options: &[acp::SessionConfigOption]) {
        let configs = events::config_options(options);
        *self.configs.lock().unwrap() = configs.clone();
        self.emit(AgentEvent::ConfigOptions(configs)).await;
    }

    /// Ends the current turn exactly once (from the prompt task or the process).
    pub(crate) async fn finish_turn(&self, only: Option<TurnId>, outcome: TurnOutcome) {
        self.end_turn(only, outcome, only.is_none()).await;
    }

    async fn end_turn(&self, only: Option<TurnId>, outcome: TurnOutcome, force: bool) {
        let turn = {
            let mut current = self.turn.lock().unwrap();
            let mut steering = self.steering.lock().unwrap();
            if only.is_none() {
                *steering = Default::default();
            }
            match *current {
                Some(t) if only.is_none_or(|o| o == t) => {
                    if (!force || (steering.pending.is_some() && !steering.cancelled))
                        && steering.hold_end(outcome.clone())
                    {
                        return;
                    }
                    // A cancelled steering request may still start a detached turn after
                    // the cancel watchdog ends this turn. Keep its ownership until its
                    // response can be cancelled; process exit (only == None) releases it.
                    if only.is_some() {
                        steering.retire();
                    } else {
                        *steering = Default::default();
                    }
                    current.take()
                }
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

    pub(crate) fn handshake_timeout(&self) -> Duration {
        self.handshake_timeout
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopping.is_closed()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.idle.load(Ordering::Relaxed)
    }

    pub(crate) fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
        if !running {
            self.steering_supported.store(false, Ordering::Relaxed);
        }
    }

    /// Starts running on the process.
    pub(crate) fn joined(&self) {
        self.idle.store(false, Ordering::Relaxed);
        self.exit_told.store(false, Ordering::SeqCst);
    }

    fn host(&self) -> Option<Arc<Host>> {
        self.host.lock().unwrap().clone()
    }

    /// Something needs the agent: the session asks its process to run it (started if needed).
    fn kick(self: &Arc<Self>) {
        if self.is_stopped() || self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        if !self
            .host()
            .is_some_and(|host| host.inner.attach(self.clone()))
        {
            self.set_running(false);
        }
    }

    /// The process could not start: what asked for it is dropped (its turn already failed or
    /// was cancelled), so it is not asked again until the user does something.
    pub(crate) fn abandon(&self) {
        while self.commands.try_recv().is_ok() {}
        self.pending_first.lock().unwrap().take();
    }

    /// After the session stopped running: commands that came meanwhile start it again.
    fn kick_if_pending(self: &Arc<Self>) {
        if !self.commands.is_empty() || self.pending_first.lock().unwrap().is_some() {
            self.kick();
        }
    }

    /// The process this session ran on stopped (idle, crashed, shut down or failed to start).
    pub(crate) async fn process_gone(self: &Arc<Self>, reason: &ExitReason, name: &str) {
        self.steering_supported.store(false, Ordering::Relaxed);
        *self.steering.lock().unwrap() = Default::default();
        self.cancel_permissions();
        if self.exit_told.swap(true, Ordering::SeqCst) {
            return;
        }
        self.replaying.store(false, Ordering::Relaxed);
        if !matches!(reason, ExitReason::Idle) {
            self.finish_turn(None, TurnOutcome::Failed(format!("「{name}」进程已退出")))
                .await;
        }
        self.emit(AgentEvent::Exited {
            reason: reason.clone(),
        })
        .await;
        self.set_running(false);
        self.kick_if_pending();
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

/// One session with an agent. Sessions of the same agent and variables share its process when
/// they come from the same [`AgentPool`].
pub struct AgentClient {
    session: Arc<SessionState>,
    commands: async_channel::Sender<Command>,
    events: async_channel::Receiver<AgentEvent>,
    stopping: async_channel::Sender<()>,
}

impl AgentClient {
    /// The agent process itself starts (or is joined) on the first [`prompt`](Self::prompt)
    /// or [`connect`](Self::connect).
    pub fn start(options: ClientOptions) -> io::Result<Self> {
        let workspace = Workspace::new(&options.workspace_root)?;
        let key = HostKey {
            preset: options.preset.clone(),
            env: options.env_overrides,
            search: options.search_path.unwrap_or_else(SearchPath::from_env),
            install_root: options.install_root,
        };
        let host = match &options.pool {
            Some(pool) => pool.host(key, options.buffers.clone())?,
            None => Host::spawn(key, options.buffers.clone())?,
        };
        let (events_tx, events_rx) = async_channel::bounded(EVENT_CAPACITY);
        let (commands_tx, commands_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let (done_tx, done_rx) = async_channel::unbounded();
        let (stopping_tx, stopping_rx) = async_channel::bounded(1);
        let session = Arc::new(SessionState {
            preset: options.preset,
            workspace,
            text_only: options.text_only,
            idle_timeout: options.idle_timeout,
            cancel_grace: options.cancel_grace,
            handshake_timeout: options.handshake_timeout,
            configs: Mutex::new(Vec::new()),
            buffers: options.buffers,
            stopping: stopping_rx,
            events: events_tx,
            commands: commands_rx,
            turn_done: done_tx,
            turn_done_rx: done_rx,
            parked: Mutex::new(None),
            permissions: Mutex::new(HashMap::new()),
            next_permission: AtomicU64::new(1),
            turn: Mutex::new(None),
            steering: Mutex::new(Default::default()),
            steering_supported: AtomicBool::new(false),
            image_support: Mutex::new(None),
            next_turn: AtomicU64::new(1),
            replaying: AtomicBool::new(false),
            snapshots: Mutex::new(BTreeMap::new()),
            snapshots_full: AtomicBool::new(false),
            edits: Mutex::new(HashMap::new()),
            resume: Mutex::new(options.resume_session.map(acp::SessionId::new)),
            mode: Mutex::new(None),
            pending_first: Mutex::new(None),
            running: AtomicBool::new(false),
            idle: AtomicBool::new(false),
            exit_told: AtomicBool::new(false),
            host: Mutex::new(Some(host)),
            pool: options.pool,
        });
        Ok(Self {
            session,
            commands: commands_tx,
            events: events_rx,
            stopping: stopping_tx,
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
        self.session.kick();
    }

    pub fn prompt(&self, parts: Vec<PromptPart>) -> Result<TurnId, ClientError> {
        let session = &self.session;
        if *session.image_support.lock().unwrap() == Some(false)
            && parts.iter().any(|p| matches!(p, PromptPart::Image { .. }))
        {
            return Err(ClientError::ImagesUnavailable);
        }
        let turn = {
            let mut current = session.turn.lock().unwrap();
            if current.is_some() {
                return Err(ClientError::Busy);
            }
            let mut steering = session.steering.lock().unwrap();
            if steering.pending.is_some() {
                return Err(ClientError::Busy);
            }
            let turn = session.next_turn.fetch_add(1, Ordering::Relaxed);
            *current = Some(turn);
            *steering = Default::default();
            turn
        };
        // A new turn clears an earlier stop. Not when the install starts: a stop pressed
        // while the agent was still being resolved would be lost.
        if let Some(host) = session.host() {
            host.inner.install_cancel.store(false, Ordering::Relaxed);
        }
        if let Err(e) = self.commands.try_send(Command::Prompt { turn, parts }) {
            *session.turn.lock().unwrap() = None;
            return Err(if e.is_full() {
                ClientError::Busy
            } else {
                ClientError::Stopped
            });
        }
        session.kick();
        Ok(turn)
    }

    /// Adds instructions to the running turn without cancelling it or its permissions.
    /// Only one steering request is in flight; a rejected submission stays with the caller.
    pub fn steer(&self, parts: Vec<PromptPart>) -> Result<TurnId, ClientError> {
        if *self.session.image_support.lock().unwrap() == Some(false)
            && parts.iter().any(|p| matches!(p, PromptPart::Image { .. }))
        {
            return Err(ClientError::ImagesUnavailable);
        }
        let current = self.session.turn.lock().unwrap();
        let Some(turn) = *current else {
            return Err(ClientError::Busy);
        };
        if !self.supports_steering() || self.session.parked.lock().unwrap().is_some() {
            return Err(ClientError::SteeringUnavailable);
        }
        let mut steering = self.session.steering.lock().unwrap();
        if steering.pending.is_some() || steering.followup.is_some() || steering.cancelled {
            return Err(ClientError::Busy);
        }
        steering.pending = Some(turn);
        if let Err(error) = self.commands.try_send(Command::Steer { turn, parts }) {
            steering.pending = None;
            return Err(if error.is_full() {
                ClientError::Busy
            } else {
                ClientError::Stopped
            });
        }
        Ok(turn)
    }

    pub fn supports_steering(&self) -> bool {
        self.session.steering_supported.load(Ordering::Relaxed)
    }

    /// `session/cancel`; pending permission requests are answered `cancelled`. The turn ends
    /// with [`TurnOutcome::Cancelled`] once the agent stops.
    pub fn cancel(&self) {
        self.session.steering.lock().unwrap().cancelled = true;
        if let Some(host) = self.session.host() {
            host.inner.install_cancel.store(true, Ordering::Relaxed);
        }
        self.session.cancel_permissions();
        let _ = self.commands.try_send(Command::Cancel);
    }

    /// Signs in with one of the methods from [`AgentEvent::AuthRequired`]: `authenticate` for
    /// agent methods (e.g. Codex opens the browser), Terminal.app for terminal methods.
    pub fn login(&self, method_id: impl Into<String>) {
        let _ = self.commands.try_send(Command::Login(method_id.into()));
        self.session.kick();
    }

    /// Tries again after a login finished elsewhere (in the terminal) or after the user changed
    /// the agent's variables: with other `env_overrides` (resolved again from the settings) the
    /// session moves to a process started with them; a prompt waiting for the login is sent
    /// once the session starts.
    pub fn retry_login(&self, env_overrides: BTreeMap<String, String>) {
        let _ = self.commands.try_send(Command::RetryLogin(env_overrides));
        self.session.kick();
    }

    /// Answers a permission request with one of its option ids (`None` = dismissed, sent as
    /// `cancelled`). Returns `false` if the request is no longer pending.
    pub fn respond_permission(&self, id: PermissionId, option_id: Option<String>) -> bool {
        match self.session.permissions.lock().unwrap().remove(&id) {
            Some(tx) => tx.send(option_id).is_ok(),
            None => false,
        }
    }

    /// Switches the session mode; modes the preset forbids (bypass / full access) are refused
    /// and `false` is returned.
    pub fn set_mode(&self, mode_id: impl Into<String>) -> bool {
        let mode_id = mode_id.into();
        if !self.session.preset.modes.allows(&mode_id) {
            eprintln!("event=agent_mode_refused agent={}", self.session.preset.id);
            return false;
        }
        self.commands.try_send(Command::SetMode(mode_id)).is_ok()
    }

    /// Picks `value` for a model setting from [`AgentEvent::ConfigOptions`]; refused (`false`)
    /// when the agent does not offer that option and value.
    pub fn set_config_option(&self, id: impl Into<String>, value: impl Into<String>) -> bool {
        let (id, value) = (id.into(), value.into());
        let offered = self
            .session
            .configs
            .lock()
            .unwrap()
            .iter()
            .any(|o| o.id == id && o.offers(&value));
        if !offered {
            eprintln!(
                "event=agent_config_refused agent={}",
                self.session.preset.id
            );
            return false;
        }
        self.commands
            .try_send(Command::SetConfig { id, value })
            .is_ok()
    }

    /// Paths the agent changed (or announced an edit for), with their content
    /// before that first change.
    pub fn snapshot_paths(&self) -> Vec<PathBuf> {
        self.session
            .snapshots
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// `Some(None)`: the file did not exist before the agent created it.
    pub fn snapshot(&self, path: &Path) -> Option<Option<String>> {
        self.session.snapshots.lock().unwrap().get(path).cloned()
    }

    /// Replaces a file's "before" version after a partial review (`None` forgets the file:
    /// everything the agent changed in it was accepted or reverted).
    pub fn set_snapshot(&self, path: &Path, before: Option<Option<String>>) {
        let mut snapshots = self.session.snapshots.lock().unwrap();
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
        self.session.snapshots.lock().unwrap().clear();
    }

    /// The agent's process id while it runs (shared with the other sessions on it).
    pub fn pid(&self) -> Option<u32> {
        self.session
            .host()
            .and_then(|host| *host.inner.pid.lock().unwrap())
    }

    pub fn is_busy(&self) -> bool {
        self.session.turn.lock().unwrap().is_some()
    }

    /// Ends the session; the last session on a process stops it (process group included) and
    /// waits for it.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.session.cancel_permissions();
        // The session's task closes it in the agent; a full queue must not keep it alive.
        let _ = self.commands.try_send(Command::Shutdown);
        self.commands.close();
        // Nobody may read events any more; unblock a task waiting to send one.
        self.events.close();
        self.stopping.close();
        // The last handle stops the process and joins its thread.
        drop(self.session.host.lock().unwrap().take());
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        self.stop();
    }
}

enum SessionEnd {
    /// Closed after the idle timeout (or a cancelled login); runs again on the next prompt.
    Idle,
    Shutdown,
    /// The connection closed: the process tells the session.
    Closed,
    /// Session setup failed; already reported.
    Failed,
    /// [`AgentClient::retry_login`] with other variables: the session continues on a process
    /// started with them.
    Move(BTreeMap<String, String>),
}

/// A session's task on its process: opens (or restores) it, runs its commands and turns, and
/// closes it when idle or shut down.
pub(crate) async fn run_session(
    session: Arc<SessionState>,
    cx: ConnectionTo<Agent>,
    init: Arc<Init>,
    launch: Arc<ResolvedLaunch>,
    host: Arc<HostInner>,
) -> Result<(), sdk::Error> {
    while session.turn_done_rx.try_recv().is_ok() {}
    let end = match session_body(&session, &cx, &init, &launch, &host).await {
        Ok(end) => end,
        Err(e) => {
            eprintln!(
                "event=agent_session_error agent={} error={}",
                session.preset.id,
                error_text(&e)
            );
            SessionEnd::Closed
        }
    };
    host.leave(&session);
    let closed = matches!(end, SessionEnd::Closed);
    match end {
        // The last session leaving stops the process, which then says so; with others still on
        // it, the session says it now.
        SessionEnd::Idle if host.has_members() => {
            if !session.exit_told.swap(true, Ordering::SeqCst) {
                session
                    .emit(AgentEvent::Exited {
                        reason: ExitReason::Idle,
                    })
                    .await;
            }
        }
        SessionEnd::Failed => {
            if !session.exit_told.swap(true, Ordering::SeqCst) {
                session
                    .emit(AgentEvent::Exited {
                        reason: ExitReason::SetupFailed {
                            stderr_tail: String::new(),
                        },
                    })
                    .await;
            }
        }
        SessionEnd::Move(env) => move_session(&session, &host, env).await,
        SessionEnd::Idle | SessionEnd::Shutdown | SessionEnd::Closed => {}
    }
    if !closed {
        session.set_running(false);
        session.kick_if_pending();
    }
    Ok(())
}

/// Hands the session to the process for `env` (started if needed); this process no longer
/// tells it anything.
async fn move_session(
    session: &Arc<SessionState>,
    host: &HostInner,
    env: BTreeMap<String, String>,
) {
    let mut key = host.key.clone();
    key.env = env;
    let next = match &session.pool {
        Some(pool) => pool.host(key, host.buffers.clone()),
        None => Host::spawn(key, host.buffers.clone()),
    };
    match next {
        Ok(next) => {
            session.exit_told.store(true, Ordering::SeqCst);
            let mut slot = session.host.lock().unwrap();
            if slot.is_some() {
                // May let go of this process's last handle; it then stops by itself.
                *slot = Some(next);
            }
        }
        Err(e) => {
            let message = format!("无法启动「{}」：{e}", session.preset.display_name);
            session
                .emit(AgentEvent::Error {
                    message: message.clone(),
                })
                .await;
            session.pending_first.lock().unwrap().take();
            session
                .finish_turn(None, TurnOutcome::Failed(message))
                .await;
        }
    }
}

/// Frees the session in the agent (Claude ends its CLI, Codex its thread subscription).
async fn close_session(
    session: &SessionState,
    cx: &ConnectionTo<Agent>,
    init: &Init,
    host: &HostInner,
    id: &acp::SessionId,
) {
    host.unregister(id);
    if !init.close_session {
        return;
    }
    let request = cx.send_request(acp::CloseSessionRequest::new(id.clone()));
    if matches!(
        with_timeout(request.block_task(), CLOSE_TIMEOUT).await,
        Some(Ok(_))
    ) {
        *session.steering.lock().unwrap() = Default::default();
    } else {
        eprintln!(
            "event=agent_session_close_failed agent={}",
            session.preset.id
        );
    }
}

async fn fail(session: &SessionState, message: String) -> Result<SessionEnd, sdk::Error> {
    eprintln!("event=agent_session_failed agent={}", session.preset.id);
    session
        .emit(AgentEvent::Error {
            message: message.clone(),
        })
        .await;
    session
        .finish_turn(None, TurnOutcome::Failed(message))
        .await;
    Ok(SessionEnd::Failed)
}

async fn session_body(
    s: &Arc<SessionState>,
    cx: &ConnectionTo<Agent>,
    init: &Init,
    launch: &ResolvedLaunch,
    host: &HostInner,
) -> Result<SessionEnd, sdk::Error> {
    let name = s.preset.display_name.clone();
    let timeout = s.handshake_timeout;
    // What started the session: a prompt is sent first; a stop or a mode switch sent while it
    // was not running is moot.
    let mut first = s.pending_first.lock().unwrap().take();
    if first.is_none() {
        loop {
            match s.commands.try_recv() {
                Ok(Command::Prompt { turn, parts }) => {
                    first = Some((turn, parts));
                    break;
                }
                Ok(Command::Connect | Command::Login(_)) => break,
                Ok(Command::RetryLogin(env)) if env != host.key.env => {
                    return Ok(SessionEnd::Move(env));
                }
                Ok(Command::RetryLogin(_)) => break,
                Ok(
                    Command::Cancel
                    | Command::SetMode(_)
                    | Command::SetConfig { .. }
                    | Command::Steer { .. },
                ) => {}
                Ok(Command::Shutdown) | Err(async_channel::TryRecvError::Closed) => {
                    return Ok(SessionEnd::Shutdown);
                }
                Err(async_channel::TryRecvError::Empty) => break,
            }
        }
    }
    s.emit(AgentEvent::Ready(init.info.clone())).await;

    let root = s.workspace.root().to_path_buf();
    let policy = s.preset.modes.clone();
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
    let meta = s
        .preset
        .session_meta
        .as_ref()
        .and_then(|m| m.as_object().cloned());
    let mut started_modes: Option<Modes> = None;
    let mut session = None;
    let resume = s.resume.lock().unwrap().clone();
    if let Some(previous) = resume
        && init.load_session
    {
        // Routed before the request: the replay arrives as updates for this id.
        host.register(&previous, s);
        s.replaying.store(true, Ordering::Relaxed);
        let request =
            acp::LoadSessionRequest::new(previous.clone(), root.clone()).meta(meta.clone());
        let loaded = s
            .bounded(cx.send_request(request).block_task(), timeout)
            .await;
        s.replaying.store(false, Ordering::Relaxed);
        if let Some(Ok(response)) = loaded {
            let raw_current = response
                .modes
                .as_ref()
                .map(|m| m.current_mode_id.to_string());
            started_modes = modes(response.modes);
            if let (Some(m), Some(current)) = (started_modes.as_mut(), raw_current) {
                m.current = current;
            }
            s.emit(AgentEvent::SessionStarted {
                session_id: previous.to_string(),
                resumed: true,
                modes: started_modes.clone(),
            })
            .await;
            s.offer_configs(response.config_options.as_deref().unwrap_or_default())
                .await;
            session = Some(previous);
        } else {
            host.unregister(&previous);
            eprintln!("event=agent_session_load_failed agent={}", s.preset.id);
        }
    }
    let session = match session {
        Some(session) => session,
        None => loop {
            let request = acp::NewSessionRequest::new(root.clone())
                .mcp_servers(Vec::new())
                .meta(meta.clone());
            match s
                .bounded(cx.send_request(request).block_task(), timeout)
                .await
            {
                Some(Ok(response)) => {
                    let early = host.register(&response.session_id, s);
                    let raw_current = response
                        .modes
                        .as_ref()
                        .map(|m| m.current_mode_id.to_string());
                    started_modes = modes(response.modes);
                    if let (Some(m), Some(current)) = (started_modes.as_mut(), raw_current) {
                        m.current = current;
                    }
                    s.emit(AgentEvent::SessionStarted {
                        session_id: response.session_id.to_string(),
                        resumed: false,
                        modes: started_modes.clone(),
                    })
                    .await;
                    s.offer_configs(response.config_options.as_deref().unwrap_or_default())
                        .await;
                    // Updates the agent sent before its answer (e.g. the command list).
                    for notification in early {
                        handle_update(s, notification, cx).await;
                    }
                    break response.session_id;
                }
                Some(Err(e))
                    if e.code == acp::ErrorCode::AuthRequired && !init.auth_methods.is_empty() =>
                {
                    let wait = Login {
                        shared: s,
                        cx,
                        launch,
                        methods: &init.auth_methods,
                        host_env: &host.key.env,
                    };
                    if let Some(end) = wait.run(&mut first).await {
                        return Ok(end);
                    }
                }
                Some(Err(e)) => {
                    return fail(s, format!("「{name}」无法新建会话：{}", error_text(&e))).await;
                }
                None => return fail(s, format!("「{name}」新建会话超时")).await,
            }
        },
    };
    *s.resume.lock().unwrap() = Some(session.clone());
    if s.text_only
        && let Some(modes) = &started_modes
        && let Some(mode) = ["read-only", "plan"].into_iter().find(|id| {
            s.preset.modes.allows(id) && modes.available.iter().any(|(offered, _)| offered == id)
        })
    {
        *s.mode.lock().unwrap() = Some(mode.into());
    }
    if !enforce_mode(s, cx, &session, started_modes.as_ref()).await && s.text_only {
        return Ok(SessionEnd::Failed);
    }

    let can_login = !init.auth_methods.is_empty();
    let embedded = init.embedded_context;
    *s.image_support.lock().unwrap() = Some(init.info.image);
    s.steering_supported
        .store(init.steering && !s.text_only, Ordering::Relaxed);
    if let Some((turn, parts)) = first {
        start_prompt(s, cx, &session, turn, parts, can_login, embedded)?;
    }
    loop {
        let busy = s.turn.lock().unwrap().is_some();
        let resting = s.is_idle();
        let idle_timeout = s.idle_timeout;
        let idle = async move {
            if busy || resting {
                futures::future::pending::<()>().await;
            } else {
                Timer::after(idle_timeout).await;
            }
        };
        let command = s.commands.recv().fuse();
        let done = s.turn_done_rx.recv().fuse();
        let idle = idle.fuse();
        let closed = cx.incoming_closed().fuse();
        futures::pin_mut!(command, done, idle, closed);
        futures::select! {
            command = command => {
                s.idle.store(false, Ordering::Relaxed);
                match command {
                    Err(_) | Ok(Command::Shutdown) => {
                        close_session(s, cx, init, host, &session).await;
                        return Ok(SessionEnd::Shutdown);
                    }
                    Ok(Command::Connect | Command::Login(_)) => {}
                    Ok(Command::RetryLogin(env)) => {
                        if env != host.key.env {
                            close_session(s, cx, init, host, &session).await;
                            return Ok(SessionEnd::Move(env));
                        }
                    }
                    Ok(Command::Prompt { turn, parts }) => {
                        start_prompt(s, cx, &session, turn, parts, can_login, embedded)?
                    }
                    Ok(Command::Steer { turn, parts }) => {
                        steering::start(s, cx, &session, turn, parts, embedded)?;
                    }
                    Ok(Command::Cancel) => {
                        let waiting = {
                            let mut steering = s.steering.lock().unwrap();
                            steering.cancel_followup()
                        };
                        if waiting {
                            let turn = *s.turn.lock().unwrap();
                            s.end_turn(turn, TurnOutcome::Cancelled, true).await;
                        }
                        s.cancel_permissions();
                        cx.send_notification(acp::CancelNotification::new(session.clone()))?;
                        // An agent that never answers the prompt would keep the turn (and the
                        // idle timer) stuck; its late answer is ignored once the turn has ended.
                        let turn = *s.turn.lock().unwrap();
                        if let Some(turn) = turn {
                            let shared = s.clone();
                            cx.spawn(async move {
                                Timer::after(shared.cancel_grace).await;
                                if *shared.turn.lock().unwrap() == Some(turn) {
                                    eprintln!("event=agent_cancel_unanswered agent={}", shared.preset.id);
                                    shared
                                        .end_turn(
                                            Some(turn),
                                            TurnOutcome::Failed("Agent 没有响应取消，已结束这一轮".into()),
                                            true,
                                        )
                                        .await;
                                    let _ = shared.turn_done.send(()).await;
                                }
                                Ok(())
                            })?;
                        }
                    }
                    Ok(Command::SetConfig { id, value }) => {
                        let request = cx.send_request(acp::SetSessionConfigOptionRequest::new(session.clone(), id, value.as_str()));
                        let shared = s.clone();
                        cx.spawn(async move {
                            match request.block_task().await {
                                Ok(response) => shared.offer_configs(&response.config_options).await,
                                Err(e) => shared.emit(AgentEvent::Error { message: format!("切换失败：{}", error_text(&e)) }).await,
                            }
                            Ok(())
                        })?;
                    }
                    Ok(Command::SetMode(mode)) => {
                        let request = cx.send_request(acp::SetSessionModeRequest::new(session.clone(), mode.clone()));
                        let shared = s.clone();
                        cx.spawn(async move {
                            match request.block_task().await {
                                // The answer is the switch: an agent need not also send
                                // `current_mode_update` for a change the client asked for
                                // (Codex does not).
                                Ok(_) => {
                                    *shared.mode.lock().unwrap() = Some(mode.clone());
                                    shared.emit(AgentEvent::ModeChanged { mode_id: mode }).await
                                }
                                Err(e) => shared.emit(AgentEvent::Error { message: format!("切换模式失败：{}", error_text(&e)) }).await,
                            }
                            Ok(())
                        })?;
                    }
                }
            },
            _ = done => {
                let continuation = {
                    let mut steering = s.steering.lock().unwrap();
                    steering.continuation()
                };
                if let Some(parts) = continuation {
                    let turn = *s.turn.lock().unwrap();
                    if let Some(turn) = turn {
                        start_prompt(s, cx, &session, turn, parts, can_login, embedded)?;
                    }
                }
                let parked = s.parked.lock().unwrap().take();
                if let Some(prompt) = parked {
                    let mut pending = Some(prompt);
                    let wait = Login {
                        shared: s,
                        cx,
                        launch,
                        methods: &init.auth_methods,
                        host_env: &host.key.env,
                    };
                    if let Some(end) = wait.run(&mut pending).await {
                        if matches!(end, SessionEnd::Move(_)) {
                            close_session(s, cx, init, host, &session).await;
                        }
                        return Ok(end);
                    }
                    if let Some((turn, parts)) = pending {
                        start_prompt(s, cx, &session, turn, parts, can_login, embedded)?;
                    }
                }
            }
            _ = idle => {
                if init.close_session {
                    close_session(s, cx, init, host, &session).await;
                    return Ok(SessionEnd::Idle);
                }
                // The agent cannot free it: the process stops once every session rests.
                s.idle.store(true, Ordering::Relaxed);
                host.wake();
            }
            _ = closed => return Ok(SessionEnd::Closed),
        }
    }
}

/// Waiting for the user to sign in after `session/new` or `session/prompt` answered "auth
/// required".
struct Login<'a> {
    shared: &'a Arc<SessionState>,
    cx: &'a ConnectionTo<Agent>,
    launch: &'a ResolvedLaunch,
    methods: &'a [acp::AuthMethod],
    /// The variables of the process the session runs on.
    host_env: &'a BTreeMap<String, String>,
}

impl Login<'_> {
    /// `None`: signed in, try the session again in this process. A prompt sent meanwhile is
    /// kept in `first`.
    async fn run(&self, first: &mut Option<(TurnId, Vec<PromptPart>)>) -> Option<SessionEnd> {
        let shared = self.shared;
        let commands = &shared.commands;
        eprintln!("event=agent_auth_required agent={}", shared.preset.id);
        let methods = self
            .methods
            .iter()
            .map(|m| AuthChoice {
                id: m.id().to_string(),
                name: m.name().to_string(),
                description: m.description().map(str::to_string),
                terminal: matches!(m, acp::AuthMethod::Terminal(_)),
            })
            .collect();
        shared.emit(AgentEvent::AuthRequired { methods }).await;
        loop {
            let command = commands.recv().fuse();
            let idle = Timer::after(shared.idle_timeout).fuse();
            let closed = self.cx.incoming_closed().fuse();
            futures::pin_mut!(command, idle, closed);
            let command = futures::select! {
                command = command => command,
                _ = idle => {
                    shared
                        .finish_turn(None, TurnOutcome::Failed("等待登录超时".into()))
                        .await;
                    return Some(SessionEnd::Idle);
                }
                _ = closed => return Some(SessionEnd::Closed),
            };
            match command {
                Err(_) | Ok(Command::Shutdown) => return Some(SessionEnd::Shutdown),
                Ok(Command::Connect | Command::SetMode(_) | Command::SetConfig { .. }) => {}
                Ok(Command::Steer { turn, .. }) => {
                    steering::failed(shared, turn, "正在等待登录，补充指令未发送".into()).await;
                }
                Ok(Command::Prompt { turn, parts }) => {
                    first.get_or_insert((turn, parts));
                }
                Ok(Command::Cancel) => {
                    shared.finish_turn(None, TurnOutcome::Cancelled).await;
                    return Some(SessionEnd::Idle);
                }
                Ok(Command::RetryLogin(env)) if env != *self.host_env => {
                    *shared.pending_first.lock().unwrap() = first.take();
                    return Some(SessionEnd::Move(env));
                }
                Ok(Command::RetryLogin(_)) => return None,
                Ok(Command::Login(id)) => {
                    if let Some(end) = self.login(&id, commands, first).await {
                        return end;
                    }
                }
            }
        }
    }

    /// `Some(None)`: signed in, retry; `Some(Some(end))`: stop; `None`: keep waiting.
    async fn login(
        &self,
        id: &str,
        commands: &async_channel::Receiver<Command>,
        first: &mut Option<(TurnId, Vec<PromptPart>)>,
    ) -> Option<Option<SessionEnd>> {
        let shared = self.shared;
        let error = |message: String| shared.emit(AgentEvent::Error { message });
        let progress = |message: String| shared.emit(AgentEvent::Progress { message });
        let Some(method) = self.methods.iter().find(|m| m.id().to_string() == id) else {
            error(format!("没有这种登录方式：{id}")).await;
            return None;
        };
        eprintln!("event=agent_login agent={} method={id}", shared.preset.id);
        if let acp::AuthMethod::Terminal(terminal) = method {
            let keep: Vec<&str> = shared
                .preset
                .local_cli
                .iter()
                .map(|cli| cli.env.as_str())
                .collect();
            let script = login::script(
                self.launch,
                &terminal.args,
                &terminal.env,
                &keep,
                shared.workspace.root(),
            );
            match login::open_in_terminal(&script) {
                Ok(_) => {
                    progress(format!(
                        "已在「终端」里打开「{}」的登录，完成后点「已登录，重试」",
                        method.name()
                    ))
                    .await
                }
                Err(e) => error(format!("无法打开「终端」：{e}")).await,
            }
            return None;
        }
        progress(format!(
            "正在用「{}」登录，请按提示在浏览器里完成",
            method.name()
        ))
        .await;
        let request = self
            .cx
            .send_request(acp::AuthenticateRequest::new(id.to_string()))
            .block_task()
            .fuse();
        let timeout = Timer::after(LOGIN_TIMEOUT).fuse();
        futures::pin_mut!(request, timeout);
        loop {
            let command = commands.recv().fuse();
            futures::pin_mut!(command);
            futures::select! {
                result = request => {
                    return match result {
                        Ok(_) => Some(None),
                        Err(e) => {
                            error(format!("登录失败：{}", error_text(&e))).await;
                            None
                        }
                    };
                }
                _ = timeout => {
                    error("登录超时，请重试".into()).await;
                    return None;
                }
                command = command => match command {
                    Err(_) | Ok(Command::Shutdown) => return Some(Some(SessionEnd::Shutdown)),
                    Ok(Command::Cancel) => {
                        shared.finish_turn(None, TurnOutcome::Cancelled).await;
                        return Some(Some(SessionEnd::Idle));
                    }
                    Ok(Command::Prompt { turn, parts }) => {
                        first.get_or_insert((turn, parts));
                    }
                    Ok(_) => {}
                },
            }
        }
    }
}

/// `can_login`: the agent offered login methods, so an "auth required" answer parks the
/// prompt (see [`SessionState::parked`]) instead of failing the turn.
fn start_prompt(
    shared: &Arc<SessionState>,
    cx: &ConnectionTo<Agent>,
    session: &acp::SessionId,
    turn: TurnId,
    parts: Vec<PromptPart>,
    can_login: bool,
    embedded: bool,
) -> Result<(), sdk::Error> {
    let blocks = prompt_blocks(&parts, embedded);
    if *shared.image_support.lock().unwrap() == Some(false)
        && parts.iter().any(|p| matches!(p, PromptPart::Image { .. }))
    {
        let shared = shared.clone();
        cx.spawn(async move {
            shared
                .finish_turn(
                    Some(turn),
                    TurnOutcome::Failed(ClientError::ImagesUnavailable.to_string()),
                )
                .await;
            let _ = shared.turn_done.send(()).await;
            Ok(())
        })?;
        return Ok(());
    }
    let request = cx.send_request(acp::PromptRequest::new(session.clone(), blocks));
    let shared = shared.clone();
    cx.spawn(async move {
        let outcome = match request.block_task().await {
            Ok(response) => events::turn_outcome(&response.stop_reason),
            Err(e) if can_login && e.code == acp::ErrorCode::AuthRequired => {
                *shared.parked.lock().unwrap() = Some((turn, parts));
                let _ = shared.turn_done.send(()).await;
                return Ok(());
            }
            Err(e) => TurnOutcome::Failed(error_text(&e)),
        };
        shared.finish_turn(Some(turn), outcome).await;
        let _ = shared.turn_done.send(()).await;
        Ok(())
    })
}

/// Starts in the mode the session was last in (back after an idle close or a restart), else in
/// the preset's asking mode, whatever the agent's own settings chose; a forbidden current mode
/// with no allowed alternative is reported.
async fn enforce_mode(
    shared: &Arc<SessionState>,
    cx: &ConnectionTo<Agent>,
    session: &acp::SessionId,
    modes: Option<&Modes>,
) -> bool {
    let Some(modes) = modes else {
        return true;
    };
    let policy = &shared.preset.modes;
    let offered = |id: &str| modes.available.iter().any(|(mode, _)| mode == id);
    let chosen = shared
        .mode
        .lock()
        .unwrap()
        .clone()
        .filter(|mode| policy.allows(mode) && offered(mode));
    let target = match (chosen, &policy.initial) {
        (Some(chosen), _) => Some(chosen),
        (None, Some(initial)) if offered(initial) => Some(initial.clone()),
        _ if !policy.allows(&modes.current) => modes.available.first().map(|(id, _)| id.clone()),
        _ => None,
    };
    match target {
        Some(target) if target != modes.current => {
            let request = acp::SetSessionModeRequest::new(session.clone(), target.clone());
            match shared
                .bounded(
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
                    return false;
                }
            }
        }
        None if !policy.allows(&modes.current) => {
            shared
                .emit(AgentEvent::Error {
                    message: "Agent 处于跳过审批的模式，且没有可切换的询问模式".into(),
                })
                .await;
            return false;
        }
        _ => {}
    }
    true
}

pub(crate) async fn handle_update(
    shared: &Arc<SessionState>,
    notification: acp::SessionNotification,
    cx: &ConnectionTo<Agent>,
) {
    steering::status(shared, &notification.update).await;
    let Some(mut event) = events::from_update(&notification.update) else {
        return;
    };
    // Reviews diff against the file as it was before the first edit. An edit that carries
    // its diffs gives the text before even if the agent wrote the file first; otherwise the
    // paths an edit tool call announces are read before it runs. Replayed history
    // (`session/load`) is not about to edit anything.
    let edit = edit_update(&notification.update);
    if let Some(edit) = &edit
        && !shared.replaying.load(Ordering::Relaxed)
    {
        let mut paths = Vec::new();
        for (path, diffs) in &edit.diffs {
            if let Ok(path) = shared.workspace.resolve(path) {
                shared.snapshot_edit(&path, diffs).await;
                paths.push(path);
            }
        }
        if !paths.is_empty() {
            let mut edits = shared.edits.lock().unwrap();
            let known = edits.entry(edit.id.clone()).or_default();
            for path in paths {
                if !known.contains(&path) {
                    known.push(path);
                }
            }
        }
    }
    if let AgentEvent::ToolCall(call) = &event
        && !shared.replaying.load(Ordering::Relaxed)
        && matches!(
            call.kind,
            events::ToolKind::Edit | events::ToolKind::Delete | events::ToolKind::Move
        )
    {
        for location in &call.locations {
            if let Ok(path) = shared.workspace.resolve(&location.path) {
                shared.snapshot(&path).await;
            }
        }
    }
    if let AgentEvent::ConfigOptions(configs) = &event {
        *shared.configs.lock().unwrap() = configs.clone();
    }
    // The agent switched itself to a mode the preset forbids: it is switched straight back
    // (the UI keeps showing the allowed mode, which becomes true again once that succeeds).
    if let AgentEvent::ModeChanged { mode_id } = &event
        && !shared.preset.modes.allows(mode_id)
    {
        eprintln!("event=agent_mode_forbidden agent={}", shared.preset.id);
        let Some(fallback) = shared.preset.modes.initial.clone() else {
            event = AgentEvent::Error {
                message: format!(
                    "Agent 切换到了跳过审批的模式（{mode_id}），请在模式菜单里改回询问"
                ),
            };
            shared.emit(event).await;
            return;
        };
        shared
            .emit(AgentEvent::Error {
                message: format!("Agent 切换到了跳过审批的模式（{mode_id}），已切回 {fallback}"),
            })
            .await;
        let request = cx.send_request(acp::SetSessionModeRequest::new(
            notification.session_id.clone(),
            fallback.clone(),
        ));
        let shared = shared.clone();
        let spawned = cx.spawn(async move {
            match request.block_task().await {
                Ok(_) => {
                    shared
                        .emit(AgentEvent::ModeChanged { mode_id: fallback })
                        .await
                }
                Err(e) => {
                    shared
                        .emit(AgentEvent::Error {
                            message: format!(
                                "未能切回 {fallback}：{}，请在模式菜单里改回询问",
                                error_text(&e)
                            ),
                        })
                        .await
                }
            }
            Ok(())
        });
        if let Err(e) = spawned {
            eprintln!("event=agent_mode_restore_failed error={}", error_text(&e));
        }
        return;
    }
    if let AgentEvent::ModeChanged { mode_id } = &event {
        *shared.mode.lock().unwrap() = Some(mode_id.clone());
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
    // The edit is done: the app recounts, reloads open editors and reviews against the
    // snapshot (an agent writing through `fs/write_text_file` said so already; twice is fine).
    if let Some(edit) = edit
        && let Some(status) = edit.status
        && matches!(
            status,
            acp::ToolCallStatus::Completed | acp::ToolCallStatus::Failed
        )
    {
        let paths = shared.edits.lock().unwrap().remove(&edit.id);
        if status == acp::ToolCallStatus::Completed {
            for path in paths.into_iter().flatten() {
                if !shared.snapshots.lock().unwrap().contains_key(&path) {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    shared
                        .emit(AgentEvent::Error {
                            message: format!("没能确定 {name} 改动前的内容，它不能对比或还原"),
                        })
                        .await;
                }
                shared.emit(AgentEvent::FileWritten { path }).await;
            }
        }
    }
}

/// An edit's `(old, new)` texts for one file (`old` `None`: a new file).
type EditDiffs = Vec<(Option<String>, String)>;

/// A tool call (or update) as far as its file edits go.
struct EditUpdate {
    id: String,
    status: Option<acp::ToolCallStatus>,
    /// `(old, new)` texts by file, in the order the agent sent them.
    diffs: Vec<(PathBuf, EditDiffs)>,
}

fn edit_update(update: &acp::SessionUpdate) -> Option<EditUpdate> {
    let (id, status, content) = match update {
        acp::SessionUpdate::ToolCall(call) => (
            &call.tool_call_id,
            Some(call.status),
            Some(call.content.as_slice()),
        ),
        acp::SessionUpdate::ToolCallUpdate(update) => (
            &update.tool_call_id,
            update.fields.status,
            update.fields.content.as_deref(),
        ),
        _ => return None,
    };
    let mut diffs: Vec<(PathBuf, EditDiffs)> = Vec::new();
    for content in content.unwrap_or_default() {
        if let acp::ToolCallContent::Diff(diff) = content {
            let pair = (diff.old_text.clone(), diff.new_text.clone());
            match diffs.iter_mut().find(|(path, _)| *path == diff.path) {
                Some((_, pairs)) => pairs.push(pair),
                None => diffs.push((diff.path.clone(), vec![pair])),
            }
        }
    }
    Some(EditUpdate {
        id: id.to_string(),
        status,
        diffs,
    })
}

/// Holds the dispatch loop only to register and announce the request; the answer is awaited
/// in a spawned task.
pub(crate) async fn handle_permission(
    shared: &Arc<SessionState>,
    request: acp::RequestPermissionRequest,
    responder: Responder<acp::RequestPermissionResponse>,
    cx: &ConnectionTo<Agent>,
) -> Result<(), sdk::Error> {
    if shared.text_only {
        return responder.respond(acp::RequestPermissionResponse::new(
            acp::RequestPermissionOutcome::Cancelled,
        ));
    }
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

pub(crate) async fn handle_read(
    shared: &SessionState,
    request: &acp::ReadTextFileRequest,
) -> Result<acp::ReadTextFileResponse, sdk::Error> {
    if shared.text_only {
        return Err(sdk::Error::invalid_params().data("此会话仅生成文本，不允许读取文件"));
    }
    let path = shared.workspace.resolve(&request.path).map_err(fs_error)?;
    let text = match shared.buffer_text(&path).await.map_err(fs_error)? {
        Some(text) => text,
        None => read_disk(&path).map_err(fs_error)?,
    };
    Ok(acp::ReadTextFileResponse::new(window(
        text,
        request.line,
        request.limit,
    )))
}

pub(crate) async fn handle_write(
    shared: &SessionState,
    request: acp::WriteTextFileRequest,
) -> Result<acp::WriteTextFileResponse, sdk::Error> {
    if shared.text_only {
        return Err(sdk::Error::invalid_params().data("此会话仅生成文本，不允许修改文件"));
    }
    let path = shared.workspace.resolve(&request.path).map_err(fs_error)?;
    shared.snapshot(&path).await;
    write_atomic(&path, &request.content).map_err(fs_error)?;
    shared.emit(AgentEvent::FileWritten { path }).await;
    Ok(acp::WriteTextFileResponse::new())
}

pub(crate) type Transport =
    ByteStreams<Async<std::process::ChildStdin>, BoundedLines<Async<std::process::ChildStdout>>>;

pub(crate) fn transport(process: &mut AgentProcess) -> io::Result<Transport> {
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
pub(crate) fn error_text(e: &sdk::Error) -> String {
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

pub(crate) async fn with_timeout<T>(future: impl Future<Output = T>, limit: Duration) -> Option<T> {
    let future = std::pin::pin!(future);
    match futures::future::select(future, Timer::after(limit)).await {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
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
            PromptPart::Image { data, mime_type } => acp::ContentBlock::Image(
                acp::ImageContent::new(data.to_string(), mime_type.clone()),
            ),
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
