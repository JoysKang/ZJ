//! Agent panel state and logic: live sessions (one ACP client each, started on the first
//! prompt), the event pump, history persistence, permission rules, the composer's context
//! and the reviews of what an agent changed. Rendering is in `agent_panel.rs`,
//! `agent_history.rs` and `agent_review.rs`.
//!
//! Resource rules: nothing runs while the panel is closed and no session is active; the
//! spinner's timer only ticks while the panel is visible in the focused window and a session
//! runs; threads keep at most `thread::MAX_ITEMS` items in memory and older messages come
//! back from the history database.

use super::*;
use crate::agent_model::{self, Attachment};
use crate::markdown::{self, Block};
use gpui_kit::component::input::InputState;
use workspace_editor_agent::{
    AgentClient, AgentEvent, AgentPreset, ClientOptions, PermissionKind, WriteMode,
    builtin_presets,
    review::{line_counts, resolve_file, review_texts},
    thread::{self as agent_thread, FileChange, Item, PermissionState, Record, Thread},
};
use workspace_editor_agent_history::{
    History, NewSession, Role as HistoryRole, SessionId, SessionStatus, SessionSummary,
};

#[cfg(test)]
#[path = "agent_ui_tests.rs"]
mod ui_tests;

gpui_kit::actions!(
    agent,
    [
        ToggleAgentPanel,
        SearchSessions,
        AddSelectionToAgent,
        NewAgentSession,
        NextApproval
    ]
);

/// Messages loaded when a stored session is opened, and per "加载更早的消息".
const HISTORY_PAGE: usize = 200;
/// Spinner frame interval (8 frames per turn, about one turn a second).
const SPIN_FRAME: Duration = Duration::from_millis(125);
pub(super) const SPIN_FRAMES: usize = 8;

/// The history database, shared by every window. Opening it is lazy (first use, off the UI
/// thread).
pub struct AgentStore {
    pub history: Option<Arc<History>>,
}

impl Global for AgentStore {}

pub fn init_store(cx: &mut App) {
    let path = std::env::var_os("ZJ_AGENT_DB")
        .map(PathBuf::from)
        .or_else(workspace_editor_agent_history::default_db_path);
    cx.set_global(AgentStore {
        history: path.map(|path| Arc::new(History::new(path))),
    });
}

fn history(cx: &App) -> Option<Arc<History>> {
    cx.try_global::<AgentStore>()
        .and_then(|store| store.history.clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentView {
    Thread,
    History,
    Settings,
}

/// A session in this window. The client starts with the first prompt.
pub(super) struct LiveSession {
    pub key: u64,
    pub preset: AgentPreset,
    pub client: Option<Arc<AgentClient>>,
    starting: bool,
    pub thread: Thread,
    pub db: Option<SessionId>,
    db_creating: bool,
    /// Records waiting for the database row.
    pending: Vec<(HistoryRole, String)>,
    pub started_at: i64,
    pub branch: Option<String>,
    /// Parsed replies by absolute item index (`thread.dropped + i`).
    pub md: HashMap<usize, Rc<Vec<Block>>>,
    /// Oldest stored message loaded (history threads page backwards from it).
    pub oldest_seq: Option<i64>,
    resume: Option<String>,
    stored_status: Option<SessionStatus>,
    pub turns: usize,
    pump: Option<Task<()>>,
    stats_task: Option<Task<()>>,
    highlight_task: Option<Task<()>>,
    /// The prompt typed while the agent was starting.
    queued: Option<(String, Vec<Attachment>)>,
}

impl LiveSession {
    fn new(key: u64, preset: AgentPreset) -> Self {
        Self {
            key,
            preset,
            client: None,
            starting: false,
            thread: Thread::new(),
            db: None,
            db_creating: false,
            pending: Vec::new(),
            started_at: workspace_editor_agent_history::now_ms(),
            branch: None,
            md: HashMap::new(),
            oldest_seq: None,
            resume: None,
            stored_status: None,
            turns: 0,
            pump: None,
            stats_task: None,
            highlight_task: None,
            queued: None,
        }
    }

    pub fn title(&self) -> String {
        self.thread.title.clone().unwrap_or_else(|| {
            self.thread
                .items
                .iter()
                .find_map(|item| match item {
                    Item::User { text, .. } => {
                        Some(workspace_editor_agent_history::placeholder_title(text))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| "新会话".into())
        })
    }

    pub fn row_status(&self) -> agent_model::RowStatus {
        let status = if self.starting && self.thread.status == agent_thread::Status::Idle {
            agent_thread::Status::Running
        } else {
            self.thread.status
        };
        agent_model::RowStatus::of_thread(status, self.thread.unread)
    }

    pub fn busy(&self) -> bool {
        self.starting
            || matches!(
                self.thread.status,
                agent_thread::Status::Running | agent_thread::Status::Awaiting
            )
    }
}

/// The `@` file picker under the composer.
pub(super) struct Mention {
    pub range: std::ops::Range<usize>,
    pub results: Vec<PathBuf>,
    pub selected: usize,
    generation: u64,
    _task: Option<Task<()>>,
}

/// Session list filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ListFilter {
    pub all_workspaces: bool,
    pub agent: Option<String>,
    pub running: bool,
    pub pending: bool,
    pub archived: bool,
}

pub(super) struct HistoryList {
    pub rows: Vec<SessionSummary>,
    pub grouped: Vec<agent_model::ListRow>,
    /// (this workspace, all) session counts; archived count.
    pub counts: (usize, usize, usize),
    pub filter: ListFilter,
    pub error: Option<String>,
    pub loaded: bool,
    pub list: ListState,
    generation: u64,
    task: Option<Task<()>>,
    pub renaming: Option<(SessionId, Entity<InputState>, Subscription)>,
}

pub(super) struct AgentPanel {
    pub visible: bool,
    pub width: Pixels,
    pub view: AgentView,
    pub sessions: Vec<LiveSession>,
    pub current: Option<u64>,
    next_key: u64,
    pub composer: Entity<TextareaState>,
    pub attachments: Vec<Attachment>,
    pub mention: Option<Mention>,
    mention_generation: u64,
    pub history: HistoryList,
    pub search: Option<super::agent_search::SessionSearch>,
    pub thread_list: ListState,
    list_shape: (u64, usize, usize, bool),
    pub spin: usize,
    spin_task: Option<Task<()>>,
    width_task: Option<Task<()>>,
    pub expanded_tools: HashSet<(u64, String)>,
    pub expanded_thoughts: HashSet<(u64, usize)>,
    pub collapsed_plans: HashSet<u64>,
    pub changes_collapsed: bool,
    pub strip_collapsed: bool,
    pub switcher_open: bool,
    pub presets: Vec<AgentPreset>,
    /// The agent picked for the next new session.
    pub agent_id: String,
    pub composer_focused: bool,
    /// Sending in a window without a folder asks for one; the prompt goes out once it is open.
    pub send_after_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl AgentPanel {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Prototype>) -> Self {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .submit_on_enter(true)
                .placeholder("继续追问，@ 引用文件")
        });
        let events = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Prototype, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                } => this.agent_submit(window, cx),
                InputEvent::Change => this.agent_composer_changed(window, cx),
                InputEvent::Focus => {
                    this.agent.composer_focused = true;
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.agent.composer_focused = false;
                    cx.notify();
                }
                _ => {}
            },
        );
        let mut presets = builtin_presets();
        presets.extend(
            settings
                .custom
                .iter()
                .cloned()
                .map(|agent| agent.into_preset()),
        );
        let thread_list = ListState::new(0, ListAlignment::Top, theme::AGENT_THREAD_MAX);
        thread_list.set_follow_mode(FollowMode::Tail);
        Self {
            visible: settings.panel_visible,
            width: px(settings.panel_width),
            view: AgentView::Thread,
            sessions: Vec::new(),
            current: None,
            next_key: 1,
            composer,
            attachments: Vec::new(),
            mention: None,
            mention_generation: 0,
            history: HistoryList {
                rows: Vec::new(),
                grouped: Vec::new(),
                counts: (0, 0, 0),
                filter: ListFilter::default(),
                error: None,
                loaded: false,
                list: ListState::new(0, ListAlignment::Top, theme::AGENT_THREAD_MAX),
                generation: 0,
                task: None,
                renaming: None,
            },
            search: None,
            thread_list,
            list_shape: (0, 0, 0, false),
            spin: 0,
            spin_task: None,
            width_task: None,
            expanded_tools: HashSet::new(),
            expanded_thoughts: HashSet::new(),
            collapsed_plans: HashSet::new(),
            changes_collapsed: false,
            strip_collapsed: false,
            switcher_open: false,
            agent_id: settings.default_agent.clone(),
            presets,
            composer_focused: false,
            send_after_open: false,
            _subscriptions: vec![events],
        }
    }

    pub fn session(&self, key: u64) -> Option<&LiveSession> {
        self.sessions.iter().find(|s| s.key == key)
    }

    pub fn session_mut(&mut self, key: u64) -> Option<&mut LiveSession> {
        self.sessions.iter_mut().find(|s| s.key == key)
    }

    pub fn current(&self) -> Option<&LiveSession> {
        self.current.and_then(|key| self.session(key))
    }

    pub fn preset(&self, id: &str) -> Option<&AgentPreset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// Sessions running, waiting for approval or finished unread (direction B's strip).
    pub fn active_sessions(&self) -> impl Iterator<Item = &LiveSession> {
        self.sessions.iter().filter(|s| s.busy() || s.thread.unread)
    }

    pub fn compact(&self) -> bool {
        self.width < theme::AGENT_COMPACT_WIDTH
    }

    /// Thread rows: an optional "older messages" row, then one per item.
    pub fn older_row(&self) -> bool {
        self.current()
            .is_some_and(|s| s.thread.dropped > 0 || s.oldest_seq.is_some_and(|seq| seq > 1))
    }
}

