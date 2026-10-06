use crate::files::{self, Entry, PathIndex};
use crate::theme;
use gpui_kit::{
    component::{
        input::{EditorState, InputEvent, TextareaState},
        resizable::{h_resizable, resizable_panel, v_resizable},
        v_flex,
    },
    *,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use workspace_editor_core::{DocumentId, RepoId, Repository};
use workspace_editor_git::{
    ChangeKind, Commit, DiffSide, Discovery, GitService, Operation, Request, Status,
    WriteOperation, WriteRequest,
};

mod agent;
pub use agent::{
    AddSelectionToAgent, NewAgentSession, NextApproval, SearchSessions, ToggleAgentPanel,
    init_store as init_agent_store,
};
mod agent_history;
mod agent_panel;
mod agent_review;
pub use agent_review::{AcceptAgentChange, RejectAgentChange};
mod agent_search;
mod chrome;
mod commands;
pub use commands::ShowAllCommands;
#[cfg(test)]
#[path = "workbench/commands_ui_tests.rs"]
mod commands_ui_tests;
mod diff_ops;
pub use diff_ops::{CopyDiff, SelectAllDiff};
mod diff_view;
mod documents;
pub use documents::{
    AutoSaveAfterDelay, AutoSaveOff, AutoSaveOnFocusChange, CloseEditor, NewUntitled,
    OpenDocuments, Quit, Save, SaveAll, SaveAs, ToggleLineEnding, ToggleSoftWrap, quit,
};
mod edit_commands;
pub use edit_commands::{
    CopyLinesDown, CopyLinesUp, MoveLinesDown, MoveLinesUp, SelectNextOccurrence,
    ToggleLineComment, key_bindings as edit_key_bindings,
};
#[cfg(test)]
#[path = "workbench/edit_commands_ui_tests.rs"]
mod edit_commands_ui_tests;
mod editor_area;
mod explorer_ops;
mod find_widget;
#[cfg(test)]
#[path = "workbench/go_to_line_ui_tests.rs"]
mod go_to_line_ui_tests;
mod graph_view;
mod large_view;
mod markdown_preview;
#[cfg(test)]
#[path = "workbench/markdown_ui_tests.rs"]
mod markdown_ui_tests;
pub use explorer_ops::{
    CopyFiles, CopyPath, CopyRelativePath, CutFiles, Delete as DeleteFile, NewFile, NewFolder,
    PasteFiles, Rename as RenameFile, RevealInFinder,
};
pub use find_widget::{
    FindInFile, FindNext, FindPrevious, FindReplace, ReplaceAll, ReplaceOne, ToggleFindCase,
    ToggleFindInSelection, ToggleFindRegex, ToggleFindWord, TogglePreserveCase,
};
pub use markdown_preview::ToggleMarkdownPreview;
#[cfg(test)]
#[path = "workbench/auto_save_ui_tests.rs"]
mod auto_save_ui_tests;
pub mod navigation;
mod quick_open;
pub(crate) mod recovery;
#[cfg(test)]
#[path = "workbench/reopen_ui_tests.rs"]
mod reopen_ui_tests;
mod scm;
mod scm_actions;
mod search_replace;
mod search_view;
#[cfg(test)]
#[path = "workbench/session_ui_tests.rs"]
mod session_ui_tests;
mod sidebar;
mod tab_menu;
mod terminal_panel;
pub use terminal_panel::{KillTerminal, NewTerminal, SplitTerminal, ToggleTerminal};
#[cfg(test)]
#[path = "workbench/terminal_ui_tests.rs"]
mod terminal_ui_tests;
mod terminal_view;
#[cfg(test)]
#[path = "workbench/test_support.rs"]
mod test_support;
pub use tab_menu::{
    CloseAllEditors, CloseOtherEditors, CopyActivePath, CopyActiveRelativePath, ReopenClosedEditor,
    RevealActiveInExplorer, RevealActiveInFinder,
};
#[cfg(test)]
#[path = "workbench/idle_ui_tests.rs"]
mod idle_ui_tests;
#[cfg(test)]
#[path = "workbench/indent_ui_tests.rs"]
mod indent_ui_tests;
#[cfg(test)]
#[path = "workbench/soft_wrap_ui_tests.rs"]
mod soft_wrap_ui_tests;
#[cfg(test)]
#[path = "workbench/tab_menu_ui_tests.rs"]
mod tab_menu_ui_tests;
#[cfg(test)]
#[path = "workbench/welcome_ui_tests.rs"]
mod welcome_ui_tests;
mod workspace_refresh;

gpui_kit::actions!(
    workspace,
    [
        NewWindow,
        OpenFile,
        OpenFolder,
        QuickOpenFile,
        ToggleSidebar,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleHiddenFiles,
        FindInFiles
    ]
);

#[derive(Clone)]
pub struct DocumentOwner {
    path: PathBuf,
    view: WeakEntity<Workbench>,
    window: AnyWindowHandle,
}
pub type DocumentOwners = Rc<RefCell<HashMap<DocumentId, DocumentOwner>>>;

#[derive(Clone, Copy, PartialEq)]
enum Sidebar {
    Explorer,
    Search,
    SourceControl,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pane {
    Welcome,
    Document(DocumentId),
    Diff,
    /// The Git Graph tab (one at a time, like the diff preview).
    Graph,
    /// The restricted viewer of a file too large (or not UTF-8) to edit, one at a time.
    Large,
}

struct Document {
    id: DocumentId,
    path: PathBuf,
    editor: Entity<EditorState>,
    dirty: bool,
    readonly: bool,
    bytes: usize,
    /// Display name of the language shown in the status bar.
    language: &'static str,
    crlf: bool,
    bom: bool,
    /// Tab key and indent width of this buffer (the status bar can change it).
    indent: crate::indent::Indent,
    /// Long lines wrap at the editor's width.
    soft_wrap: bool,
    /// The file as last loaded or saved (`None` for untitled buffers).
    disk: Option<crate::save::DiskState>,
    /// Untitled-N: saving asks for a path.
    untitled: bool,
    /// Counts edits, so a save that finishes after more typing keeps the tab edited.
    version: u64,
    saving: bool,
    /// The file was deleted on disk; the buffer is kept (and counts as edited).
    deleted: bool,
    /// The buffer's version when its file was deleted, if it had no edits then: a file that
    /// comes back (a branch switch) reloads silently.
    unedited_when_deleted: Option<u64>,
    /// The file changed on disk while the buffer had edits.
    banner: Option<documents::Banner>,
    /// `version` when an agent last read the unsaved text: if nothing was typed since, the
    /// file the agent writes back replaces the buffer instead of raising the banner.
    agent_read: Option<u64>,
    auto_save: crate::save::Debounce,
    auto_save_task: Option<Task<()>>,
    /// The recovery snapshot key of an untitled buffer (made on its first snapshot, or kept
    /// from the snapshot it was restored from).
    untitled_key: Option<String>,
    /// Edits waiting for their recovery snapshot (`recovery::SNAPSHOT_DELAY`).
    snapshot: crate::save::Debounce,
    snapshot_task: Option<Task<()>>,
    /// The key of this buffer's snapshot on disk, if one was written.
    snapshot_on_disk: Option<String>,
    /// Restored from a snapshot at launch: the recovery banner shows until 保留 / a save.
    recovered: bool,
    /// Markdown files: the live preview (`None` for other languages).
    markdown: Option<markdown_preview::MarkdownPreview>,
    _subscription: Subscription,
}

struct DiffTab {
    label: String,
    tooltip: String,
    path: PathBuf,
    source: DiffSource,
}

/// Where a diff tab's patch comes from.
#[derive(Clone)]
enum DiffSource {
    Git(Request),
    /// A full-context patch built in the app (the Search view's replace preview).
    Local(Arc<str>),
    /// An agent's changes to review (built from the client's snapshot or proposal).
    Agent(agent_review::AgentDiff),
}

impl DiffTab {
    /// The Git comparison, if this tab shows one (only those can be staged or reverted).
    fn request(&self) -> Option<&Request> {
        match &self.source {
            DiffSource::Git(request) => Some(request),
            DiffSource::Local(_) | DiffSource::Agent(_) => None,
        }
    }

    fn agent(&self) -> Option<&agent_review::AgentDiff> {
        match &self.source {
            DiffSource::Agent(diff) => Some(diff),
            _ => None,
        }
    }
}

struct TreeRow {
    entry: Entry,
    depth: usize,
    /// The inline name field of a new file or folder (`entry.path` is its parent).
    pending: bool,
}

struct Group {
    repo: Repository,
    status: Option<Result<Arc<Status>, String>>,
    expanded: bool,
    /// Collapsed resource groups ("暂存的更改" / "更改"), as in VS Code.
    staged_collapsed: bool,
    changes_collapsed: bool,
    commit_input: Entity<TextareaState>,
    _commit_subscription: Subscription,
    write_task: Option<Task<()>>,
    write_pending: bool,
    write_message: String,
    /// Commits the upstream does not have, loaded with the status (empty when none).
    outgoing: Result<Vec<Commit>, String>,
    outgoing_collapsed: bool,
}
impl Group {
    /// A repository whose status loaded and has nothing to commit; it is listed compactly
    /// after the others (or hidden), as in VS Code.
    fn clean(&self) -> bool {
        matches!(&self.status, Some(Ok(status)) if status.changes.is_empty())
    }

    /// Commits ahead of the upstream (0 without one).
    fn ahead(&self) -> usize {
        match &self.status {
            Some(Ok(status)) if status.upstream.is_some() => status.ahead,
            _ => 0,
        }
    }

    /// Whether the header opens: there are changes, or commits to push.
    fn expandable(&self) -> bool {
        !self.clean() || self.ahead() > 0
    }
}

/// Unpushed commits listed under a repository; older ones are counted, not listed.
const OUTGOING_LIMIT: usize = 100;

/// Line breaks in file names would break single-line rows; they are shown as ⏎.
const SINGLE_LINE: [char; 2] = ['\n', '\r'];

/// Highlighter language (only grammars compiled into Kit) and status bar display name.
/// Prompt buttons, the action first (macOS puts it on the right, on Return) and 取消 marked
/// as the cancel button so Escape answers it; a plain "取消" label is not recognized as one.
pub(crate) fn prompt_buttons(labels: &[&str]) -> Vec<PromptButton> {
    labels
        .iter()
        .map(|&label| match label {
            "取消" => PromptButton::cancel(label),
            _ => PromptButton::new(label),
        })
        .collect()
}

fn language_for(path: &std::path::Path) -> (&'static str, &'static str) {
    crate::languages::for_path(path)
}

/// Explorer git decoration, VS Code style: colored name plus a letter (files) or dot (folders).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum DecorationKind {
    Untracked,
    Added,
    Modified,
    Deleted,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Decoration {
    kind: DecorationKind,
    letter: char,
}

fn decoration(change: &workspace_editor_git::Change) -> Decoration {
    let code = if change.worktree != b'.' {
        change.worktree
    } else {
        change.index
    };
    let (kind, letter) = match (change.kind, code) {
        (ChangeKind::Conflict, _) => (DecorationKind::Conflict, '!'),
        (ChangeKind::Untracked, _) => (DecorationKind::Untracked, 'U'),
        (_, b'A') => (DecorationKind::Added, 'A'),
        (_, b'D') => (DecorationKind::Deleted, 'D'),
        (ChangeKind::Renamed, _) => (DecorationKind::Modified, 'R'),
        _ => (DecorationKind::Modified, 'M'),
    };
    Decoration { kind, letter }
}

#[derive(Clone, Copy)]
enum Row {
    Group(usize),
    /// The repository's commit message box and 提交 button.
    Commit(usize),
    Heading(usize, DiffSide, usize),
    File(usize, usize, DiffSide),
    /// "未推送的提交" with the push button.
    Outgoing(usize),
    OutgoingCommit(usize, usize),
    /// Under the commits: why they could not be listed, or how many older ones are not.
    OutgoingNote(usize),
}
enum Event {
    Repo(Repository),
    Status(RepoId, Result<Status, String>, Result<Vec<Commit>, String>),
    Issue(String),
    Excluded,
    Done,
}

pub struct Workbench {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    service: GitService,
    groups: Vec<Group>,
    rows: Vec<Row>,
    scm_repo: Option<RepoId>,
    write_generation: u64,
    sidebar: Sidebar,
    sidebar_visible: bool,
    quick_open: Option<quick_open::QuickOpen>,
    /// 命令面板里最近执行的命令名称，新的在前（只在内存，不持久化）。
    command_history: Vec<&'static str>,
    active: Pane,
    documents: Vec<Document>,
    owners: DocumentOwners,
    /// The Explorer: the visible tree, expanded folders, selection and inline rename, and its folder listings.
    explorer: sidebar::Explorer,
    /// Source Control lists only repositories with changes (settings).
    hide_clean_repos: bool,
    /// Source Control rows vary in height (repository rows wrap long branch names), so they
    /// use a measured list instead of `uniform_list`.
    scm_list: ListState,
    search: search_view::SearchState,
    find: find_widget::FindState,
    /// The selection ⌘D made from the word under the cursor: the next ⌘D matches whole words.
    whole_word_selection: Option<(DocumentId, std::ops::Range<usize>)>,
    /// Buffers being reloaded after a replace in the Search view: their next change event
    /// does not mark them edited.
    reloading: std::collections::HashSet<DocumentId>,
    index: Option<Arc<PathIndex>>,
    index_task: Option<Task<()>>,
    index_cancel: Arc<AtomicBool>,
    index_generation: u64,
    file_task: Option<Task<()>>,
    file_generation: u64,
    path_prompt_open: bool,
    closing: bool,
    /// What the window's "edited" mark currently shows.
    window_edited: bool,
    /// Cursor (line, column) of the active editor, 0-based, for the status bar.
    cursor: Option<(u32, u32)>,
    _cursor_observer: Option<Subscription>,
    /// The diff tab: what it shows, the parsed document, selection and scrolling, and its loading task.
    diff: diff_view::DiffPane,
    /// Go to definition, references and back / forward: the symbol index and the navigation history.
    nav: navigation::NavState,
    /// The Git Graph tab's state (opened from a repository's header).
    graph: Option<graph_view::GitGraph>,
    large: Option<large_view::LargeView>,
    /// The last tab right-click menu; tab tooltips stay hidden while it has focus.
    tab_menu_focus: Option<FocusHandle>,
    /// ⇧⌘T reopens these, most recent first.
    closed_tabs: tab_menu::ClosedTabs,
    /// Tabs restored from the last session that have not been opened yet: shown in the tab
    /// bar, read only when chosen (开发说明: clean tabs load on demand).
    pending_tabs: Vec<PathBuf>,
    terminals: terminal_panel::Terminals,
    generation: u64,
    cancel: Arc<AtomicBool>,
    loading: bool,
    refresh_completed: bool,
    excluded: usize,
    issues: Vec<String>,
    message: String,
    refresh_task: Option<Task<()>>,
    /// File watching: the shared subscription, the debounce task and refreshes waiting for a quiet moment.
    watch: workspace_refresh::WatchState,
    /// The agent panel: sessions, thread view, history and the ⌘J search.
    agent: agent::AgentPanel,
    _subscriptions: Vec<Subscription>,
}

impl Drop for Workbench {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.index_cancel.store(true, Ordering::Relaxed);
        for document in &self.documents {
            self.owners.borrow_mut().remove(&document.id);
        }
        eprintln!(
            "event=window_closed generation={} groups={}",
            self.generation,
            self.groups.len()
        );
    }
}

impl Workbench {
    pub fn new(
        root: Option<PathBuf>,
        service: GitService,
        owners: DocumentOwners,
        number: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        theme::follow_appearance(Some(window), cx);
        let appearance = cx.observe_window_appearance(window, |this, window, cx| {
            theme::follow_appearance(Some(window), cx);
            // Diff colors and syntax spans are baked into the document; rebuild them.
            if this.diff.tab.is_some() {
                this.load_diff(window, cx);
            }
            this.agent_rehighlight(cx);
        });
        // Another window (or this one) changed a setting that changes what is listed.
        let settings =
            cx.observe_global_in::<crate::settings::Settings>(window, |this, window, cx| {
                let hide_clean = cx.global::<crate::settings::Settings>().hide_clean_repos;
                if hide_clean != this.hide_clean_repos {
                    this.hide_clean_repos = hide_clean;
                    this.rebuild_rows();
                    cx.notify();
                }
                let show_hidden = cx.global::<crate::settings::Settings>().show_hidden;
                if show_hidden != this.explorer.show_hidden {
                    this.explorer.show_hidden = show_hidden;
                    this.reload_tree(window, cx);
                    if !this.search.query.read(cx).value().is_empty() {
                        this.schedule_search(Duration::ZERO, window, cx);
                    }
                    this.update_quick_open(window, cx);
                }
                this.agent_follow_settings(cx);
            });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            this.agent_update_spin(window, cx);
            if window.is_window_active() {
                // Files opened from outside the workspace are not watched: look on activation.
                this.check_disk(None, window, cx);
                if this.root.is_some() {
                    this.refresh_on_activation(window, cx);
                }
            } else {
                this.auto_save_on_focus_change(None, window, cx);
            }
        });
        crate::session::remember(window, root.as_deref(), true, cx);
        // A buffer saved in any window may be a file an agent here changed: its review and
        // line counts compare against what is on disk now.
        let saved_view = cx.weak_entity();
        let saved_window = window.window_handle();
        documents::on_buffer_saved(cx, move |path, cx| {
            let path = path.to_path_buf();
            let saved_view = saved_view.clone();
            // Deferred: the saving window's workbench is still being updated here.
            cx.defer(move |cx| {
                let _ = saved_window.update(cx, |_, window, cx| {
                    let _ =
                        saved_view.update(cx, |this, cx| this.agent_file_saved(&path, window, cx));
                });
            });
        });
        let frame = cx.observe_window_bounds(window, |this, window, cx| {
            crate::session::remember(window, this.root.as_deref(), false, cx);
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| {
                if this.groups.iter().any(|g| g.write_pending) {
                    this.message = "Git 操作尚未结束，请稍后关闭窗口".into();
                    cx.notify();
                    return false;
                }
                if !this.documents.iter().any(|document| document.dirty) {
                    return true;
                }
                // 「要保存对 X 的更改吗？」, then the window closes itself.
                this.close_window_after_confirm(window, cx);
                false
            })
            .unwrap_or(true)
        });
        let name = root
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("新窗口 {number}"));
        window.set_window_title(&format!("ZJ · {name}"));
        let search = search_view::SearchState::new(window, cx);
        let find = find_widget::FindState::new(window, cx);
        let agent = agent::AgentPanel::new(window, cx);
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            root,
            service,
            groups: vec![],
            rows: vec![],
            scm_repo: None,
            write_generation: 0,
            sidebar: Sidebar::Explorer,
            sidebar_visible: true,
            quick_open: None,
            command_history: Vec::new(),
            active: Pane::Welcome,
            documents: Vec::new(),
            owners,
            explorer: sidebar::Explorer {
                rows: Vec::new(),
                collapsed: false,
                decorations: HashMap::new(),
                scroll: UniformListScrollHandle::new(),
                show_hidden: cx.global::<crate::settings::Settings>().show_hidden,
                selection: None,
                focus: cx.focus_handle(),
                edit: None,
                focus_on_open: false,
                reveal_pending: false,
                expanded: HashSet::new(),
                restore_expanded: HashSet::new(),
                tasks: HashMap::new(),
                generation: 0,
                message: String::new(),
            },
            hide_clean_repos: cx.global::<crate::settings::Settings>().hide_clean_repos,
            scm_list: ListState::new(0, ListAlignment::Top, theme::SCM_LIST_OVERDRAW),
            search,
            find,
            reloading: Default::default(),
            whole_word_selection: None,
            index: None,
            index_task: None,
            index_cancel: Arc::new(AtomicBool::new(false)),
            index_generation: 0,
            file_task: None,
            file_generation: 0,
            path_prompt_open: false,
            closing: false,
            window_edited: false,
            cursor: None,
            _cursor_observer: None,
            diff: diff_view::DiffPane {
                tab: None,
                title: String::new(),
                fallback: None,
                doc: None,
                raw: None,
                selection: None,
                dragging: false,
                focus: cx.focus_handle(),
                source: None,
                change: None,
                inline: cx.global::<crate::settings::Settings>().diff_inline,
                scroll: UniformListScrollHandle::new(),
                stale: false,
                task: None,
                cancel: Arc::new(AtomicBool::new(false)),
                generation: 0,
            },
            nav: navigation::NavState {
                symbols: None,
                symbols_task: None,
                symbols_cancel: Arc::new(AtomicBool::new(false)),
                symbols_requested: false,
                generation: 0,
                targets: (0, Vec::new()),
                back: Vec::new(),
                forward: Vec::new(),
                task: None,
                pending_place: None,
            },
            // Top / bottom by default: the right side of the window is kept for an agent panel.
            graph: None,
            large: None,
            tab_menu_focus: None,
            closed_tabs: Default::default(),
            pending_tabs: Vec::new(),
            terminals: Default::default(),
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            loading: false,
            refresh_completed: false,
            excluded: 0,
            issues: vec![],
            message: String::new(),
            refresh_task: None,
            watch: workspace_refresh::WatchState {
                subscription: None,
                task: None,
                error: None,
                debouncing: false,
                refresh_pending: false,
                pending_plan: None,
                ignore_cache: Default::default(),
            },
            agent,
            _subscriptions: vec![appearance, activation, settings, frame],
        };
        this.start_watching(window, cx);
        this.refresh_tree(window, cx);
        this.focus_handle.focus(window, cx);
        this.refresh(window, cx);
        if this.agent.visible {
            this.agent_ensure_session();
        }
        // Snapshots an abnormal exit left: first each window takes its folder's, then (one
        // round later, once every window has had its turn) the first takes the rest.
        crate::perf::first_frame(window);
        cx.defer_in(window, |this, window, cx| {
            this.claim_recovery(false, window, cx);
            cx.defer_in(window, |this, window, cx| {
                this.claim_recovery(true, window, cx)
            });
        });
        eprintln!("event=window_opened number={number}");
        this
    }

    fn refresh_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_tree(window, cx);
        if let Some(root) = self.root.clone() {
            self.build_index(root, window, cx);
        }
    }

    /// Lists the root and every expanded folder again, keeping what is expanded.
    fn reload_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.reveal_pending = matches!(self.active, Pane::Document(_));
        self.explorer.generation += 1;
        self.explorer.tasks.clear();
        self.explorer.restore_expanded = std::mem::take(&mut self.explorer.expanded);
        self.explorer.rows.clear();
        self.explorer.edit = None;
        if let Some(root) = self.root.clone() {
            self.explorer.rows.push(TreeRow {
                entry: Entry {
                    path: root.clone(),
                    directory: true,
                    symlink: false,
                },
                depth: 0,
                pending: false,
            });
            self.load_directory(root, window, cx);
        } else {
            self.explorer.message = "点击顶部“打开文件夹”选择工作区；也可单独打开文件".into();
        }
        cx.notify();
    }

    fn load_directory(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.expanded.insert(path.clone());
        self.explorer.message = "正在读取目录…".into();
        let generation = self.explorer.generation;
        let directory = path.clone();
        let show_hidden = self.explorer.show_hidden;
        let job = cx.background_spawn(async move { files::directory(&directory, show_hidden) });
        let key = path.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.explorer.generation != generation {
                    return;
                }
                this.explorer.tasks.remove(&path);
                if !this.explorer.expanded.contains(&path) {
                    return;
                }
                let Some(index) = this
                    .explorer
                    .rows
                    .iter()
                    .position(|row| row.entry.path == path)
                else {
                    return;
                };
                match result {
                    Ok((entries, _hidden)) => {
                        let restore: Vec<_> = entries
                            .iter()
                            .filter(|entry| {
                                entry.directory
                                    && this.explorer.restore_expanded.remove(&entry.path)
                            })
                            .map(|entry| entry.path.clone())
                            .collect();
                        let depth = this.explorer.rows[index].depth + 1;
                        // A new-entry field opened before the listing arrived stays on top.
                        let at = index
                            + 1
                            + usize::from(
                                this.explorer
                                    .rows
                                    .get(index + 1)
                                    .is_some_and(|row| row.pending),
                            );
                        this.explorer.rows.splice(
                            at..at,
                            entries.into_iter().map(|entry| TreeRow {
                                entry,
                                depth,
                                pending: false,
                            }),
                        );
                        this.explorer.message.clear();
                        this.reveal_current_file(window, cx);
                        for path in restore {
                            if !this.explorer.expanded.contains(&path) {
                                this.load_directory(path, window, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.explorer.expanded.remove(&path);
                        this.explorer.message = format!("{}: {error}", path.display());
                        if this.documents.iter().any(|doc| {
                            this.active == Pane::Document(doc.id) && doc.path.starts_with(&path)
                        }) {
                            this.explorer.reveal_pending = false;
                        }
                    }
                }
                this.flush_workspace_refresh(window, cx);
                cx.notify();
            });
        });
        self.explorer.tasks.insert(key, task);
    }

    fn toggle_directory(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.reveal_pending = false;
        let path = self.explorer.rows[index].entry.path.clone();
        if self.explorer.expanded.remove(&path) {
            self.explorer
                .restore_expanded
                .retain(|restore| !restore.starts_with(&path));
            let depth = self.explorer.rows[index].depth;
            let end = self
                .explorer
                .rows
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, row)| row.depth <= depth)
                .map(|(index, _)| index)
                .unwrap_or(self.explorer.rows.len());
            for row in self.explorer.rows.drain(index + 1..end) {
                self.explorer.expanded.remove(&row.entry.path);
                self.explorer.tasks.remove(&row.entry.path);
            }
            self.explorer.tasks.remove(&path);
        } else {
            self.load_directory(path, window, cx);
        }
        self.flush_workspace_refresh(window, cx);
        cx.notify();
    }

    /// Lists an expanded directory again (file watching), keeping expanded subdirectories.
    fn reload_directory(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .explorer
            .rows
            .iter()
            .position(|row| row.entry.path == path)
        else {
            return;
        };
        let depth = self.explorer.rows[index].depth;
        let end = self
            .explorer
            .rows
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, row)| row.depth <= depth)
            .map(|(index, _)| index)
            .unwrap_or(self.explorer.rows.len());
        let mut pending = None;
        for row in self.explorer.rows.drain(index + 1..end) {
            if row.pending {
                pending = Some(row);
                continue;
            }
            if self.explorer.expanded.remove(&row.entry.path) {
                self.explorer
                    .restore_expanded
                    .insert(row.entry.path.clone());
            }
            self.explorer.tasks.remove(&row.entry.path);
        }
        if let Some(row) = pending {
            self.explorer.rows.insert(index + 1, row);
        }
        self.load_directory(path, window, cx);
    }

    /// Builds the quick-open index in the background; events and manual refresh rebuild it.
    fn build_index(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.index_cancel.store(true, Ordering::Relaxed);
        self.index_cancel = Arc::new(AtomicBool::new(false));
        self.index_generation += 1;
        self.index = None;
        let generation = self.index_generation;
        let cancel = self.index_cancel.clone();
        let service = self.service.clone();
        let job = cx.background_spawn(async move {
            let started = Instant::now();
            let list = |dir: &std::path::Path, cancel: &AtomicBool| service.list_files(dir, cancel);
            let index = PathIndex::build(&root, &cancel, &list);
            eprintln!(
                "event=index_built entries={} incomplete={} heap_kb={} seconds={:.3}",
                index.len(),
                index.incomplete,
                index.heap_bytes() / 1024,
                started.elapsed().as_secs_f64()
            );
            index
        });
        self.index_task = Some(cx.spawn_in(window, async move |this, cx| {
            let index = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.index_generation != generation {
                    return;
                }
                this.index_task = None;
                this.index = Some(Arc::new(index));
                // A rebuilt file list rebuilds the symbol index if navigation has been used.
                if this.nav.symbols_requested {
                    this.build_symbol_index(cx);
                }
                this.resume_search(window, cx);
                this.update_quick_open(window, cx);
                this.flush_workspace_refresh(window, cx);
            });
        }));
    }

    /// Changes the settings for every window and reports a failed write in this one.
    fn change_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut crate::settings::Settings),
    ) {
        let saved = crate::settings::update(cx, change);
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = saved.await {
                let _ = this.update(cx, |this, cx| {
                    this.message = format!("设置未能保存：{error}");
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows or hides dot files in every window's Explorer and quick open (⌘⇧.).
    fn toggle_hidden_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = !self.explorer.show_hidden;
        self.change_settings(window, cx, |settings| settings.show_hidden = show);
        self.message = if show {
            "已显示隐藏文件".into()
        } else {
            "已隐藏点文件和点文件夹".into()
        };
        cx.notify();
    }

    /// Editor font size: one pixel per step, `None` resets (⌘= / ⌘- / ⌘0).
    fn zoom(&mut self, step: Option<f32>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::settings::{EDITOR_FONT_DEFAULT, EDITOR_FONT_MAX, EDITOR_FONT_MIN};
        let current = cx.global::<crate::settings::Settings>().editor_font_size;
        let next = step.map_or(EDITOR_FONT_DEFAULT, |step| {
            (current + step).clamp(EDITOR_FONT_MIN, EDITOR_FONT_MAX)
        });
        if next != current {
            self.change_settings(window, cx, |settings| settings.editor_font_size = next);
            self.message = format!("编辑器字号 {next} px");
        }
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn active_editor(&self) -> Option<Entity<EditorState>> {
        match self.active {
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| doc.editor.clone()),
            Pane::Diff => self.diff.fallback.clone(),
            Pane::Welcome | Pane::Graph | Pane::Large => None,
        }
    }

    /// Tracks the active editor's cursor. The editor also notifies on caret blink, so the
    /// workbench only re-renders when the position actually changed.
    fn observe_cursor(&mut self, cx: &mut Context<Self>) {
        let editor = self.active_editor();
        self.cursor = editor.as_ref().map(|editor| {
            let position = editor.read(cx).cursor_position();
            (position.line, position.character)
        });
        self._cursor_observer = editor.map(|editor| {
            cx.observe(&editor, |this: &mut Self, editor, cx| {
                let position = editor.read(cx).cursor_position();
                let cursor = Some((position.line, position.character));
                if this.cursor != cursor {
                    this.cursor = cursor;
                    cx.notify();
                }
            })
        });
    }

    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus_preview(window, cx) {
            return;
        }
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        } else if self.active == Pane::Diff {
            self.diff.focus.focus(window, cx);
        } else {
            self.focus_handle.focus(window, cx);
        }
    }

    fn select_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        if let Pane::Document(previous) = self.active
            && pane != self.active
        {
            self.auto_save_on_focus_change(Some(previous), window, cx);
        }
        self.file_generation += 1;
        self.file_task = None;
        self.active = pane;
        self.explorer.reveal_pending = matches!(pane, Pane::Document(_));
        self.clear_tree_selection_for(pane);
        self.find_update(false, cx);
        if std::mem::take(&mut self.explorer.focus_on_open) {
            self.explorer.focus.focus(window, cx);
        } else {
            self.focus_active_editor(window, cx);
        }
        self.observe_cursor(cx);
        self.reveal_current_file(window, cx);
        self.remember_tabs(false, window, cx);
        cx.notify();
    }

    fn reveal_current_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.explorer.reveal_pending {
            return;
        }
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(path) = self
            .documents
            .iter()
            .find(|doc| doc.id == id)
            .map(|doc| doc.path.clone())
        else {
            return;
        };
        let Some(root) = self.root.clone() else {
            return;
        };
        let Ok(relative) = path.strip_prefix(&root) else {
            return;
        };
        let mut parent = root;
        for component in relative.components() {
            if !self
                .explorer
                .rows
                .iter()
                .any(|row| row.entry.path == parent)
            {
                return;
            }
            if !self.explorer.expanded.contains(&parent) {
                self.load_directory(parent, window, cx);
                return;
            }
            parent.push(component.as_os_str());
        }
        if let Some(index) = self
            .explorer
            .rows
            .iter()
            .position(|row| row.entry.path == path)
        {
            self.explorer
                .scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
            self.explorer.reveal_pending = false;
        }
    }

    fn new_window(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = crate::open_workspace(
            None,
            self.service.clone(),
            self.owners.clone(),
            cx.windows().len(),
            None,
            Default::default(),
            cx,
        ) {
            self.message = format!("无法新建窗口：{error}");
            cx.notify();
        }
    }

    fn choose_path(&mut self, directory: bool, window: &mut Window, cx: &mut Context<Self>) {
        if directory && self.groups.iter().any(|g| g.write_pending) {
            self.message = "Git 操作尚未结束，请稍后切换工作区".into();
            cx.notify();
            return;
        }
        if self.path_prompt_open {
            return;
        }
        self.path_prompt_open = true;
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: !directory,
            directories: directory,
            multiple: false,
            prompt: Some(
                if directory {
                    "打开文件夹"
                } else {
                    "打开文件"
                }
                .into(),
            ),
        });
        cx.spawn_in(window, async move |this, cx| {
            let selected = match answer.await {
                Ok(Ok(paths)) => Ok(paths.and_then(|paths| paths.into_iter().next())),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let selected = if directory {
                match selected {
                    Ok(Some(path)) => {
                        cx.background_spawn(async move {
                            let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
                            if !path.is_dir() {
                                return Err("所选路径不是目录".into());
                            }
                            Ok(Some(path))
                        })
                        .await
                    }
                    result => result,
                }
            } else {
                selected
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.path_prompt_open = false;
                match selected {
                    Ok(Some(path)) if directory => {
                        if this.root.is_none() {
                            window.set_window_title(&format!("ZJ · {}", path.display()));
                            this.root = Some(path);
                            crate::session::remember(window, this.root.as_deref(), true, cx);
                            this.sidebar = Sidebar::Explorer;
                            this.start_watching(window, cx);
                            this.refresh_tree(window, cx);
                            this.refresh(window, cx);
                        } else if let Err(error) = crate::open_workspace(
                            Some(path),
                            this.service.clone(),
                            this.owners.clone(),
                            cx.windows().len(),
                            None,
                            Default::default(),
                            cx,
                        ) {
                            this.message = format!("无法打开工作区：{error}");
                        }
                    }
                    Ok(Some(path)) => this.open_file(path, None, window, cx),
                    Ok(None) => {}
                    Err(error) => this.message = format!("选择失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_file(
        &mut self,
        path: PathBuf,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_generation += 1;
        let generation = self.file_generation;
        self.message = format!("正在打开 {}", path.display());
        let requested = path.clone();
        let job = cx.background_spawn(async move {
            let loaded = files::text_file(root.as_deref(), &path)?;
            let root = root.and_then(|root| std::fs::canonicalize(root).ok());
            let indent = crate::indent::resolve(&loaded.path, root.as_deref(), &loaded.text);
            Ok::<_, std::io::Error>((loaded, indent))
        });
        self.file_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.file_generation != generation {
                    return;
                }
                let (loaded, indent) = match result {
                    Ok(loaded) => loaded,
                    Err(error) if let Some(reason) = files::Restricted::of(&error) => {
                        this.open_large(requested, reason, window, cx);
                        return;
                    }
                    Err(error) => {
                        this.message = format!("打开失败：{error}");
                        cx.notify();
                        return;
                    }
                };
                if this.install_loaded(loaded, indent, window, cx).is_some() {
                    this.apply_pending_place(window, cx);
                }
            });
        }));
        cx.notify();
    }

    /// Opens `path` without the newest-request-wins rule of `open_file` (restoring several
    /// tabs at launch must not cancel each other), then runs `then` with the new tab.
    fn open_in_background(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<DocumentId, String>, &mut Window, &mut Context<Self>)
        + 'static,
    ) {
        let workspace = self.root.clone();
        let requested = path.clone();
        let job = cx.background_spawn(async move {
            // These paths were named explicitly (command line, snapshot, last session): a file
            // outside the folder is read like a file-picker choice, inside it through the
            // folder's confined walk.
            let inside = workspace.as_ref().is_some_and(|root| {
                std::fs::canonicalize(&path).is_ok_and(|path| path.starts_with(root))
            });
            let root = workspace.filter(|_| inside);
            let loaded = files::text_file(root.as_deref(), &path)?;
            let root = root.and_then(|root| std::fs::canonicalize(root).ok());
            let indent = crate::indent::resolve(&loaded.path, root.as_deref(), &loaded.text);
            Ok::<_, std::io::Error>((loaded, indent))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let opened = match result {
                    Ok((loaded, indent)) => this
                        .install_loaded(loaded, indent, window, cx)
                        .ok_or_else(|| "文件已打开，或已达到打开文件的上限".to_string()),
                    // Not editable: shown in the viewer, and the caller still hears why.
                    Err(error) if let Some(reason) = files::Restricted::of(&error) => {
                        this.open_large(requested, reason, window, cx);
                        Err(format!("{error}，已以只读方式打开"))
                    }
                    Err(error) => Err(error.to_string()),
                };
                then(this, opened, window, cx);
            });
        })
        .detach();
    }

    /// Reopens the last session's tabs: the active one now, the others when chosen.
    pub(crate) fn restore_tabs(
        &mut self,
        tabs: Vec<PathBuf>,
        active: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_tabs = tabs
            .into_iter()
            .filter(|tab| Some(tab) != active.as_ref())
            .collect();
        if let Some(active) = active {
            self.open_in_background(active, window, cx, |this, opened, _, cx| {
                if let Err(error) = opened {
                    this.message = format!("没能重新打开上次的标签：{error}");
                    cx.notify();
                }
            });
        }
        cx.notify();
    }

    /// A file from the command line: opened now.
    pub(crate) fn open_now(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_in_background(path, window, cx, |this, opened, _, cx| {
            if let Err(error) = opened {
                this.message = format!("没能打开命令行中的文件：{error}");
                cx.notify();
            }
        });
    }

    /// A restored tab that was not open yet was chosen: read it now.
    pub(super) fn open_pending_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        self.open_file(path, root, window, cx);
    }

    /// × on a restored tab that was never opened.
    pub(super) fn drop_pending_tab(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_tabs.retain(|tab| tab != path);
        self.remember_tabs(true, window, cx);
        cx.notify();
    }

    /// Updates the session record of this window's tabs (written when `persist`).
    fn remember_tabs(&self, persist: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mut tabs: Vec<PathBuf> = self
            .documents
            .iter()
            .filter(|doc| !doc.untitled)
            .map(|doc| doc.path.clone())
            .collect();
        for pending in &self.pending_tabs {
            if !tabs.contains(pending) {
                tabs.push(pending.clone());
            }
        }
        let active = match self.active {
            Pane::Document(id) => self
                .document(id)
                .filter(|doc| !doc.untitled)
                .map(|doc| doc.path.clone()),
            _ => None,
        };
        crate::session::remember_tabs(window, tabs, active, persist, cx);
    }

    /// A file read in the background becomes a tab (or focuses the tab that already has it,
    /// here or in another window). Returns the new tab's document, `None` when the file was
    /// already open or the open-file limit is reached.
    fn install_loaded(
        &mut self,
        loaded: files::TextFile,
        indent: crate::indent::Indent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<DocumentId> {
        let this = self;
        let id = loaded.id;
        if let Some(existing) = this
            .documents
            .iter()
            .find(|doc| doc.id == id || doc.path == loaded.path)
            .map(|doc| doc.id)
        {
            this.select_pane(Pane::Document(existing), window, cx);
            this.message = "已定位到打开的标签；保留缓冲区内容".into();
            this.apply_pending_place(window, cx);
            return None;
        }
        let owner = this
            .owners
            .borrow()
            .iter()
            .find(|(key, owner)| **key == id || owner.path == loaded.path)
            .map(|(id, owner)| (*id, owner.clone()));
        if let Some((existing, owner)) = owner {
            if cx
                .update_window(owner.window, |_, window, cx| {
                    owner
                        .view
                        .update(cx, |this, cx| {
                            this.select_pane(Pane::Document(existing), window, cx)
                        })
                        .map(|_| window.activate_window())
                })
                .is_ok_and(|result| result.is_ok())
            {
                this.message = "文件已在另一窗口打开，已定位到原窗口".into();
                cx.notify();
                return None;
            }
            this.owners.borrow_mut().remove(&existing);
        }
        if this.documents.len() >= 20
            || this.documents.iter().map(|doc| doc.bytes).sum::<usize>() + loaded.bytes
                > files::MAX_OPEN_BYTES
        {
            this.message = "已达到 20 个文件 / 20 MiB 原文的原型上限，请先关闭标签".into();
            cx.notify();
            return None;
        }
        let path_for_pending = loaded.path.clone();
        let (language, language_name) = language_for(&loaded.path);
        let path = loaded.path.clone();
        let (editor, subscription) =
            this.document_editor(id, &path, loaded.text, indent, window, cx);
        this.documents.push(Document {
            indent,
            readonly: loaded.readonly,
            bytes: loaded.bytes,
            language: language_name,
            crlf: loaded.crlf,
            bom: loaded.bom,
            disk: Some(loaded.disk),
            untitled: false,
            markdown: markdown_preview::MarkdownPreview::for_language(language, false, cx),
            ..Document::new(id, path, editor, subscription)
        });
        this.owners.borrow_mut().insert(
            id,
            DocumentOwner {
                path: loaded.path,
                view: cx.weak_entity(),
                window: window.window_handle(),
            },
        );
        this.message.clear();
        this.markdown_refresh(id, cx);
        this.pending_tabs.retain(|tab| *tab != path_for_pending);
        eprintln!("event=document_opened");
        this.select_pane(Pane::Document(id), window, cx);
        this.remember_tabs(true, window, cx);
        Some(id)
    }

    fn remove_document(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        // ⇧⌘T reopens closed files; untitled buffers have nothing to reopen.
        if let Some(doc) = self.documents.iter().find(|doc| doc.id == id)
            && !doc.untitled
        {
            let offset = doc.editor.read(cx).cursor();
            self.closed_tabs.push(tab_menu::ClosedTab {
                path: doc.path.clone(),
                offset,
            });
        }
        // Closed after its edits were saved or discarded: nothing to recover.
        self.forget_snapshot(id, cx);
        self.documents.retain(|doc| doc.id != id);
        self.owners.borrow_mut().remove(&id);
        self.remember_tabs(true, window, cx);
        if self.active == Pane::Document(id) {
            let pane = self
                .documents
                .last()
                .map(|doc| Pane::Document(doc.id))
                .unwrap_or(Pane::Welcome);
            self.select_pane(pane, window, cx);
        }
        cx.notify();
    }

    /// The tab's ×: asks 「要保存对 X 的更改吗？」 for an edited buffer.
    fn close_document(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_close(vec![id], true, window, cx).detach();
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_repos(None, window, cx);
    }

    /// `targets` limits the refresh to a status query of these known repositories (file
    /// watching); `None` rediscovers repositories under the root.
    fn refresh_repos(
        &mut self,
        targets: Option<HashSet<RepoId>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let full = targets.is_none();
        let known: Option<Vec<Repository>> = targets.as_ref().map(|ids| {
            self.groups
                .iter()
                .filter(|g| ids.contains(&g.repo.id))
                .map(|g| g.repo.clone())
                .collect()
        });
        if known.as_ref().is_some_and(Vec::is_empty) {
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        let generation = self.generation;
        let cancel = self.cancel.clone();
        let service = self.service.clone();
        let watch = self.watch.subscription.clone();
        let (sender, receiver) = mpsc::sync_channel(64);
        self.loading = true;
        self.excluded = 0;
        self.diff.stale = self.diff.tab.as_ref().is_some_and(|diff| {
            diff.request().is_some_and(|request| {
                targets
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&request.repo.id))
            })
        });
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut repos = Vec::new();
            let mut discovery_failed = false;
            if let Some(known) = known {
                repos = known;
            } else {
                service.discover(&[root], &cancel, |event| match event {
                    Discovery::Repository(repo) => {
                        // External Git metadata must be watched before its first status query.
                        if let Some(watch) = &watch
                            && let Err(error) = watch.add_repository(&repo)
                        {
                            let _ = sender.send(Event::Issue(format!("Git 目录监听失败：{error}")));
                        }
                        let _ = sender.send(Event::Repo(repo.clone()));
                        repos.push(repo);
                    }
                    Discovery::Issue(path, e) => {
                        discovery_failed = true;
                        let _ = sender.send(Event::Issue(format!("{}: {e}", path.display())));
                    }
                    Discovery::Excluded(_) => {
                        let _ = sender.send(Event::Excluded);
                    }
                    Discovery::Cancelled => {}
                });
                if !discovery_failed
                    && !cancel.load(Ordering::Relaxed)
                    && let Some(watch) = &watch
                {
                    watch.retain_repositories(&repos);
                }
            }
            let count = repos.len();
            let queue = Mutex::new(repos.into_iter());
            std::thread::scope(|scope| {
                for _ in 0..2 {
                    let queue = &queue;
                    let service = &service;
                    let sender = &sender;
                    let cancel = &cancel;
                    scope.spawn(move || {
                        loop {
                            if cancel.load(Ordering::Relaxed) {
                                break;
                            }
                            let Some(repo) = queue.lock().unwrap().next() else {
                                break;
                            };
                            let result = service
                                .status(&repo, generation, cancel)
                                .map_err(|e| e.to_string());
                            let outgoing = match &result {
                                Ok(status) if status.upstream.is_some() && status.ahead > 0 => {
                                    service
                                        .outgoing(&repo, OUTGOING_LIMIT, cancel)
                                        .map_err(|e| e.to_string())
                                }
                                _ => Ok(Vec::new()),
                            };
                            if sender
                                .send(Event::Status(repo.id, result, outgoing))
                                .is_err()
                            {
                                break;
                            }
                        }
                    });
                }
            });
            let _ = sender.send(Event::Done);
            eprintln!(
                "event=refresh_finished generation={generation} full={full} repos={count} seconds={:.3}",
                started.elapsed().as_secs_f64()
            );
        });
        // Only poll while a bounded job is in flight; idle windows have no refresh timer.
        self.refresh_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut seen = std::collections::HashSet::new();
            let mut issues = Vec::new();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let mut done = false;
                let events: Vec<_> = (0..64)
                    .filter_map(|_| match receiver.try_recv() {
                        Ok(e) => Some(e),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            done = true;
                            None
                        }
                        Err(mpsc::TryRecvError::Empty) => None,
                    })
                    .collect();
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.generation != generation {
                            done = true;
                            return;
                        }
                        for event in events {
                            match event {
                                Event::Repo(repo) => {
                                    seen.insert(repo.id.clone());
                                    if !this.groups.iter().any(|g| g.repo.id == repo.id) {
                                        let commit_input = cx.new(|cx| {
                                            TextareaState::new(window, cx)
                                                .auto_grow(1, 6)
                                                .placeholder("消息（⌘Enter 提交）")
                                        });
                                        let subscription = cx.subscribe_in(
                                            &commit_input,
                                            window,
                                            move |_, _, _: &InputEvent, _, cx| cx.notify(),
                                        );
                                        if this.scm_repo.is_none() {
                                            this.scm_repo = Some(repo.id.clone());
                                        }
                                        this.groups.push(Group {
                                            repo,
                                            status: None,
                                            expanded: true,
                                            staged_collapsed: false,
                                            changes_collapsed: false,
                                            commit_input,
                                            _commit_subscription: subscription,
                                            write_task: None,
                                            write_pending: false,
                                            write_message: String::new(),
                                            outgoing: Ok(Vec::new()),
                                            outgoing_collapsed: false,
                                        });
                                    }
                                }
                                Event::Status(id, status, outgoing) => {
                                    if let Some(group) =
                                        this.groups.iter_mut().find(|g| g.repo.id == id)
                                    {
                                        if let Ok(status) = &status {
                                            let branch = status
                                                .branch
                                                .clone()
                                                .unwrap_or_else(|| "未知分支".into());
                                            group.commit_input.update(cx, |input, cx| {
                                                input.set_placeholder(
                                                    format!("消息（⌘Enter 在“{branch}”提交）"),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                        group.status = Some(status.map(Arc::new));
                                        group.outgoing = outgoing;
                                    }
                                }
                                Event::Issue(issue) => {
                                    if issues.len() < 100 {
                                        issues.push(issue);
                                    }
                                }
                                Event::Excluded => this.excluded += 1,
                                Event::Done => {
                                    if full
                                        && issues.is_empty()
                                        && !this.cancel.load(Ordering::Relaxed)
                                    {
                                        this.groups.retain(|g| {
                                            g.write_pending || seen.contains(&g.repo.id)
                                        });
                                    }
                                    done = true;
                                }
                            }
                        }
                        if done {
                            if full || !issues.is_empty() {
                                this.issues = std::mem::take(&mut issues);
                            }
                            this.loading = false;
                            this.refresh_completed = true;
                        }
                        this.rebuild_rows();
                        if done {
                            this.flush_workspace_refresh(window, cx);
                            if this.diff.stale && !this.loading {
                                this.load_diff(window, cx);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                    || done
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn rebuild_rows(&mut self) {
        self.rebuild_decorations();
        self.fill_rows();
        // Keep the scroll position: resize at the tail, then remeasure every row in place.
        let (old, new) = (self.scm_list.item_count(), self.rows.len());
        let kept = old.min(new);
        self.scm_list.splice(kept..old, new - kept);
        self.scm_list.remeasure_items(0..new);
    }

    fn fill_rows(&mut self) {
        self.rows.clear();
        let hide_clean = self.hide_clean_repos;
        // Repositories with changes (or still loading, or failing) first, clean ones after;
        // each part by path.
        let mut active: Vec<usize> = (0..self.groups.len())
            .filter(|g| !self.groups[*g].clean())
            .collect();
        let mut clean: Vec<usize> = (0..self.groups.len())
            .filter(|g| self.groups[*g].clean() && !hide_clean)
            .collect();
        let by_path = |a: &usize, b: &usize| {
            self.groups[*a]
                .repo
                .worktree
                .cmp(&self.groups[*b].repo.worktree)
        };
        active.sort_by(by_path);
        clean.sort_by(by_path);
        let order: Vec<usize> = active.into_iter().chain(clean).collect();
        for g in order {
            let group = &self.groups[g];
            self.rows.push(Row::Group(g));
            if !group.expanded || !group.expandable() {
                continue;
            }
            if let Some(Ok(status)) = &group.status {
                self.rows.push(Row::Commit(g));
                for side in [DiffSide::Staged, DiffSide::Worktree] {
                    let changes: Vec<_> = status
                        .changes
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| match side {
                            DiffSide::Staged => c.staged(),
                            DiffSide::Worktree => c.unstaged(),
                        })
                        .map(|(i, _)| i)
                        .collect();
                    if !changes.is_empty() {
                        self.rows.push(Row::Heading(g, side, changes.len()));
                    }
                    let collapsed = match side {
                        DiffSide::Staged => group.staged_collapsed,
                        DiffSide::Worktree => group.changes_collapsed,
                    };
                    if !collapsed {
                        self.rows
                            .extend(changes.into_iter().map(|i| Row::File(g, i, side)));
                    }
                }
            }
            let ahead = group.ahead();
            if ahead > 0 {
                self.rows.push(Row::Outgoing(g));
                if !group.outgoing_collapsed {
                    let listed = group.outgoing.as_ref().map_or(0, Vec::len);
                    self.rows
                        .extend((0..listed).map(|i| Row::OutgoingCommit(g, i)));
                    if group.outgoing.is_err() || listed < ahead {
                        self.rows.push(Row::OutgoingNote(g));
                    }
                }
            }
        }
    }

    /// Files keep their own decoration; each ancestor folder up to the workspace root takes the
    /// most severe decoration below it, as VS Code does.
    fn rebuild_decorations(&mut self) {
        self.explorer.decorations.clear();
        let root = self.root.clone();
        for group in &self.groups {
            let Some(Ok(status)) = &group.status else {
                continue;
            };
            for change in &status.changes {
                let decoration = decoration(change);
                let path = group.repo.worktree.join(&change.path);
                let mut ancestor = path.parent();
                self.explorer.decorations.insert(path.clone(), decoration);
                while let Some(folder) = ancestor {
                    if root.as_ref().is_some_and(|root| !folder.starts_with(root)) {
                        break;
                    }
                    let entry = self
                        .explorer
                        .decorations
                        .entry(folder.to_path_buf())
                        .or_insert(decoration);
                    if decoration.kind > entry.kind {
                        *entry = decoration;
                    }
                    if root.as_deref() == Some(folder) {
                        break;
                    }
                    ancestor = folder.parent();
                }
            }
        }
    }

    fn collapse_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.explorer.reveal_pending = false;
        let Some(root) = self.explorer.rows.first().map(|row| row.entry.path.clone()) else {
            return;
        };
        self.explorer.rows.retain(|row| row.depth <= 1);
        self.explorer.expanded.retain(|path| *path == root);
        self.explorer.restore_expanded.clear();
        self.explorer.tasks.clear();
        self.flush_workspace_refresh(window, cx);
        cx.notify();
    }

    fn close_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_generation += 1;
        self.file_task = None;
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.diff.generation += 1;
        self.diff.task = None;
        self.diff.fallback = None;
        self.diff.doc = None;
        self.diff.raw = None;
        self.diff.selection = None;
        self.diff.source = None;
        self.diff.change = None;
        self.diff.scroll = UniformListScrollHandle::new();
        self.diff.title.clear();
        self.diff.tab = None;
        self.diff.stale = false;
        if self.active == Pane::Diff {
            self.active = self
                .documents
                .last()
                .map(|doc| Pane::Document(doc.id))
                .unwrap_or(Pane::Welcome);
            self.focus_active_editor(window, cx);
        }
        cx.notify();
    }

    fn open_diff(
        &mut self,
        group: usize,
        index: usize,
        side: DiffSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let g = &self.groups[group];
        let Some(Ok(status)) = &g.status else {
            return;
        };
        let change = &status.changes[index];
        let request = Request {
            repo: g.repo.clone(),
            generation: self.generation,
            operation: if change.kind == ChangeKind::Untracked {
                Operation::UntrackedDiff {
                    path: change.path.clone(),
                }
            } else {
                Operation::Diff {
                    side,
                    path: change.path.clone(),
                    original_path: change.original_path.clone(),
                }
            },
        };
        let diff_tab = DiffTab {
            label: format!(
                "{} ({})",
                change
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                match side {
                    DiffSide::Staged => "索引",
                    DiffSide::Worktree => "工作树",
                }
            ),
            tooltip: g.repo.worktree.join(&change.path).display().to_string(),
            path: g.repo.worktree.join(&change.path),
            source: DiffSource::Git(request),
        };
        self.close_preview(window, cx);
        self.message.clear();
        self.diff.tab = Some(diff_tab);
        self.active = Pane::Diff;
        self.focus_handle.focus(window, cx);
        self.load_diff(window, cx);
    }

    /// Requery the selected comparison without discarding its last completed view.
    fn load_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(diff) = &self.diff.tab else {
            return;
        };
        if diff.agent().is_some() {
            self.load_agent_diff(window, cx);
            return;
        }
        let mut source = diff.source.clone();
        let title = diff.label.clone();
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.diff.generation += 1;
        if let DiffSource::Git(request) = &mut source {
            request.generation = self.diff.generation;
        }
        self.diff.stale = false;
        self.diff.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.diff.cancel.clone();
        let service = self.service.clone();
        let version = self.diff.generation;
        self.diff.title = format!("正在加载 {title}");
        let highlight = gpui_kit::component::Theme::global(cx)
            .highlight_theme
            .clone();
        let dark = gpui_kit::component::Theme::global(cx).is_dark();
        let colors = theme::colors(cx);
        let change_colors = crate::diff_doc::ChangeColors {
            inserted_text: colors.diff_added_text,
            removed_text: colors.diff_deleted_text,
        };
        let language = match language_for(&diff.path).0 {
            "plain" => None,
            language => Some(language),
        };
        let job = cx.background_spawn(async move {
            let text: Arc<str> = match source {
                DiffSource::Git(request) => {
                    let reply = service.execute(&request, &cancel)?;
                    if reply.repo != request.repo.id || reply.generation != request.generation {
                        return Err(std::io::Error::other("Diff 结果的仓库或版本不匹配"));
                    }
                    String::from_utf8(reply.output)
                        .map_err(std::io::Error::other)?
                        .into()
                }
                DiffSource::Local(patch) => patch,
                // Agent reviews load in `load_agent_diff`.
                DiffSource::Agent(_) => return Err(std::io::Error::other("Agent 审阅不经过 Git")),
            };
            let doc = crate::diff_doc::DiffDoc::parse(&text, language, &highlight, change_colors)
                .map(Arc::new);
            let raw = crate::partial_patch::parse(&text).map(Arc::new);
            Ok::<_, std::io::Error>((text, doc, raw))
        });
        self.diff.task =
            Some(cx.spawn_in(window, async move |this, cx| {
                let result = job.await;
                let _ =
                    this.update_in(cx, |this, window, cx| {
                        if this.diff.generation != version {
                            return;
                        }
                        let restore_focus = this.active == Pane::Diff
                            && (this.focus_handle.is_focused(window)
                                || this.diff.fallback.as_ref().is_some_and(|editor| {
                                    editor
                                        .read(cx)
                                        .focus_handle(cx)
                                        .contains_focused(window, cx)
                                }));
                        match result {
                            Ok((text, ..)) if text.is_empty() => {
                                this.diff.fallback = None;
                                this.diff.doc = None;
                                this.diff.source = None;
                                this.diff.title = "当前没有差异".into();
                            }
                            Ok((text, doc, raw)) => {
                                // Equal patches keep the scroll position through a refresh.
                                if this.diff.source.as_ref().is_some_and(|(old, was_dark)| {
                                    *old == text && *was_dark == dark
                                }) {
                                    this.diff.title = title;
                                    cx.notify();
                                    return;
                                }
                                let fresh = this.diff.source.is_none();
                                this.diff.source = Some((text.clone(), dark));
                                this.diff.raw = raw;
                                this.diff.selection = None;
                                this.diff.title = title;
                                match doc {
                                    Some(doc) => {
                                        this.diff.fallback = None;
                                        this.diff.doc = Some(doc);
                                        if fresh {
                                            this.reveal_first_change();
                                        }
                                    }
                                    None => {
                                        this.diff.doc = None;
                                        this.diff.fallback = Some(cx.new(|cx| {
                                            EditorState::new(window, cx)
                                                .language(crate::diff_syntax::LANGUAGE)
                                                .default_value(text.to_string())
                                        }));
                                    }
                                }
                            }
                            Err(e) => {
                                this.diff.fallback = None;
                                this.diff.doc = None;
                                this.diff.source = None;
                                this.diff.title = format!("Diff 加载失败: {e}");
                            }
                        }
                        if this.active == Pane::Diff {
                            this.observe_cursor(cx);
                            if restore_focus {
                                this.focus_active_editor(window, cx);
                            }
                        }
                        cx.notify();
                    });
            }));
        cx.notify();
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        // macOS: the dot in the window's close button.
        let edited = self.documents.iter().any(|doc| doc.dirty);
        if edited != self.window_edited {
            self.window_edited = edited;
            window.set_window_edited(edited);
        }
        let mut editor = self.render_editor_area(cx);
        if self.terminals.is_shown() {
            editor = v_resizable("editor-terminal")
                .child(resizable_panel().child(editor))
                .child(
                    resizable_panel()
                        .size(theme::TERMINAL_HEIGHT)
                        .size_range(theme::TERMINAL_MIN..theme::TERMINAL_MAX)
                        .child(self.render_terminal_panel(cx)),
                )
                .into_any_element();
        }
        let agent_panel = self.agent.visible.then(|| self.render_agent_panel(cx));
        let workbench = if self.sidebar_visible || agent_panel.is_some() {
            // One layout per combination, so a hidden panel does not leave its size behind.
            let id = match (self.sidebar_visible, agent_panel.is_some()) {
                (true, true) => "workbench-panels-agent",
                (false, true) => "workbench-editor-agent",
                _ => "workbench-panels",
            };
            let mut group = h_resizable(id);
            if self.sidebar_visible {
                group = group.child(
                    resizable_panel()
                        .size(theme::SIDEBAR_WIDTH)
                        .size_range(theme::SIDEBAR_MIN..theme::SIDEBAR_MAX)
                        .child(self.render_sidebar(cx)),
                );
            }
            group = group.child(
                resizable_panel()
                    .size_range(theme::EDITOR_MIN..theme::EDITOR_MAX)
                    .child(editor),
            );
            if let Some(panel) = agent_panel {
                let weak = cx.weak_entity();
                group = group
                    .child(
                        resizable_panel()
                            .size(self.agent.width)
                            .size_range(theme::AGENT_PANEL_MIN..theme::AGENT_PANEL_MAX)
                            .child(panel),
                    )
                    .on_resize(move |state, window, cx| {
                        let Some(width) = state.read(cx).sizes().last().copied() else {
                            return;
                        };
                        let _ = weak.update(cx, |this, cx| this.agent_resized(width, window, cx));
                    });
            }
            group.into_any_element()
        } else {
            editor
        };
        v_flex()
            .size_full()
            .relative()
            .key_context("WorkspaceEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &NewWindow, _, cx| this.new_window(cx)))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.choose_path(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| {
                this.choose_path(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save_active(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.save_active_as(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAll, window, cx| {
                this.save_all(window, cx).detach()
            }))
            .on_action(cx.listener(|this, _: &NewUntitled, window, cx| {
                this.new_untitled(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseEditor, window, cx| this.close_editor(window, cx)))
            .on_action(cx.listener(|this, _: &ReopenClosedEditor, window, cx| {
                this.reopen_closed_tab(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleTerminal, window, cx| {
                this.toggle_terminal(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| this.new_terminal(window, cx)))
            .on_action(cx.listener(|this, _: &SplitTerminal, window, cx| {
                this.split_terminal(window, cx)
            }))
            .on_action(cx.listener(|this, _: &KillTerminal, window, cx| {
                this.kill_terminal(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleMarkdownPreview, window, cx| {
                this.toggle_markdown_preview(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseOtherEditors, window, cx| {
                this.active_pane_action(Workbench::close_other_panes, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseAllEditors, window, cx| {
                this.close_all_panes(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyActivePath, window, cx| {
                this.active_pane_action(|this, pane, _, cx| this.copy_pane_path(pane, false, cx), window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyActiveRelativePath, window, cx| {
                this.active_pane_action(|this, pane, _, cx| this.copy_pane_path(pane, true, cx), window, cx)
            }))
            .on_action(cx.listener(|this, _: &RevealActiveInFinder, window, cx| {
                this.active_pane_action(|this, pane, _, cx| this.reveal_pane_in_finder(pane, cx), window, cx)
            }))
            .on_action(cx.listener(|this, _: &RevealActiveInExplorer, window, cx| {
                this.active_pane_action(Workbench::reveal_pane_in_explorer, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLineEnding, window, cx| {
                this.toggle_line_ending(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSoftWrap, window, cx| {
                this.toggle_soft_wrap(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleLineComment, window, cx| {
                this.toggle_line_comment(window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLinesUp, window, cx| {
                this.move_lines(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &MoveLinesDown, window, cx| {
                this.move_lines(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyLinesUp, window, cx| {
                this.copy_lines(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyLinesDown, window, cx| {
                this.copy_lines(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| {
                this.select_next_occurrence(cx)
            }))
            .on_action(cx.listener(|this, _: &AutoSaveOff, window, cx| {
                this.set_auto_save(crate::save::AutoSave::Off, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AutoSaveAfterDelay, window, cx| {
                this.set_auto_save(crate::save::AutoSave::AfterDelay, window, cx)
            }))
            .on_action(cx.listener(|this, _: &AutoSaveOnFocusChange, window, cx| {
                this.set_auto_save(crate::save::AutoSave::OnFocusChange, window, cx)
            }))
            .on_action(cx.listener(|this, _: &QuickOpenFile, window, cx| {
                this.open_quick_open(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowAllCommands, window, cx| {
                this.open_quick_open_with(">", window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| this.zoom(Some(1.), window, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomOut, window, cx| this.zoom(Some(-1.), window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomReset, window, cx| this.zoom(None, window, cx)))
            .on_action(
                cx.listener(|this, _: &FindInFiles, window, cx| this.find_in_files(window, cx)),
            )
            // Kit's editor leaves ⌘F to its host when it is not `searchable`.
            .on_action(cx.listener(|this, _: &gpui_kit::component::input::Search, window, cx| {
                this.open_find(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &gpui_kit::component::input::Replace, window, cx| {
                this.open_find(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &find_widget::FindInFile, window, cx| {
                this.open_find(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &find_widget::FindReplace, window, cx| {
                this.open_find(true, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &find_widget::FindNext, _, cx| this.find_step(true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &find_widget::FindPrevious, _, cx| this.find_step(false, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleHiddenFiles, window, cx| {
                this.toggle_hidden_files(window, cx)
            }))
            .on_action(cx.listener(|this, _: &navigation::GoToSymbol, window, cx| {
                this.go_to_symbol(window, cx)
            }))
            .on_action(cx.listener(|this, _: &navigation::GoToLine, window, cx| {
                this.open_quick_open_with(":", window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &navigation::FindReferences, window, cx| {
                    this.find_references(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &navigation::NavigateBack, window, cx| {
                    this.navigate_back(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &navigation::NavigateForward, window, cx| {
                    this.navigate_forward(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleAgentPanel, window, cx| {
                this.toggle_agent_panel(window, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchSessions, window, cx| {
                this.agent_open_search(window, cx)
            }))
            .on_action(cx.listener(|this, _: &AddSelectionToAgent, window, cx| {
                this.agent_add_selection(window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextApproval, window, cx| {
                this.agent_next_approval(window, cx)
            }))
            .bg(colors.editor)
            .text_color(colors.foreground)
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().w_full().child(workbench))
            .child(self.render_status_bar(cx))
            .children(self.render_quick_open(cx))
            .children(self.render_agent_search(cx))
    }
}
