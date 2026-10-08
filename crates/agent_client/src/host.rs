//! One agent process shared by the sessions of one agent (same preset, same variables): they
//! run on one ACP connection, each with its own `cwd`, and requests from the agent are routed
//! by session id (ADR 0004). The process starts when a session needs it and stops when no
//! session is left on it (or, for an agent without `session/close`, when all of them are idle).

use crate::{
    client::{self, Init, SessionState},
    events::{AgentEvent, AgentInfo, ExitReason, TurnOutcome},
    fs::BufferProvider,
    login,
    process::{AgentProcess, describe},
    registry::{AgentPreset, ResolvedLaunch, SearchPath},
};
use agent_client_protocol::{
    self as sdk, Agent, Client, ConnectionTo, Responder,
    schema::{ProtocolVersion, v1 as acp},
};
use futures::FutureExt;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

/// Updates for a session id not known yet (they can arrive before the `session/new` answer):
/// kept for this many sessions, this many each.
const ORPHAN_SESSIONS: usize = 8;
const ORPHAN_UPDATES: usize = 64;

/// What makes two sessions able to share a process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostKey {
    pub preset: AgentPreset,
    pub env: BTreeMap<String, String>,
    pub search: SearchPath,
}

/// The agent processes of a whole app (all windows): sessions of the same agent with the same
/// variables share one. Without a pool, every client gets a process of its own.
#[derive(Default)]
pub struct AgentPool {
    hosts: Mutex<Vec<(HostKey, Weak<Host>)>>,
    stopping: AtomicBool,
}

impl AgentPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Permanently closes the pool, stops every process group and waits for cleanup.
    /// Clients may still be held by background work when the application quits.
    pub fn shutdown(&self) {
        let hosts: Vec<_> = {
            let mut hosts = self.hosts.lock().unwrap();
            self.stopping.store(true, Ordering::Relaxed);
            std::mem::take(&mut *hosts)
                .into_iter()
                .filter_map(|(_, host)| host.upgrade())
                .collect()
        };
        for host in hosts {
            host.shutdown();
        }
    }

    pub(crate) fn host(
        &self,
        key: HostKey,
        buffers: Option<Arc<dyn BufferProvider>>,
    ) -> io::Result<Arc<Host>> {
        let mut hosts = self.hosts.lock().unwrap();
        if self.stopping.load(Ordering::Relaxed) {
            return Err(io::Error::other("agent process pool is closed"));
        }
        hosts.retain(|(_, host)| host.strong_count() > 0);
        if let Some(host) = hosts
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, host)| host.upgrade())
        {
            return Ok(host);
        }
        let host = Host::spawn(key.clone(), buffers)?;
        hosts.push((key, Arc::downgrade(&host)));
        Ok(host)
    }
}

