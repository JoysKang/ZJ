//! Agent panel state and logic: live sessions (one ACP client each, started on the first
//! prompt) and the panel's views here; the rest by topic in `agent/`: `turns` (sending, the
//! event pump, logins), `permissions` (requests, 始终允许 rules, modes), `composer` (context
//! and the @ picker), `changes` (changed files), `history` (stored sessions) and `buffers`
//! (unsaved buffers for the agents). Rendering is in `agent_panel.rs`, `agent_history.rs`
//! and `agent_review.rs`.
//!
//! Resource rules: nothing runs while the panel is closed and no session is active; the
//! spinner's timer only ticks while the panel is visible in the focused window and a session
//! runs; a visible Codex thread refreshes quota every ten minutes. Threads keep at most
//! `thread::MAX_ITEMS` items in memory and older messages come back from the history database.

use super::*;
use crate::agent_model::{self, Attachment};
use crate::markdown;
use gpui_kit::component::input::InputState;
use workspace_editor_agent::{
    AgentClient, AgentCommand, AgentEvent, AgentPool, AgentPreset, ClientOptions, PermissionKind,
    builtin_presets,
    review::{line_counts, resolve_file, review_texts},
    thread::{self as agent_thread, FileChange, Item, PermissionState, Record, Thread},
};
use workspace_editor_agent_history::{
    History, NewSession, Role as HistoryRole, SessionId, SessionStatus, SessionSummary,
};

mod buffers;
mod changes;
mod composer;
mod highlights;
mod notifications;
pub use notifications::init as init_notifications;
pub(super) mod history;
mod permissions;
mod quota;
mod turns;
use buffers::buffer_provider;
pub(super) use history::HistoryOp;
use history::background_history;
pub(super) use permissions::PermissionChoice;

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
    /// Where sessions run in a window without a folder; created on first use.
    pub default_workspace: Option<PathBuf>,
}

impl Global for AgentStore {}

pub fn init_store(cx: &mut App) {
    let path = std::env::var_os("ZJ_AGENT_DB")
        .map(PathBuf::from)
        .or_else(workspace_editor_agent_history::default_db_path);
    let default_workspace = std::env::var_os("ZJ_AGENT_WORKSPACE")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(default_workspace_path);
    cx.set_global(AgentStore {
        history: path.map(|path| Arc::new(History::new(path))),
        default_workspace,
    });
}

/// `~/Library/Application Support/ZJ/workspace` on macOS, `$XDG_DATA_HOME/zj/workspace` (or
/// `~/.local/share/zj/workspace`) elsewhere, next to the history database.
fn default_workspace_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home?.join("Library/Application Support/ZJ/workspace"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")))?;
    Some(data.join("zj/workspace"))
}

fn history(cx: &App) -> Option<Arc<History>> {
    cx.try_global::<AgentStore>()
        .and_then(|store| store.history.clone())
}

/// The agent processes of all windows: sessions of the same agent with the same variables
/// share one (ADR 0004).
struct AgentProcesses(Arc<AgentPool>);

impl Global for AgentProcesses {}

pub(super) fn agent_pool(cx: &mut App) -> Arc<AgentPool> {
    if !cx.has_global::<AgentProcesses>() {
        let pool = Arc::new(AgentPool::new());
        let quitting = pool.clone();
        cx.on_app_quit(move |_| {
            // Complete process cleanup before native termination can exit the program.
            quitting.shutdown();
            async {}
        })
        .detach();
        cx.set_global(AgentProcesses(pool));
    }
    cx.global::<AgentProcesses>().0.clone()
}

pub(super) fn default_workspace(cx: &App) -> Option<PathBuf> {
    cx.try_global::<AgentStore>()
        .and_then(|store| store.default_workspace.clone())
}

pub(super) const DEFAULT_WORKSPACE_NAME: &str = "默认工作区";