/// The agent name, as the session list and cards show it.
pub(super) fn agent_name(presets: &[AgentPreset], id: &str) -> String {
    presets
        .iter()
        .find(|p| p.id == id)
        .map(|p| p.display_name.clone())
        .unwrap_or_else(|| id.to_string())
}

fn glyph_of(presets: &[AgentPreset], id: &str) -> workspace_editor_agent::Glyph {
    presets
        .iter()
        .find(|p| p.id == id)
        .map_or(workspace_editor_agent::Glyph::Generic, |p| p.glyph)
}

pub(super) fn glyph_for(presets: &[AgentPreset], id: &str) -> workspace_editor_agent::Glyph {
    glyph_of(presets, id)
}

fn record_role(record: &Record) -> (HistoryRole, String) {
    match record {
        Record::Agent(text) => (HistoryRole::Agent, text.clone()),
        Record::Tool(title) => (HistoryRole::Tool, title.clone()),
    }
}

impl Prototype {
    // ----- panel visibility, width, views -------------------------------------------------

    pub(super) fn toggle_agent_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible = !self.agent.visible;
        self.set_agent_panel(visible, window, cx);
    }

    pub(super) fn set_agent_panel(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent.visible != visible {
            self.agent.visible = visible;
            self.change_settings(window, cx, |s| s.agent.panel_visible = visible);
        }
        if visible {
            if self.agent.current.is_none() {
                self.agent_new_session(None, window, cx);
            }
            self.agent_mark_read(cx);
            if self.agent.view == AgentView::History {
                self.agent_reload_history(window, cx);
            }
            self.agent_focus_composer(window, cx);
        } else {
            self.focus_active_editor(window, cx);
        }
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    pub(super) fn agent_focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.view == AgentView::Thread {
            self.agent
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
    }

    /// The panel was dragged; the width is saved once the drag settles.
    pub(super) fn agent_resized(
        &mut self,
        width: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (f32::from(width) - f32::from(self.agent.width)).abs() < 0.5 {
            return;
        }
        self.agent.width = width;
        self.agent.width_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let width = f32::from(this.agent.width).clamp(
                    crate::settings::AGENT_PANEL_WIDTH_MIN,
                    crate::settings::AGENT_PANEL_WIDTH_MAX,
                );
                this.change_settings(window, cx, |s| s.agent.panel_width = width);
            });
        }));
        cx.notify();
    }

    pub(super) fn agent_show(
        &mut self,
        view: AgentView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent.view = view;
        self.agent.switcher_open = false;
        if view == AgentView::History {
            self.agent_reload_history(window, cx);
        }
        if view == AgentView::Thread {
            self.agent_mark_read(cx);
            self.agent_focus_composer(window, cx);
        } else if self
            .agent
            .composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            // The composer goes away with the thread; keep the shortcuts working.
            self.focus_handle.focus(window, cx);
        }
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    pub(super) fn agent_select(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.session(key).is_none() {
            return;
        }
        self.agent.current = Some(key);
        self.agent.attachments.clear();
        self.agent.mention = None;
        self.agent_sync_list(true);
        self.agent_show(AgentView::Thread, window, cx);
    }

    fn agent_mark_read(&mut self, cx: &mut Context<Self>) {
        if !self.agent.visible || self.agent.view != AgentView::Thread {
            return;
        }
        if let Some(key) = self.agent.current
            && let Some(session) = self.agent.session_mut(key)
            && session.thread.unread
        {
            session.thread.unread = false;
            cx.notify();
        }
    }

    /// The panel opened with the window: an empty session for the default agent, without
    /// taking the focus from the editor.
    pub(super) fn agent_ensure_session(&mut self) {
        if self.agent.current.is_some() {
            return;
        }
        let id = self.agent.agent_id.clone();
        let Some(preset) = self
            .agent
            .preset(&id)
            .or_else(|| self.agent.presets.first())
            .cloned()
        else {
            return;
        };
        let key = self.agent.next_key;
        self.agent.next_key += 1;
        let mut session = LiveSession::new(key, preset);
        session.branch = self.active_branch();
        self.agent.sessions.push(session);
        self.agent.current = Some(key);
        self.agent_sync_list(true);
    }

    /// A new, empty session (its process starts with the first prompt). Reuses the current
    /// session when it has no messages yet, switching its agent if needed.
    pub(super) fn agent_new_session(
        &mut self,
        agent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = agent.unwrap_or_else(|| self.agent.agent_id.clone());
        let Some(preset) = self
            .agent
            .preset(&id)
            .or_else(|| self.agent.presets.first())
            .cloned()
        else {
            return;
        };
        self.agent.agent_id = preset.id.clone();
        if let Some(key) = self.agent.current
            && let Some(session) = self.agent.session_mut(key)
            && session.thread.items.is_empty()
            && session.db.is_none()
            && !session.starting
        {
            if session.preset.id != preset.id {
                session.preset = preset;
                session.client = None;
                session.pump = None;
            }
        } else {
            let key = self.agent.next_key;
            self.agent.next_key += 1;
            let mut session = LiveSession::new(key, preset);
            session.branch = self.active_branch();
            self.agent.sessions.push(session);
            self.agent.current = Some(key);
        }
        self.agent.attachments.clear();
        self.agent_sync_list(true);
        self.agent_show(AgentView::Thread, window, cx);
    }

    // ----- sending ------------------------------------------------------------------------

    pub(super) fn agent_submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.mention.is_some() {
            self.agent_pick_mention(None, window, cx);
            return;
        }
        let text = self.agent.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(root) = self.root.clone() else {
            self.agent.send_after_open = true;
            self.choose_path(true, window, cx);
            return;
        };
        if self.agent.current.is_none() {
            self.agent_new_session(None, window, cx);
        }
        let Some(key) = self.agent.current else {
            return;
        };
        let attachments = std::mem::take(&mut self.agent.attachments);
        let busy = self.agent.session(key).is_some_and(LiveSession::busy);
        if busy {
            self.message = "Agent 正在回复：等这一轮结束，或先点停止".into();
            self.agent.attachments = attachments;
            cx.notify();
            return;
        }
        self.agent
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        let has_client = self.agent.session(key).is_some_and(|s| s.client.is_some());
        if has_client {
            self.agent_prompt(key, text, attachments, window, cx);
        } else {
            if let Some(session) = self.agent.session_mut(key) {
                session.queued = Some((text, attachments));
            }
            self.agent_start_client(key, root, window, cx);
        }
        cx.notify();
    }

    /// Resolves the environment (keychain lookups run `security`) and starts the client off
    /// the UI thread, then sends the queued prompt.
    fn agent_start_client(
        &mut self,
        key: u64,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        if session.starting {
            return;
        }
        session.starting = true;
        let preset = session.preset.clone();
        let resume = session.resume.clone();
        let overrides = settings.env.get(&preset.id).cloned().unwrap_or_default();
        let write_mode = if settings.accept_first {
            WriteMode::AcceptFirst
        } else {
            WriteMode::Direct
        };
        let idle = Duration::from_secs(u64::from(settings.idle_minutes) * 60);
        let buffers = buffer_provider(cx);
        let job = cx.background_spawn(async move {
            let env = crate::secrets::resolve(&overrides, |name| std::env::var(name).ok())?;
            let mut options = ClientOptions::new(preset, root);
            options.write_mode = write_mode;
            options.idle_timeout = idle;
            options.env_overrides = env;
            options.resume_session = resume;
            options.buffers = Some(buffers);
            AgentClient::start(options).map_err(|e| format!("无法启动 Agent：{e}"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(session) = this.agent.session_mut(key) else {
                    return;
                };
                session.starting = false;
                let queued = session.queued.take();
                match result {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let events = client.events();
                        session.client = Some(client);
                        session.pump = Some(cx.spawn_in(window, async move |this, cx| {
                            while let Ok(first) = events.recv().await {
                                let mut batch = vec![first];
                                while batch.len() < 256 {
                                    match events.try_recv() {
                                        Ok(event) => batch.push(event),
                                        Err(_) => break,
                                    }
                                }
                                if this
                                    .update_in(cx, |this, window, cx| {
                                        this.agent_events(key, batch, window, cx)
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }));
                        if let Some((text, attachments)) = queued {
                            this.agent_prompt(key, text, attachments, window, cx);
                        }
                    }
                    Err(error) => {
                        eprintln!("event=agent_start_failed agent={}", session.preset.id);
                        if let Some((text, attachments)) = queued {
                            let labels = attachments.iter().map(Attachment::label).collect();
                            session.thread.push_user(text, labels, 0);
                            session.thread.apply(
                                &AgentEvent::TurnEnded {
                                    turn: 0,
                                    outcome: workspace_editor_agent::TurnOutcome::Failed(error),
                                },
                                true,
                            );
                        }
                        this.agent_sync_list(false);
                    }
                }
                this.agent_update_spin(window, cx);
                cx.notify();
            });
        })
        .detach();
        self.agent_update_spin(window, cx);
    }

    fn agent_prompt(
        &mut self,
        key: u64,
        text: String,
        attachments: Vec<Attachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        let branch = self.active_branch();
        let workspace_name = self.workspace_name();
        let store = history(cx);
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let Some(client) = session.client.clone() else {
            return;
        };
        let parts = agent_model::prompt_parts(&text, &attachments);
        match client.prompt(parts) {
            Ok(turn) => {
                let labels: Vec<String> = attachments.iter().map(Attachment::label).collect();
                let stored = if labels.is_empty() {
                    text.clone()
                } else {
                    format!(
                        "{}\n{text}",
                        attachments
                            .iter()
                            .map(|a| format!("@{}", a.path().display()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                session.thread.push_user(text.clone(), labels, turn);
                session.turns += 1;
                if session.branch.is_none() {
                    session.branch = branch.clone();
                }
                eprintln!(
                    "event=agent_prompt agent={} turn={turn} attachments={}",
                    session.preset.id,
                    attachments.len()
                );
                // Persist: the first prompt creates the database row.
                if let Some(store) = store {
                    match session.db {
                        Some(id) => {
                            background_history(cx, store, move |h| {
                                h.append_message(id, HistoryRole::User, stored);
                                for attachment in attachments {
                                    h.touch_file(id, attachment.path().display().to_string());
                                }
                            });
                        }
                        None => {
                            session.pending.push((HistoryRole::User, stored));
                            if !session.db_creating {
                                session.db_creating = true;
                                let new = NewSession {
                                    workspace_root: root.clone().unwrap_or_default(),
                                    workspace_name: Some(workspace_name),
                                    agent_id: session.preset.id.clone(),
                                    acp_session_id: session.thread.session_id.clone(),
                                    title: session.thread.title.clone(),
                                    first_prompt: Some(text),
                                    repo: root
                                        .as_ref()
                                        .and_then(|r| r.file_name())
                                        .map(|n| n.to_string_lossy().into_owned()),
                                    branch,
                                    created_at: Some(session.started_at),
                                };
                                let job = cx.background_spawn({
                                    let store = store.clone();
                                    async move { store.create_session(new) }
                                });
                                cx.spawn_in(window, async move |this, cx| {
                                    let created = job.await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.agent_session_created(key, created, cx)
                                    });
                                })
                                .detach();
                            }
                        }
                    }
                }
            }
            Err(error) => {
                self.message = error.to_string();
            }
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    fn agent_session_created(
        &mut self,
        key: u64,
        created: workspace_editor_agent_history::Result<SessionId>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = history(cx) else { return };
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        session.db_creating = false;
        match created {
            Ok(id) => {
                session.db = Some(id);
                let pending = std::mem::take(&mut session.pending);
                let status = agent_model::stored_status(session.thread.status, session.turns > 0);
                session.stored_status = Some(status);
                let title = session.thread.title.clone();
                let acp = session.thread.session_id.clone();
                let files: Vec<String> = session
                    .thread
                    .changed_files
                    .keys()
                    .map(|p| p.display().to_string())
                    .collect();
                background_history(cx, store, move |h| {
                    for (role, text) in pending {
                        h.append_message(id, role, text);
                    }
                    h.set_status(id, status);
                    if let Some(title) = title {
                        h.set_auto_title(id, title);
                    }
                    if acp.is_some() {
                        h.set_acp_session_id(id, acp);
                    }
                    for file in files {
                        h.touch_file(id, file);
                    }
                });
            }
            Err(error) => {
                eprintln!("event=agent_history_create_failed");
                session.pending.clear();
                self.message = format!("会话未能写入历史：{error}");
            }
        }
        cx.notify();
    }

    pub(super) fn agent_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.agent.current.and_then(|key| self.agent.session(key))
            && let Some(client) = &session.client
        {
            client.cancel();
        }
        cx.notify();
    }

    // ----- events -------------------------------------------------------------------------

    fn agent_events(
        &mut self,
        key: u64,
        batch: Vec<AgentEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.agent.visible
            && self.agent.current == Some(key)
            && self.agent.view == AgentView::Thread
            && window.is_window_active();
        let rules: Vec<String> = self
            .root
            .as_ref()
            .and_then(|root| {
                cx.global::<crate::settings::Settings>()
                    .agent
                    .allow
                    .get(&root.to_string_lossy().into_owned())
                    .cloned()
            })
            .unwrap_or_default();
        let store = history(cx);
        let mut written = Vec::new();
        let mut recount = false;
        let mut turn_ended = false;
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let before = session.thread.items.len() + session.thread.dropped;
        let mut touched: Vec<String> = Vec::new();
        let mut title = None;
        let mut acp_id = None;
        for event in &batch {
            session.thread.apply(event, visible);
            match event {
                AgentEvent::PermissionRequested(request) => {
                    if agent_thread::rule_matches(request, &rules)
                        && let Some(option) = request
                            .options
                            .iter()
                            .find(|o| o.kind == PermissionKind::AllowOnce)
                        && let Some(client) = &session.client
                        && client.respond_permission(request.id, Some(option.id.clone()))
                    {
                        let command = agent_thread::permission_command(request).unwrap_or_default();
                        eprintln!("event=agent_permission_rule agent={}", session.preset.id);
                        session.thread.answer_permission(
                            request.id,
                            PermissionState::Rule(agent_thread::command_prefix(&command)),
                        );
                    } else {
                        eprintln!("event=agent_permission_asked agent={}", session.preset.id);
                    }
                }
                AgentEvent::FileWritten { path } => {
                    written.push(path.clone());
                    touched.push(path.display().to_string());
                    recount = true;
                }
                AgentEvent::EditProposed { path } => {
                    touched.push(path.display().to_string());
                    recount = true;
                }
                AgentEvent::ToolCall(call) => {
                    touched.extend(call.locations.iter().map(|l| l.path.display().to_string()));
                }
                AgentEvent::TitleChanged { title: Some(t) } => title = Some(t.clone()),
                AgentEvent::SessionStarted { session_id, .. } => {
                    acp_id = Some(session_id.clone());
                    session.resume = Some(session_id.clone());
                }
                AgentEvent::TurnEnded { .. } => {
                    turn_ended = true;
                    recount = true;
                }
                _ => {}
            }
        }
        // Persist what the thread produced.
        let records: Vec<(HistoryRole, String)> = session
            .thread
            .take_records()
            .iter()
            .map(record_role)
            .collect();
        let status = agent_model::stored_status(session.thread.status, session.turns > 0);
        let status_changed = session.stored_status != Some(status);
        match (session.db, store) {
            (Some(id), Some(store)) => {
                if status_changed {
                    session.stored_status = Some(status);
                }
                if !records.is_empty()
                    || status_changed
                    || !touched.is_empty()
                    || title.is_some()
                    || acp_id.is_some()
                {
                    background_history(cx, store, move |h| {
                        for (role, text) in records {
                            h.append_message(id, role, text);
                        }
                        if status_changed {
                            h.set_status(id, status);
                        }
                        for path in touched {
                            h.touch_file(id, path);
                        }
                        if let Some(title) = title {
                            h.set_auto_title(id, title);
                        }
                        if acp_id.is_some() {
                            h.set_acp_session_id(id, acp_id);
                        }
                    });
                }
            }
            _ => session.pending.extend(records),
        }
        self.agent_parse_from(key, before.saturating_sub(1), cx);
        if recount {
            self.agent_recount(key, window, cx);
        }
        for path in written {
            self.reload_document_from_disk(&path, window, cx);
        }
        if turn_ended {
            self.agent_reload_review(key, window, cx);
            if self.agent.view == AgentView::History {
                self.agent_reload_history(window, cx);
            }
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    /// Parses agent replies from absolute item index `from` on; finished replies are then
    /// highlighted in the background.
    fn agent_parse_from(&mut self, key: u64, from: usize, cx: &mut Context<Self>) {
        let theme = gpui_kit::component::Theme::global(cx)
            .highlight_theme
            .clone();
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let dropped = session.thread.dropped;
        session.md.retain(|&index, _| index >= dropped);
        let mut finished: Vec<(usize, Vec<Block>)> = Vec::new();
        for (i, item) in session.thread.items.iter().enumerate() {
            let index = dropped + i;
            if index < from {
                continue;
            }
            if let Item::Agent { text, streaming } = item {
                let blocks = markdown::parse(text);
                if !streaming
                    && blocks.iter().any(|b| {
                        matches!(
                            b,
                            Block::Code {
                                language: Some(_),
                                ..
                            }
                        )
                    })
                {
                    finished.push((index, blocks.clone()));
                }
                session.md.insert(index, Rc::new(blocks));
            }
        }
        if finished.is_empty() {
            return;
        }
        let job = cx.background_spawn(async move {
            for (_, blocks) in &mut finished {
                markdown::highlight(blocks, &theme);
            }
            finished
        });
        // Several batches may finish replies; each task only fills in its own indexes.
        let task = cx.spawn(async move |this, cx| {
            let highlighted = job.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this.agent.session_mut(key) {
                    for (index, blocks) in highlighted {
                        if session.md.contains_key(&index) {
                            session.md.insert(index, Rc::new(blocks));
                        }
                    }
                    cx.notify();
                }
            });
        });
        if let Some(session) = self.agent.session_mut(key) {
            match session.highlight_task.take() {
                Some(previous) => {
                    previous.detach();
                    session.highlight_task = Some(task);
                }
                None => session.highlight_task = Some(task),
            }
        }
    }

    /// Re-highlights every reply (the appearance changed).
    pub(super) fn agent_rehighlight(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<u64> = self.agent.sessions.iter().map(|s| s.key).collect();
        for key in keys {
            self.agent_parse_from(key, 0, cx);
        }
    }

    /// Keeps the thread list's item count in step with the current thread.
    pub(super) fn agent_sync_list(&mut self, switched: bool) {
        let Some(session) = self.agent.current() else {
            self.agent.thread_list.reset(0);
            self.agent.list_shape = (0, 0, 0, false);
            return;
        };
        let older = self.agent.older_row();
        let shape = (
            session.key,
            session.thread.dropped,
            session.thread.items.len(),
            older,
        );
        let count = shape.2 + usize::from(older);
        let old = self.agent.list_shape;
        if switched || old.0 != shape.0 || old.1 != shape.1 || old.3 != shape.3 || shape.2 < old.2 {
            self.agent.thread_list.reset(count);
            if switched {
                self.agent.thread_list.set_follow_mode(FollowMode::Tail);
            }
        } else if shape.2 > old.2 {
            let at = old.2 + usize::from(older);
            self.agent.thread_list.splice(at..at, shape.2 - old.2);
            self.agent.thread_list.remeasure();
        } else {
            self.agent.thread_list.remeasure();
        }
        self.agent.list_shape = shape;
    }

    /// Ticks the spinner only while it can be seen.
    pub(super) fn agent_update_spin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let spinning = self.agent.visible
            && window.is_window_active()
            && self
                .agent
                .sessions
                .iter()
                .any(|s| s.starting || s.thread.status == agent_thread::Status::Running);
        if !spinning {
            self.agent.spin_task = None;
            return;
        }
        if self.agent.spin_task.is_some() {
            return;
        }
        self.agent.spin_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SPIN_FRAME).await;
                if this
                    .update(cx, |this, cx| {
                        this.agent.spin = (this.agent.spin + 1) % SPIN_FRAMES;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    // ----- login ---------------------------------------------------------------------------

    pub(super) fn agent_login(&mut self, key: u64, method: &str, cx: &mut Context<Self>) {
        if let Some(client) = self.agent.session_mut(key).and_then(|s| s.client.as_ref()) {
            client.login(method);
        }
        cx.notify();
    }

    pub(super) fn agent_retry_login(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(client) = self.agent.session_mut(key).and_then(|s| s.client.as_ref()) {
            client.retry_login();
        }
        cx.notify();
    }

    // ----- permissions --------------------------------------------------------------------

    pub(super) fn agent_answer(
        &mut self,
        key: u64,
        request: workspace_editor_agent::PermissionId,
        choice: PermissionChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let Some(card) = session
            .thread
            .pending_permissions()
            .find(|card| card.request.id == request)
            .cloned()
        else {
            return;
        };
        let options = &card.request.options;
        let allow = options
            .iter()
            .find(|o| o.kind == PermissionKind::AllowOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|o| o.kind == PermissionKind::AllowAlways)
            });
        let reject = options
            .iter()
            .find(|o| o.kind == PermissionKind::RejectOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|o| o.kind == PermissionKind::RejectAlways)
            });
        let (option, state) = match &choice {
            PermissionChoice::Once => (
                allow.map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::AllowOnce, "已允许一次".into()),
            ),
            PermissionChoice::Always(prefix) => (
                allow.map(|o| o.id.clone()),
                PermissionState::Answered(
                    PermissionKind::AllowAlways,
                    format!("已始终允许 {prefix}"),
                ),
            ),
            PermissionChoice::Reject => (
                reject.map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::RejectOnce, "已拒绝".into()),
            ),
        };
        if let Some(client) = &session.client {
            client.respond_permission(request, option);
        }
        session.thread.answer_permission(request, state);
        eprintln!(
            "event=agent_permission_answered agent={} choice={}",
            session.preset.id,
            match choice {
                PermissionChoice::Once => "once",
                PermissionChoice::Always(_) => "always",
                PermissionChoice::Reject => "reject",
            }
        );
        if let (PermissionChoice::Always(prefix), Some(root)) = (choice, root) {
            let root = root.to_string_lossy().into_owned();
            self.change_settings(window, cx, move |s| {
                let rules = s.agent.allow.entry(root).or_default();
                if !rules.contains(&prefix) {
                    rules.push(prefix);
                }
            });
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    /// ⌘⇧A: the next session waiting for approval.
    pub(super) fn agent_next_approval(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.agent.current;
        let waiting: Vec<u64> = self
            .agent
            .sessions
            .iter()
            .filter(|s| s.thread.status == agent_thread::Status::Awaiting)
            .map(|s| s.key)
            .collect();
        let next = waiting
            .iter()
            .find(|&&k| Some(k) > current)
            .or_else(|| waiting.first())
            .copied();
        if let Some(key) = next {
            if !self.agent.visible {
                self.set_agent_panel(true, window, cx);
            }
            self.agent_select(key, window, cx);
        } else {
            self.message = "没有待批准的请求".into();
            cx.notify();
        }
    }

    pub(super) fn agent_remove_rule(
        &mut self,
        rule: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.as_ref().map(|r| r.to_string_lossy().into_owned()) else {
            return;
        };
        self.change_settings(window, cx, move |s| {
            if let Some(rules) = s.agent.allow.get_mut(&root) {
                rules.retain(|r| *r != rule);
            }
        });
        cx.notify();
    }

    // ----- modes and settings -------------------------------------------------------------

    pub(super) fn agent_set_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        if let Some(session) = self.agent.current.and_then(|k| self.agent.session(k))
            && let Some(client) = &session.client
            && !client.set_mode(mode)
        {
            self.message = "这个模式会跳过审批，ZJ 不允许切换到它".into();
        }
        cx.notify();
    }

    pub(super) fn agent_set_write_mode(
        &mut self,
        accept_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_settings(window, cx, move |s| s.agent.accept_first = accept_first);
        let mode = if accept_first {
            WriteMode::AcceptFirst
        } else {
            WriteMode::Direct
        };
        for session in &self.agent.sessions {
            if let Some(client) = &session.client {
                client.set_write_mode(mode);
            }
        }
        cx.notify();
    }

    /// Another window (or the settings file) changed agent settings.
    pub(super) fn agent_follow_settings(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let mode = if settings.accept_first {
            WriteMode::AcceptFirst
        } else {
            WriteMode::Direct
        };
        for session in &self.agent.sessions {
            if let Some(client) = &session.client
                && client.write_mode() != mode
            {
                client.set_write_mode(mode);
            }
        }
        let mut presets = builtin_presets();
        presets.extend(settings.custom.iter().cloned().map(|a| a.into_preset()));
        if presets != self.agent.presets {
            self.agent.presets = presets;
            cx.notify();
        }
    }

    // ----- composer context ---------------------------------------------------------------

    /// ⌘L: the editor's selection (or the whole file) becomes a chip and the composer gets
    /// the focus.
    pub(super) fn agent_add_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let attachment = match self.active {
            Pane::Document(id) => self.documents.iter().find(|d| d.id == id).map(|doc| {
                let editor = doc.editor.read(cx);
                let range = editor.selected_range();
                if range.is_empty() {
                    return Attachment::File(doc.path.clone());
                }
                use gpui_kit::component::input::RopeExt;
                let rope = editor.text();
                let start = rope.offset_to_position(range.start);
                let mut end = rope.offset_to_position(range.end);
                // A selection ending at the start of a line does not include that line.
                if end.character == 0 && end.line > start.line {
                    end.line -= 1;
                }
                Attachment::Selection {
                    path: doc.path.clone(),
                    start: start.line + 1,
                    end: end.line + 1,
                    text: rope.slice(range).to_string(),
                }
            }),
            _ => None,
        };
        if !self.agent.visible {
            self.set_agent_panel(true, window, cx);
        }
        if self.agent.view != AgentView::Thread {
            self.agent_show(AgentView::Thread, window, cx);
        }
        if let Some(attachment) = attachment
            && !self.agent.attachments.contains(&attachment)
        {
            self.agent.attachments.push(attachment);
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(super) fn agent_remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.agent.attachments.len() {
            self.agent.attachments.remove(index);
        }
        cx.notify();
    }

    pub(super) fn agent_composer_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let composer = self.agent.composer.read(cx);
            (composer.value().to_string(), composer.cursor())
        };
        let Some((range, query)) = agent_model::mention_at(&text, cursor) else {
            if self.agent.mention.take().is_some() {
                cx.notify();
            }
            return;
        };
        self.agent.mention_generation += 1;
        let generation = self.agent.mention_generation;
        let index = self.index.clone();
        let show_hidden = self.show_hidden;
        let recent: Vec<PathBuf> = self
            .documents
            .iter()
            .rev()
            .map(|d| d.path.clone())
            .collect();
        let task = cx.spawn_in(window, async move |this, cx| {
            let results = match index {
                Some(index) if !query.is_empty() => {
                    cx.background_spawn(async move { index.search(&query, show_hidden).paths })
                        .await
                }
                _ => recent,
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(mention) = this.agent.mention.as_mut()
                    && mention.generation == generation
                {
                    mention.results = results;
                    mention.results.truncate(50);
                    mention.selected = 0;
                    cx.notify();
                }
            });
        });
        let results = self
            .agent
            .mention
            .take()
            .map(|m| m.results)
            .unwrap_or_default();
        self.agent.mention = Some(Mention {
            range,
            results,
            selected: 0,
            generation,
            _task: Some(task),
        });
        cx.notify();
    }

    pub(super) fn agent_move_mention(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(mention) = self.agent.mention.as_mut()
            && !mention.results.is_empty()
        {
            let last = mention.results.len() - 1;
            mention.selected = mention.selected.saturating_add_signed(delta).min(last);
            cx.notify();
        }
    }

    /// Replaces `@query` with a file chip.
    pub(super) fn agent_pick_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mention) = self.agent.mention.take() else {
            return;
        };
        let Some(path) = mention
            .results
            .get(index.unwrap_or(mention.selected))
            .cloned()
        else {
            cx.notify();
            return;
        };
        let text = self.agent.composer.read(cx).value().to_string();
        if mention.range.end <= text.len() {
            let mut next = String::with_capacity(text.len());
            next.push_str(&text[..mention.range.start]);
            next.push_str(&text[mention.range.end..]);
            self.agent
                .composer
                .update(cx, |composer, cx| composer.set_value(next, window, cx));
        }
        let attachment = Attachment::File(path);
        if !self.agent.attachments.contains(&attachment) {
            self.agent.attachments.push(attachment);
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(super) fn agent_close_mention(&mut self, cx: &mut Context<Self>) {
        if self.agent.mention.take().is_some() {
            cx.notify();
        }
    }

    // ----- changed files ------------------------------------------------------------------

    /// Recomputes each changed file's line counts against its review base, in the background.
    pub(super) fn agent_recount(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let store = history(cx);
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let Some(client) = session.client.clone() else {
            return;
        };
        let mut paths: Vec<PathBuf> = session.thread.changed_files.keys().cloned().collect();
        paths.extend(client.snapshot_paths());
        paths.extend(client.shadow().pending_paths());
        paths.sort();
        paths.dedup();
        let db = session.db;
        let job = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let change = review_texts(&client, &path).map(|(before, after, origin)| {
                        let (added, removed) = line_counts(before.as_deref().unwrap_or(""), &after);
                        FileChange {
                            origin,
                            added,
                            removed,
                            new_file: before.is_none(),
                        }
                    });
                    (path, change.filter(|c| c.added + c.removed > 0))
                })
                .collect::<Vec<_>>()
        });
        session.stats_task = Some(cx.spawn_in(window, async move |this, cx| {
            let changes = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(session) = this.agent.session_mut(key) else {
                    return;
                };
                for (path, change) in changes {
                    session.thread.set_file_change(path, change);
                }
                let (added, removed) = session
                    .thread
                    .changed_files
                    .values()
                    .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
                if let (Some(id), Some(store)) = (db, store) {
                    background_history(cx, store, move |h| {
                        h.set_line_counts(id, added as i64, removed as i64)
                    });
                }
                cx.notify();
            });
        }));
    }

    /// Accepts or rejects every changed file of the current session.
    pub(super) fn agent_resolve_all(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.agent.current else {
            return;
        };
        let paths: Vec<PathBuf> = self
            .agent
            .session(key)
            .map(|s| s.thread.changed_files.keys().cloned().collect())
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        if accept {
            self.agent_resolve_files(key, paths, true, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("拒绝 Agent 对 {} 个文件的全部修改？", paths.len()),
            Some("已写入磁盘的修改会被还原成 Agent 改动之前的内容；尚未接受的建议会被丢弃。"),
            &["取消", "全部拒绝"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.agent_resolve_files(key, paths, false, window, cx)
                });
            }
        })
        .detach();
    }

    pub(super) fn agent_resolve_files(
        &mut self,
        key: u64,
        paths: Vec<PathBuf>,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.agent.session(key).and_then(|s| s.client.clone()) else {
            return;
        };
        let job = cx.background_spawn({
            let paths = paths.clone();
            async move {
                let mut errors = Vec::new();
                for path in &paths {
                    if let Err(e) = resolve_file(&client, path, accept) {
                        errors.push(e);
                    }
                }
                errors
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let errors = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(first) = errors.first() {
                    this.message = first.clone();
                }
                for path in &paths {
                    this.reload_document_from_disk(path, window, cx);
                }
                this.agent_recount(key, window, cx);
                this.agent_reload_review(key, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// An open editor follows a file the agent (or a review) changed on disk: silently when it
    /// has no edits, or when its edits are what the agent read; otherwise the banner asks.
    pub(super) fn reload_document_from_disk(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.documents.iter().find(|d| d.path == path).map(|d| d.id) {
            self.follow_agent_write(id, window, cx);
        }
    }

    // ----- history ------------------------------------------------------------------------

    pub(super) fn agent_reload_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = history(cx) else {
            self.agent.history.error = Some("没有可用的数据目录，会话历史不会保存".into());
            return;
        };
        let filter = self.agent.history.filter.clone();
        let root = self.root.clone();
        self.agent.history.generation += 1;
        let generation = self.agent.history.generation;
        let job = cx.background_spawn(async move {
            use workspace_editor_agent_history::{Archived, Filter, Scope};
            let scope = match (&root, filter.all_workspaces) {
                (Some(root), false) => Scope::Workspace(root.clone()),
                _ => Scope::All,
            };
            let base = Filter {
                agent_id: filter.agent.clone(),
                archived: if filter.archived {
                    Archived::Only
                } else {
                    Archived::Exclude
                },
                ..Default::default()
            };
            let rows = store.list(&scope, &base, 2000)?;
            let count = |scope: &Scope, archived: Archived| {
                store
                    .list(
                        scope,
                        &Filter {
                            archived,
                            ..Default::default()
                        },
                        100_000,
                    )
                    .map(|r| r.len())
                    .unwrap_or(0)
            };
            let here = match &root {
                Some(root) => count(&Scope::Workspace(root.clone()), Archived::Exclude),
                None => 0,
            };
            let all = count(&Scope::All, Archived::Exclude);
            let archived = count(&scope, Archived::Only);
            Ok::<_, workspace_editor_agent_history::Error>((rows, (here, all, archived)))
        });
        self.agent.history.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.agent.history.generation != generation {
                    return;
                }
                match result {
                    Ok((rows, counts)) => {
                        this.agent.history.rows = rows;
                        this.agent.history.counts = counts;
                        this.agent.history.error = None;
                    }
                    Err(error) => this.agent.history.error = Some(error.to_string()),
                }
                this.agent.history.loaded = true;
                this.agent_regroup_history();
                cx.notify();
            });
        }));
    }

    /// Applies the live filters (进行中 / 待处理) and groups the rows.
    pub(super) fn agent_regroup_history(&mut self) {
        let now = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now);
        let filter = self.agent.history.filter.clone();
        let live: HashMap<SessionId, agent_model::RowStatus> = self
            .agent
            .sessions
            .iter()
            .filter_map(|s| Some((s.db?, s.row_status())))
            .collect();
        let keep = |s: &SessionSummary| {
            let status = live
                .get(&s.id)
                .copied()
                .unwrap_or(agent_model::RowStatus::None);
            (!filter.running || status == agent_model::RowStatus::Running)
                && (!filter.pending
                    || matches!(
                        status,
                        agent_model::RowStatus::Awaiting | agent_model::RowStatus::Unread
                    ))
        };
        let visible: Vec<SessionSummary> = self
            .agent
            .history
            .rows
            .iter()
            .filter(|s| keep(s))
            .cloned()
            .collect();
        self.agent.history.grouped = agent_model::group_rows(&visible, now, offset);
        self.agent.history.rows = visible
            .into_iter()
            .chain(self.agent.history.rows.iter().filter(|s| !keep(s)).cloned())
            .collect();
        self.agent
            .history
            .list
            .reset(self.agent.history.grouped.len());
    }

    pub(super) fn agent_set_filter(
        &mut self,
        change: impl FnOnce(&mut ListFilter),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.agent.history.filter.clone();
        change(&mut self.agent.history.filter);
        let after = &self.agent.history.filter;
        if before.all_workspaces != after.all_workspaces
            || before.agent != after.agent
            || before.archived != after.archived
        {
            self.agent_reload_history(window, cx);
        } else {
            self.agent_regroup_history();
        }
        cx.notify();
    }

    /// Opens a stored session: switches to it if it is live in this window, otherwise loads
    /// its last messages (the agent restarts with `session/load` on the next prompt).
    pub(super) fn agent_open_stored(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self
            .agent
            .sessions
            .iter()
            .find(|s| s.db == Some(id))
            .map(|s| s.key)
        {
            self.agent_select(key, window, cx);
            return;
        }
        let Some(store) = history(cx) else { return };
        let job = cx.background_spawn(async move {
            let summary = store
                .session(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "会话已不存在".to_string())?;
            let count = store.message_count(id).map_err(|e| e.to_string())?;
            let page = store
                .messages_page(id, None, HISTORY_PAGE)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((summary, count, page))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, count, page)) => this.agent_restore(summary, count, page, window, cx),
                Err(error) => {
                    this.message = format!("无法打开会话：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn agent_restore(
        &mut self,
        summary: SessionSummary,
        count: usize,
        page: Vec<workspace_editor_agent_history::Message>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preset = self
            .agent
            .preset(&summary.agent_id)
            .cloned()
            .or_else(|| AgentPreset::find_builtin(&summary.agent_id));
        let Some(preset) = preset else {
            self.message = format!("找不到 Agent「{}」，无法继续这个会话", summary.agent_id);
            cx.notify();
            return;
        };
        let key = self.agent.next_key;
        self.agent.next_key += 1;
        let mut session = LiveSession::new(key, preset);
        session.db = Some(summary.id);
        session.started_at = summary.created_at;
        session.branch = summary.branch.clone();
        session.resume = summary.acp_session_id.clone();
        session.thread.session_id = summary.acp_session_id.clone();
        session.thread.title = Some(summary.title.clone());
        session.oldest_seq = page.first().map(|m| m.seq);
        session.turns = page.iter().filter(|m| m.role == HistoryRole::User).count();
        session.stored_status = Some(summary.status);
        let items = agent_model::items_from_messages(&page);
        session.thread.load(items, count.saturating_sub(page.len()));
        // `load` counts older rows as dropped; the history row says how to get them.
        session.thread.dropped = 0;
        self.agent.sessions.push(session);
        self.agent.current = Some(key);
        self.agent_parse_from(key, 0, cx);
        self.agent_sync_list(true);
        self.agent_show(AgentView::Thread, window, cx);
    }

    /// "加载更早的消息": the previous page from the database.
    pub(super) fn agent_load_older(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.agent.current else {
            return;
        };
        let Some(store) = history(cx) else { return };
        let Some(session) = self.agent.session(key) else {
            return;
        };
        let (Some(id), Some(before)) = (session.db, session.oldest_seq) else {
            // A live thread longer than the memory cap: reopen it from history.
            if let Some(id) = session.db {
                self.agent.sessions.retain(|s| s.key != key || s.busy());
                if self.agent.session(key).is_none() {
                    self.agent.current = None;
                    self.agent_open_stored(id, window, cx);
                }
            }
            return;
        };
        // Older pages never push the newest items out of memory.
        let room = agent_thread::MAX_ITEMS.saturating_sub(session.thread.items.len());
        if room == 0 {
            self.message = "这个会话在内存里的消息已到上限；更早的内容可在 ⌘J 搜索里找到".into();
            cx.notify();
            return;
        }
        let limit = HISTORY_PAGE.min(room);
        let job = cx.background_spawn(async move { store.messages_page(id, Some(before), limit) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(page) = job.await else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this.agent.session_mut(key) {
                    session.oldest_seq = page.first().map(|m| m.seq).or(Some(1));
                    session
                        .thread
                        .prepend(agent_model::items_from_messages(&page));
                    session.thread.dropped = 0;
                    session.md.clear();
                }
                this.agent_parse_from(key, 0, cx);
                this.agent_sync_list(true);
                this.agent.thread_list.set_follow_mode(FollowMode::Normal);
                this.agent.thread_list.scroll_to(ListOffset {
                    item_ix: page.len(),
                    offset_in_item: Pixels::ZERO,
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn agent_history_op(
        &mut self,
        op: HistoryOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = history(cx) else { return };
        if let HistoryOp::Delete(id, title) = &op {
            let id = *id;
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("永久删除会话「{title}」？"),
                Some("会话的消息、文件记录和搜索索引都会从本机删除，无法恢复。"),
                &["取消", "删除"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if answer.await != Ok(1) {
                    return;
                }
                let job = cx.background_spawn(async move { store.delete_session(id) });
                let result = job.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok(_) => {
                            for session in
                                this.agent.sessions.iter_mut().filter(|s| s.db == Some(id))
                            {
                                session.db = None;
                            }
                            this.message = "会话已删除".into();
                        }
                        Err(error) => this.message = format!("删除失败：{error}"),
                    }
                    this.agent_reload_history(window, cx);
                });
            })
            .detach();
            return;
        }
        let job = cx.background_spawn(async move {
            match op {
                HistoryOp::Pin(id) => store.pin(id),
                HistoryOp::Unpin(id) => store.unpin(id),
                HistoryOp::MovePin(id, before) => store.move_pin(id, before),
                HistoryOp::Archive(id, archived) => store.set_archived(id, archived),
                HistoryOp::Rename(id, title) => store.rename(id, title),
                HistoryOp::DeleteArchived(scope) => {
                    store.delete_archived(&scope).map(|_| ())?;
                }
                HistoryOp::Delete(..) => {}
            }
            store.flush()
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    this.message = format!("会话历史未能更新：{error}");
                }
                this.agent_reload_history(window, cx);
            });
        })
        .detach();
    }

    pub(super) fn agent_start_rename(
        &mut self,
        id: SessionId,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let title = input.read(cx).value().trim().to_string();
                    this.agent.history.renaming = None;
                    if !title.is_empty() {
                        for session in this.agent.sessions.iter_mut().filter(|s| s.db == Some(id)) {
                            session.thread.title = Some(title.clone());
                        }
                        this.agent_history_op(HistoryOp::Rename(id, title), window, cx);
                    }
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.agent.history.renaming = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self.agent.history.renaming = Some((id, input, subscription));
        cx.notify();
    }

    pub(super) fn agent_delete_archived(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use workspace_editor_agent_history::Scope;
        let scope = match (&self.root, self.agent.history.filter.all_workspaces) {
            (Some(root), false) => Scope::Workspace(root.clone()),
            _ => Scope::All,
        };
        let count = self.agent.history.counts.2;
        if count == 0 {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("永久删除 {count} 个已归档会话？"),
            Some("删除后释放磁盘空间，无法恢复。"),
            &["取消", "删除"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.agent_history_op(HistoryOp::DeleteArchived(scope), window, cx)
                });
            }
        })
        .detach();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PermissionChoice {
    Once,
    Always(String),
    Reject,
}