/// Owned by the clients using it; the last one to go stops the process.
pub(crate) struct Host {
    pub inner: Arc<HostInner>,
    stop: async_channel::Sender<()>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

pub(crate) struct HostInner {
    pub key: HostKey,
    pub buffers: Option<Arc<dyn BufferProvider>>,
    attach_tx: async_channel::Sender<Arc<SessionState>>,
    attach_rx: async_channel::Receiver<Arc<SessionState>>,
    /// A session left or went idle: the process may be done.
    wake_tx: async_channel::Sender<()>,
    wake_rx: async_channel::Receiver<()>,
    pub(crate) stopping: async_channel::Receiver<()>,
    routes: Mutex<HashMap<String, Arc<SessionState>>>,
    orphans: Mutex<VecDeque<(String, Vec<acp::SessionNotification>)>>,
    /// Sessions running on the current process.
    members: Mutex<Vec<Arc<SessionState>>>,
    /// Every session that ran on the current process: told when it stops.
    ran: Mutex<Vec<Arc<SessionState>>>,
    pub pid: Mutex<Option<u32>>,
}

impl Host {
    pub fn spawn(key: HostKey, buffers: Option<Arc<dyn BufferProvider>>) -> io::Result<Arc<Self>> {
        let (attach_tx, attach_rx) = async_channel::unbounded();
        let (wake_tx, wake_rx) = async_channel::unbounded();
        let (stop, stopping) = async_channel::bounded(1);
        let inner = Arc::new(HostInner {
            key,
            buffers,
            attach_tx,
            attach_rx,
            wake_tx,
            wake_rx,
            stopping,
            routes: Mutex::new(HashMap::new()),
            orphans: Mutex::new(VecDeque::new()),
            members: Mutex::new(Vec::new()),
            ran: Mutex::new(Vec::new()),
            pid: Mutex::new(None),
        });
        let main = inner.clone();
        let thread = thread::Builder::new()
            .name(format!("agent-{}", inner.key.preset.id))
            .spawn(move || async_io::block_on(host_main(main)))?;
        Ok(Arc::new(Self {
            inner,
            stop,
            thread: Mutex::new(Some(thread)),
        }))
    }
    fn shutdown(&self) {
        self.stop.close();
        self.inner.attach_tx.close();
        if let Some(handle) = self.thread.lock().unwrap().take()
            // A session moving to another process can let go of the last handle from this
            // process's own thread, which then ends by itself.
            && handle.thread().id() != thread::current().id()
        {
            let _ = handle.join();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl HostInner {
    /// Asks for `session` to run on this process (started if needed).
    pub fn attach(&self, session: Arc<SessionState>) -> bool {
        self.attach_tx.try_send(session).is_ok()
    }

    pub fn route(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.routes.lock().unwrap().get(session_id).cloned()
    }

    /// Routes the session's requests and hands over updates that arrived before it was known.
    pub fn register(
        &self,
        session_id: &acp::SessionId,
        session: &Arc<SessionState>,
    ) -> Vec<acp::SessionNotification> {
        let id = session_id.to_string();
        self.routes
            .lock()
            .unwrap()
            .insert(id.clone(), session.clone());
        let mut orphans = self.orphans.lock().unwrap();
        match orphans.iter().position(|(orphan, _)| *orphan == id) {
            Some(at) => orphans
                .remove(at)
                .map(|(_, updates)| updates)
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }

    pub fn unregister(&self, session_id: &acp::SessionId) {
        self.routes.lock().unwrap().remove(&session_id.to_string());
    }

    fn keep_orphan(&self, notification: acp::SessionNotification) {
        let id = notification.session_id.to_string();
        let mut orphans = self.orphans.lock().unwrap();
        match orphans.iter_mut().find(|(orphan, _)| *orphan == id) {
            Some((_, updates)) if updates.len() < ORPHAN_UPDATES => updates.push(notification),
            Some(_) => {}
            None => {
                if orphans.len() >= ORPHAN_SESSIONS {
                    orphans.pop_front();
                }
                orphans.push_back((id, vec![notification]));
            }
        }
    }

    /// The session's task ended (closed, idle, failed or moved).
    pub fn leave(&self, session: &Arc<SessionState>) {
        self.members
            .lock()
            .unwrap()
            .retain(|member| !Arc::ptr_eq(member, session));
        self.routes
            .lock()
            .unwrap()
            .retain(|_, routed| !Arc::ptr_eq(routed, session));
        let _ = self.wake_tx.try_send(());
    }

    pub fn has_members(&self) -> bool {
        !self.members.lock().unwrap().is_empty()
    }

    pub fn wake(&self) {
        let _ = self.wake_tx.try_send(());
    }

    /// No session needs the process any more.
    fn done(&self) -> bool {
        self.members
            .lock()
            .unwrap()
            .iter()
            .all(|member| member.is_idle())
    }
}

enum End {
    Idle,
    Shutdown,
    /// The agent closed stdout (exited or crashed).
    Closed,
    /// Launch or handshake failed; already reported.
    Failed,
}

async fn host_main(inner: Arc<HostInner>) {
    loop {
        let first = {
            let attach = std::pin::pin!(inner.attach_rx.recv());
            let stopping = std::pin::pin!(inner.stopping.recv());
            match futures::future::select(attach, stopping).await {
                futures::future::Either::Left((Ok(session), _)) => session,
                _ => return,
            }
        };
        let mut queue = vec![first];
        while let Ok(session) = inner.attach_rx.try_recv() {
            queue.push(session);
        }
        queue.retain(|session| {
            let live = !session.is_stopped();
            if !live {
                session.set_running(false);
            }
            live
        });
        if queue.is_empty() {
            continue;
        }
        if let End::Shutdown = run_process(&inner, queue).await {
            return;
        }
    }
}

/// Every session waiting for the process: the reason it did not start.
async fn fail_all(queue: &[Arc<SessionState>], message: String) {
    for session in queue {
        session.abandon();
        session
            .emit(AgentEvent::Error {
                message: message.clone(),
            })
            .await;
        session
            .finish_turn(None, TurnOutcome::Failed(message.clone()))
            .await;
        session.set_running(false);
    }
}

async fn run_process(inner: &Arc<HostInner>, queue: Vec<Arc<SessionState>>) -> End {
    let key = &inner.key;
    let name = key.preset.display_name.clone();
    let launch = match key.preset.resolve(&key.search, &key.env) {
        Ok(launch) => launch,
        Err(e) => {
            eprintln!("event=agent_launch_unavailable agent={}", key.preset.id);
            fail_all(&queue, e.to_string()).await;
            return End::Failed;
        }
    };
    // Each session gives the agent its own `cwd`; the process itself sits in the home folder.
    let cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_dir())
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut process = match AgentProcess::spawn(&launch, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("event=agent_spawn_failed agent={}", key.preset.id);
            fail_all(&queue, format!("启动「{name}」失败：{e}")).await;
            return End::Failed;
        }
    };
    let pid = process.pid();
    *inner.pid.lock().unwrap() = Some(pid);
    eprintln!("event=agent_spawn agent={} pid={pid}", key.preset.id);
    for session in &queue {
        session.joined();
        session.emit(AgentEvent::Starting { pid }).await;
    }
    // Told when the process stops, even if the handshake fails before they run.
    inner.ran.lock().unwrap().extend(queue.iter().cloned());
    let launch = Arc::new(launch);
    let end = match client::transport(&mut process) {
        Ok(transport) => match connect(inner.clone(), transport, launch, queue).await {
            Ok(end) => end,
            Err(e) => {
                eprintln!("event=agent_connection_error agent={}", key.preset.id);
                // A plain EOF is reported by the exit event (with the stderr tail).
                if !sdk::is_incoming_transport_closed(&e) {
                    let ran = inner.ran.lock().unwrap().clone();
                    for session in &ran {
                        session
                            .emit(AgentEvent::Error {
                                message: format!(
                                    "与「{name}」的连接出错：{}",
                                    client::error_text(&e)
                                ),
                            })
                            .await;
                    }
                }
                End::Closed
            }
        },
        Err(e) => {
            fail_all(&queue, format!("无法连接「{name}」的标准输入输出：{e}")).await;
            End::Failed
        }
    };

    inner.routes.lock().unwrap().clear();
    inner.orphans.lock().unwrap().clear();
    inner.members.lock().unwrap().clear();
    *inner.pid.lock().unwrap() = None;
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
        key.preset.id,
        match reason {
            ExitReason::Idle => "idle",
            ExitReason::Shutdown => "shutdown",
            ExitReason::SetupFailed { .. } => "setup_failed",
            ExitReason::Crashed { .. } => "crashed",
        }
    );
    let ran = std::mem::take(&mut *inner.ran.lock().unwrap());
    for session in &ran {
        session.process_gone(&reason, &name).await;
    }
    end
}

/// What the agent said about itself, shared by the sessions on this process.
fn init_from(response: &acp::InitializeResponse) -> Init {
    let caps = &response.agent_capabilities;
    Init {
        info: AgentInfo {
            name: response.agent_info.as_ref().map(|i| i.name.clone()),
            title: response.agent_info.as_ref().and_then(|i| i.title.clone()),
            version: response.agent_info.as_ref().map(|i| i.version.clone()),
            load_session: caps.load_session,
            embedded_context: caps.prompt_capabilities.embedded_context,
            image: caps.prompt_capabilities.image,
            auth_methods: response
                .auth_methods
                .iter()
                .map(|m| m.id().to_string())
                .collect(),
        },
        load_session: caps.load_session,
        close_session: caps.session_capabilities.close.is_some(),
        embedded_context: caps.prompt_capabilities.embedded_context,
        steering: response
            .meta
            .as_ref()
            .and_then(|meta| meta.get("steering"))
            .and_then(|steering| steering.get("supported"))
            .and_then(serde_json::Value::as_bool)
            == Some(true),
        auth_methods: response.auth_methods.clone(),
    }
}

async fn connect(
    inner: Arc<HostInner>,
    transport: client::Transport,
    launch: Arc<ResolvedLaunch>,
    queue: Vec<Arc<SessionState>>,
) -> Result<End, sdk::Error> {
    let on_update = inner.clone();
    let on_permission = inner.clone();
    let on_read = inner.clone();
    let on_write = inner.clone();
    Client
        .builder()
        .name("zj")
        .on_receive_notification(
            async move |notification: acp::SessionNotification, cx: ConnectionTo<Agent>| {
                match on_update.route(&notification.session_id.to_string()) {
                    Some(session) => client::handle_update(&session, notification, &cx).await,
                    None => on_update.keep_orphan(notification),
                }
                Ok(())
            },
            sdk::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: acp::RequestPermissionRequest,
                        responder: Responder<acp::RequestPermissionResponse>,
                        cx: ConnectionTo<Agent>| {
                match on_permission.route(&request.session_id.to_string()) {
                    Some(session) => {
                        client::handle_permission(&session, request, responder, &cx).await
                    }
                    None => responder.respond(acp::RequestPermissionResponse::new(
                        acp::RequestPermissionOutcome::Cancelled,
                    )),
                }
            },
            sdk::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::ReadTextFileRequest,
                        responder: Responder<acp::ReadTextFileResponse>,
                        _cx: ConnectionTo<Agent>| {
                let result = match on_read.route(&request.session_id.to_string()) {
                    Some(session) => client::handle_read(&session, &request).await,
                    None => Err(unknown_session()),
                };
                responder.respond_with_result(result)
            },
            sdk::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::WriteTextFileRequest,
                        responder: Responder<acp::WriteTextFileResponse>,
                        _cx: ConnectionTo<Agent>| {
                let result = match on_write.route(&request.session_id.to_string()) {
                    Some(session) => client::handle_write(&session, request).await,
                    None => Err(unknown_session()),
                };
                responder.respond_with_result(result)
            },
            sdk::on_receive_request!(),
        )
        .connect_with(transport, async move |cx: ConnectionTo<Agent>| {
            serve(inner, cx, launch, queue).await
        })
        .await
}

fn unknown_session() -> sdk::Error {
    sdk::Error::invalid_params().data("ZJ 没有这个会话")
}

/// The handshake, then sessions joining and leaving until none needs the process.
async fn serve(
    inner: Arc<HostInner>,
    cx: ConnectionTo<Agent>,
    launch: Arc<ResolvedLaunch>,
    queue: Vec<Arc<SessionState>>,
) -> Result<End, sdk::Error> {
    let name = inner.key.preset.display_name.clone();
    let timeout = queue
        .first()
        .map_or(Duration::from_secs(180), |s| s.handshake_timeout());
    let capabilities = acp::ClientCapabilities::new()
        .fs(acp::FileSystemCapabilities::new()
            .read_text_file(true)
            .write_text_file(true))
        // Not advertised in v1: agents run commands with their own tools, behind permission
        // requests, instead of in an editor-owned terminal.
        .terminal(false)
        .auth(acp::AuthCapabilities::new().terminal(login::SUPPORTED));
    let request = acp::InitializeRequest::new(ProtocolVersion::V1)
        .client_capabilities(capabilities)
        .client_info(acp::Implementation::new("zj", env!("CARGO_PKG_VERSION")).title("ZJ"));
    let request = std::pin::pin!(cx.send_request(request).block_task());
    let stopping = std::pin::pin!(inner.stopping.recv());
    let answer =
        match client::with_timeout(futures::future::select(request, stopping), timeout).await {
            Some(futures::future::Either::Left((answer, _))) => Some(Some(answer)),
            Some(futures::future::Either::Right(_)) => Some(None),
            None => None,
        };
    let init = match answer {
        Some(Some(Ok(init))) => Arc::new(init_from(&init)),
        Some(Some(Err(e))) => {
            eprintln!("event=agent_handshake_failed agent={}", inner.key.preset.id);
            fail_all(
                &queue,
                format!("「{name}」握手失败：{}", client::error_text(&e)),
            )
            .await;
            return Ok(End::Failed);
        }
        Some(None) => return Ok(End::Shutdown),
        None => {
            eprintln!("event=agent_handshake_failed agent={}", inner.key.preset.id);
            fail_all(
                &queue,
                format!("「{name}」在 {} 秒内没有响应握手", timeout.as_secs()),
            )
            .await;
            return Ok(End::Failed);
        }
    };
    let join = |session: Arc<SessionState>| {
        if session.is_stopped() {
            session.set_running(false);
            return;
        }
        session.joined();
        inner.members.lock().unwrap().push(session.clone());
        let mut ran = inner.ran.lock().unwrap();
        if !ran.iter().any(|known| Arc::ptr_eq(known, &session)) {
            ran.push(session.clone());
        }
        drop(ran);
        let task = client::run_session(
            session,
            cx.clone(),
            init.clone(),
            launch.clone(),
            inner.clone(),
        );
        if let Err(e) = cx.spawn(task) {
            eprintln!(
                "event=agent_session_spawn_failed error={}",
                client::error_text(&e)
            );
        }
    };
    for session in queue {
        join(session);
    }
    loop {
        let attach = std::pin::pin!(inner.attach_rx.recv());
        let wake = std::pin::pin!(inner.wake_rx.recv());
        let closed = std::pin::pin!(cx.incoming_closed());
        let stopping = std::pin::pin!(inner.stopping.recv());
        let woke = futures::future::select(attach, wake);
        let ended = futures::future::select(closed, stopping);
        match futures::future::select(woke, ended).await {
            futures::future::Either::Left((futures::future::Either::Left((attached, _)), _)) => {
                match attached {
                    Ok(session) => join(session),
                    Err(_) => return Ok(End::Shutdown),
                }
            }
            futures::future::Either::Left((futures::future::Either::Right(_), _)) => {
                // Sessions also leave when the agent's output ended: that is a crash.
                if cx.incoming_closed().now_or_never().is_some() {
                    return Ok(End::Closed);
                }
                if inner.done() && inner.attach_rx.is_empty() {
                    return Ok(End::Idle);
                }
            }
            futures::future::Either::Right((futures::future::Either::Left(_), _)) => {
                return Ok(End::Closed);
            }
            futures::future::Either::Right((futures::future::Either::Right(_), _)) => {
                return Ok(End::Shutdown);
            }
        }
    }
}