/// The name shown for a session's workspace.
pub(super) fn workspace_label(root: &std::path::Path, cx: &App) -> String {
    if default_workspace(cx).as_deref() == Some(root) {
        DEFAULT_WORKSPACE_NAME.into()
    } else {
        agent_model::file_name(root)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentView {
    Thread,
    History,
    Settings,
}

/// A session in this window. The client starts with the first prompt.
pub(super) struct LiveSession {
    pub changes_collapsed: bool,
    pub key: u64,
    pub preset: AgentPreset,
    pub client: Option<Arc<AgentClient>>,
    /// The workspace the agent runs in: the window's folder, or the default workspace when
    /// the window has none. Fixed when the agent first starts.
    pub root: Option<PathBuf>,
    starting: bool,
    pub thread: Thread,
    pub db: Option<SessionId>,
    db_creating: bool,
    /// Records waiting for the database row.
    pending: Vec<(HistoryRole, String)>,
    pub started_at: i64,
    pub branch: Option<String>,
    /// Reply texts by absolute item index (`thread.dropped + i`) for Kit's `TextView`: the
    /// same allocation every frame, so an unchanged reply is not compared again.
    pub md: HashMap<usize, SharedString>,
    /// How long each finished turn took, by the absolute index of its last row.
    pub turn_times: HashMap<usize, std::time::Duration>,
    /// Oldest stored message loaded (history threads page backwards from it).
    pub oldest_seq: Option<i64>,
    resume: Option<String>,
    stored_status: Option<SessionStatus>,
    pub turns: usize,
    pump: Option<Task<()>>,
    stats_task: Option<Task<()>>,
    /// Files to recount once the recount in flight is done (`None`: all of them).
    recount_pending: Option<std::collections::BTreeSet<PathBuf>>,
    /// The prompt typed while the agent was starting.
    queued: Option<(String, Vec<Attachment>)>,
    /// The last event, prompt or look at it: idle sessions are put away after a while.
    last_active: std::time::Instant,
    /// The last prompt: the line above the composer counts from it.
    pub turn_started: std::time::Instant,
    /// A turn stopped by the user remains quiet even when cancellation times out.
    cancelled_turn: Option<workspace_editor_agent::TurnId>,
}

impl LiveSession {
    fn new(key: u64, preset: AgentPreset) -> Self {
        Self {
            changes_collapsed: true,
            key,
            preset,
            client: None,
            root: None,
            starting: false,
            thread: Thread::new(),
            db: None,
            db_creating: false,
            pending: Vec::new(),
            started_at: workspace_editor_agent_history::now_ms(),
            branch: None,
            md: HashMap::new(),
            turn_times: HashMap::new(),
            oldest_seq: None,
            resume: None,
            stored_status: None,
            turns: 0,
            pump: None,
            stats_task: None,
            recount_pending: Some(Default::default()),
            queued: None,
            last_active: std::time::Instant::now(),
            turn_started: std::time::Instant::now(),
            cancelled_turn: None,
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
        let status = if self.starting() {
            agent_thread::Status::Running
        } else {
            self.thread.status
        };
        agent_model::RowStatus::of_thread(status, self.thread.unread)
    }

    /// Starting the agent with nothing sent yet.
    pub fn starting(&self) -> bool {
        self.starting && self.thread.status == agent_thread::Status::Idle
    }

    pub fn busy(&self) -> bool {
        self.starting
            || matches!(
                self.thread.status,
                agent_thread::Status::Running | agent_thread::Status::Awaiting
            )
    }
}

/// The `@` file and folder picker under the composer.
pub(super) struct Mention {
    pub range: std::ops::Range<usize>,
    pub query: String,
    pub results: Vec<files::Entry>,
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
    pub image_previews: HashMap<u64, Arc<Image>>,
    pub images_loading: usize,
    pub image_preview_task: Option<Task<()>>,
    image_generation: u64,
    next_image_reference: u64,
    pub image_references: HashMap<String, Option<u64>>,
    pub mention: Option<Mention>,
    mention_generation: u64,
    /// The selected row of the `/` command picker, while the message starts with a command.
    pub slash: Option<usize>,
    /// The last commands each agent listed, by (preset id, workspace): a new or reopened
    /// session offers them before its agent has started.
    pub known_commands: HashMap<(String, PathBuf), Vec<AgentCommand>>,
    /// Why an agent can't start here (CLI not installed or too old, key missing), by preset
    /// id: checked whenever it is picked, shown in its empty session. ZJ installs nothing.
    pub unavailable: HashMap<String, String>,
    /// Questions being answered (several picks, typed text), by (session key, request id).
    pub questions: HashMap<(u64, workspace_editor_agent::PermissionId), permissions::QuestionDraft>,
    /// The latest of those checks: an older answer arriving later is dropped.
    check_generation: u64,
    /// The Codex account quota (see `crate::quota`) and whether its card is open.
    pub quota: Option<crate::quota::Quota>,
    pub quota_open: bool,
    pub quota_pinned: bool,
    pub quota_loading: bool,
    pub quota_error: Option<String>,
    quota_task: Option<Task<()>>,
    quota_timer: Option<Task<()>>,
    pub quota_next_refresh_ms: Option<i64>,
    pub history: HistoryList,
    pub search: Option<super::agent_search::SessionSearch>,
    pub thread_list: ListState,
    pub thread_rows: Vec<agent_model::ThreadRow>,
    list_shape: (u64, usize, usize, bool),
    pub spin: usize,
    spin_task: Option<Task<()>>,
    width_task: Option<Task<()>>,
    pub expanded_tools: HashSet<(u64, String)>,
    pub expanded_thoughts: HashSet<(u64, usize)>,
    pub expanded_processes: HashSet<(u64, usize)>,
    pub collapsed_plans: HashSet<u64>,
    pub strip_collapsed: bool,
    pub switcher_open: bool,
    pub presets: Vec<AgentPreset>,
    /// The agent picked for the next new session.
    pub agent_id: String,
    pub composer_focused: bool,
    /// Syntax colors for code blocks in replies.
    pub code: highlights::CodeHighlights,
    _subscriptions: Vec<Subscription>,
}

impl AgentPanel {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 8)
                .submit_on_enter(true)
                .placeholder("继续追问，@ 引用文件或文件夹")
        });
        let events = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Workbench, _, event: &InputEvent, window, cx| match event {
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
        let focus = composer.read(cx).focus_handle(cx);
        let entered = cx.on_focus_in(&focus, window, |this: &mut Workbench, window, cx| {
            this.input_area_entered(crate::input_switch::Area::Composer, window, cx)
        });
        let left = cx.on_focus_out(&focus, window, |this: &mut Workbench, _, window, cx| {
            this.input_composer_left(window, cx)
        });
        let presets = presets_from(&settings);
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
            image_previews: HashMap::new(),
            images_loading: 0,
            image_preview_task: None,
            image_generation: 0,
            next_image_reference: 0,
            image_references: HashMap::new(),
            mention: None,
            mention_generation: 0,
            slash: None,
            known_commands: HashMap::new(),
            unavailable: HashMap::new(),
            questions: HashMap::new(),
            check_generation: 0,
            quota: None,
            quota_open: false,
            quota_pinned: false,
            quota_loading: false,
            quota_error: None,
            quota_task: None,
            quota_timer: None,
            quota_next_refresh_ms: None,
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
            thread_rows: Vec::new(),
            list_shape: (0, 0, 0, false),
            spin: 0,
            spin_task: None,
            width_task: None,
            expanded_tools: HashSet::new(),
            expanded_thoughts: HashSet::new(),
            expanded_processes: HashSet::new(),
            collapsed_plans: HashSet::new(),
            strip_collapsed: false,
            switcher_open: false,
            agent_id: settings.default_agent.clone(),
            presets,
            composer_focused: false,
            code: highlights::CodeHighlights::new(cx),
            _subscriptions: vec![events, entered, left],
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

    /// The preset `id`, or the first one when it is gone (removed from the settings).
    pub fn preset_or_first(&self, id: &str) -> Option<&AgentPreset> {
        self.preset(id).or_else(|| self.presets.first())
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

/// The agent's glyph; unknown agents get the generic one.
pub(super) fn glyph_for(presets: &[AgentPreset], id: &str) -> workspace_editor_agent::Glyph {
    presets
        .iter()
        .find(|p| p.id == id)
        .map_or(workspace_editor_agent::Glyph::Generic, |p| p.glyph)
}

/// The built-in presets followed by the user's own agents from the settings.
fn presets_from(settings: &crate::settings::AgentSettings) -> Vec<AgentPreset> {
    let mut presets = builtin_presets();
    presets.extend(settings.custom.iter().cloned().map(|a| a.into_preset()));
    presets
}

fn record_role(record: &Record) -> (HistoryRole, String) {
    match record {
        Record::Agent(text) => (HistoryRole::Agent, text.clone()),
        Record::Tool(title) => (HistoryRole::Tool, title.clone()),
    }
}

impl Workbench {
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
        self.agent_sync_quota_timer(cx);
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
            self.agent_refresh_quota(cx);
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
        self.agent_sync_quota_timer(cx);
        cx.notify();
    }

    pub(super) fn agent_select(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.session(key).is_none() {
            return;
        }
        // The session being left was looked at until now.
        if let Some(left) = self.agent.current.and_then(|k| self.agent.session_mut(k)) {
            left.last_active = std::time::Instant::now();
        }
        self.agent.current = Some(key);
        self.agent_reclaim_idle(cx);
        self.agent_clear_attachments(cx);
        self.agent.mention = None;
        self.agent.slash = None;
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
    pub(super) fn agent_ensure_session(&mut self, cx: &mut Context<Self>) {
        if self.agent.current.is_some() {
            return;
        }
        let id = self.agent.agent_id.clone();
        let Some(preset) = self.agent.preset_or_first(&id).cloned() else {
            return;
        };
        self.agent_check_available(preset.clone(), cx);
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
        let Some(preset) = self.agent.preset_or_first(&id).cloned() else {
            return;
        };
        self.agent.agent_id = preset.id.clone();
        self.agent_check_available(preset.clone(), cx);
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
        self.agent_clear_attachments(cx);
        self.agent_reclaim_idle(cx);
        self.agent_sync_list(true);
        self.agent_show(AgentView::Thread, window, cx);
    }

    /// Checks off the UI thread that `preset` can start here (its CLI runs `--version`, a
    /// keychain key is looked up). Runs each time the agent is picked, so an install done
    /// meanwhile is noticed.
    fn agent_check_available(&mut self, preset: AgentPreset, cx: &mut Context<Self>) {
        let overrides = cx
            .global::<crate::settings::Settings>()
            .agent
            .env_for(&preset.id);
        let id = preset.id.clone();
        self.agent.check_generation += 1;
        let generation = self.agent.check_generation;
        let check = cx.background_spawn(async move {
            let env = crate::secrets::resolve(&overrides, |name| std::env::var(name).ok())?;
            preset
                .resolve(&workspace_editor_agent::SearchPath::from_env(), &env)
                .map(drop)
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = check.await;
            let _ = this.update(cx, |this, cx| {
                if this.agent.check_generation != generation {
                    return;
                }
                let changed = match result {
                    Ok(()) => this.agent.unavailable.remove(&id).is_some(),
                    Err(message) => {
                        eprintln!("event=agent_unavailable agent={id}");
                        this.agent.unavailable.insert(id, message.clone()) != Some(message)
                    }
                };
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Puts away sessions left idle longer than the agent idle setting (their process has
    /// stopped by then too): not the one shown, nothing running, waiting, unread, queued or
    /// unsaved, and no changes waiting for review. They stay in the history and reopen from
    /// there. Runs on events (switching, a new session, a turn ending, the window coming
    /// back), never on a timer.
    pub(super) fn agent_reclaim_idle(&mut self, cx: &mut Context<Self>) {
        let minutes = cx.global::<crate::settings::Settings>().agent.idle_minutes;
        let idle = Duration::from_secs(u64::from(minutes) * 60);
        let current = self.agent.current;
        let idle_enough = |s: &LiveSession| {
            Some(s.key) != current
                && !s.busy()
                && !s.thread.unread
                && s.queued.is_none()
                && s.db.is_some()
                && !s.db_creating
                && s.pending.is_empty()
                && s.thread.changed_files.is_empty()
                && s.last_active.elapsed() >= idle
        };
        if !self.agent.sessions.iter().any(idle_enough) {
            return;
        }
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.sessions)
            .into_iter()
            .partition(|s| idle_enough(s));
        self.agent.sessions = kept;
        for mut session in gone {
            eprintln!("event=agent_session_reclaimed agent={}", session.preset.id);
            // Stopping a client may wait for its process: not on the UI thread.
            if let Some(client) = session.client.take() {
                cx.background_spawn(async move { drop(client) }).detach();
            }
        }
        self.agent_sync_list(false);
        cx.notify();
    }
}