pub(super) enum HistoryOp {
    Pin(SessionId),
    Unpin(SessionId),
    MovePin(SessionId, Option<SessionId>),
    Archive(SessionId, bool),
    Rename(SessionId, String),
    Delete(SessionId, String),
    DeleteArchived(workspace_editor_agent_history::Scope),
}

/// Runs history writes off the UI thread (the first call opens the database).
fn background_history(
    cx: &mut Context<Prototype>,
    store: Arc<History>,
    work: impl FnOnce(&History) + Send + 'static,
) {
    cx.background_spawn(async move { work(&store) }).detach();
}

type BufferRequest = (PathBuf, async_channel::Sender<Option<String>>);

/// Answers the agents' `fs/read_text_file` with unsaved buffers from any window. The agent
/// threads only send requests; the UI thread reads the buffers when it gets to them.
struct BufferBridge(async_channel::Sender<BufferRequest>);

impl workspace_editor_agent::BufferProvider for BufferBridge {
    fn buffer_text(
        &self,
        path: &std::path::Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
        let (reply, answer) = async_channel::bounded(1);
        let sent = self.0.try_send((path.to_path_buf(), reply)).is_ok();
        Box::pin(async move {
            if sent {
                answer.recv().await.ok().flatten()
            } else {
                None
            }
        })
    }
}

struct AgentBuffers {
    bridge: Arc<BufferBridge>,
    _task: Task<()>,
}
impl Global for AgentBuffers {}

/// One bridge for the whole app, started with the first agent.
fn buffer_provider(cx: &mut App) -> Arc<dyn workspace_editor_agent::BufferProvider> {
    if let Some(buffers) = cx.try_global::<AgentBuffers>() {
        return buffers.bridge.clone();
    }
    let (requests, incoming) = async_channel::unbounded::<BufferRequest>();
    let task = cx.spawn(async move |cx| {
        while let Ok((path, reply)) = incoming.recv().await {
            let text = cx.update(|cx| super::documents::buffer_text(&path, cx));
            let _ = reply.try_send(text);
        }
    });
    let bridge = Arc::new(BufferBridge(requests));
    cx.set_global(AgentBuffers {
        bridge: bridge.clone(),
        _task: task,
    });
    bridge
}
