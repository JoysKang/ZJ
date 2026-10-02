use crate::files::{self, Entry, PathIndex, SearchResults};
use crate::{theme, watch};
use gpui_kit::{
    component::{
        input::{EditorState, InputEvent, InputState, TextareaState},
        resizable::{h_resizable, resizable_panel},
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
    ChangeKind, DiffSide, Discovery, GitService, Operation, Request, Status, WriteOperation,
    WriteRequest,
};

mod chrome;
mod diff_ops;
pub use diff_ops::{CopyDiff, SelectAllDiff};
mod diff_view;
mod editor_area;
pub mod navigation;
mod quick_open;
mod scm;
mod scm_actions;
mod sidebar;
mod workspace_refresh;

gpui_kit::actions!(
    workspace,
    [
        SaveUnavailable,
        NewWindow,
        OpenFile,
        OpenFolder,
        QuickOpenFile,
        ToggleSidebar,
        ZoomIn,
        ZoomOut,
        ZoomReset
    ]
);

#[derive(Clone)]
pub struct DocumentOwner {
    path: PathBuf,
    view: WeakEntity<Prototype>,
    window: AnyWindowHandle,
}
pub type DocumentOwners = Rc<RefCell<HashMap<DocumentId, DocumentOwner>>>;

#[derive(Clone, Copy, PartialEq)]
enum Sidebar {
    Explorer,
    Search,
    SourceControl,
}

#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Welcome,
    Document(DocumentId),
    Diff,
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
    _subscription: Subscription,
}

struct DiffTab {
    label: String,
    tooltip: String,
    path: PathBuf,
    request: Request,
}

struct TreeRow {
    entry: Entry,
    depth: usize,
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
}
/// Line breaks in file names would break single-line rows; they are shown as ⏎.
const SINGLE_LINE: [char; 2] = ['\n', '\r'];

/// Highlighter language (only grammars compiled into Kit) and status bar display name.
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
    Heading(usize, DiffSide, usize),
    File(usize, usize, DiffSide),
}
enum Event {
    Repo(Repository),
    Status(RepoId, Result<Status, String>),
    Issue(String),
    Excluded,
    Done,
}

pub struct Prototype {
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
    active: Pane,
    documents: Vec<Document>,
    owners: DocumentOwners,
    tree: Vec<TreeRow>,
    explorer_collapsed: bool,
    decorations: HashMap<PathBuf, Decoration>,
    tree_scroll: UniformListScrollHandle,
    /// Source Control rows vary in height (repository rows wrap long branch names), so they
    /// use a measured list instead of `uniform_list`.
    scm_list: ListState,
    reveal_pending: bool,
    expanded: HashSet<PathBuf>,
    restore_expanded: HashSet<PathBuf>,
    tree_tasks: HashMap<PathBuf, Task<()>>,
    tree_generation: u64,
    tree_message: String,
    search_input: Entity<InputState>,
    search_results: SearchResults,
    search_task: Option<Task<()>>,
    search_generation: u64,
    searching: bool,
    index: Option<Arc<PathIndex>>,
    index_task: Option<Task<()>>,
    index_cancel: Arc<AtomicBool>,
    index_generation: u64,
    file_task: Option<Task<()>>,
    file_generation: u64,
    path_prompt_open: bool,
    closing: bool,
    /// Cursor (line, column) of the active editor, 0-based, for the status bar.
    cursor: Option<(u32, u32)>,
    _cursor_observer: Option<Subscription>,
    preview: Option<Entity<EditorState>>,
    /// Workspace symbols for cross-file go to definition, built on first use.
    symbols: Option<Arc<crate::symbol_index::SymbolIndex>>,
    symbols_task: Option<Task<()>>,
    symbols_cancel: Arc<AtomicBool>,
    symbols_requested: bool,
    nav_generation: u64,
    nav_targets: (u64, Vec<navigation::Target>),
    nav_back: Vec<navigation::NavPoint>,
    nav_forward: Vec<navigation::NavPoint>,
    nav_task: Option<Task<()>>,
    pending_place: Option<(PathBuf, navigation::Placement)>,
    /// The parsed diff editor document; `preview` is only Git's raw text when parsing fails.
    diff_doc: Option<Arc<crate::diff_doc::DiffDoc>>,
    /// Git's own patch lines, for copying exact text and staging selected lines.
    diff_raw: Option<Arc<crate::partial_patch::RawPatch>>,
    diff_selection: Option<diff_ops::DiffSelection>,
    diff_dragging: bool,
    diff_focus: FocusHandle,
    /// Patch text and dark mode the document was built from; equal reloads keep the view.
    diff_source: Option<(Arc<str>, bool)>,
    diff_change: Option<usize>,
    diff_inline: bool,
    diff_scroll: UniformListScrollHandle,
    preview_title: String,
    preview_diff: Option<DiffTab>,
    preview_stale: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    loading: bool,
    refresh_completed: bool,
    excluded: usize,
    issues: Vec<String>,
    message: String,
    refresh_task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    preview_cancel: Arc<AtomicBool>,
    preview_generation: u64,
    watch: Option<Arc<watch::Subscription>>,
    watch_task: Option<Task<()>>,
    watch_error: Option<String>,
    watch_debouncing: bool,
    /// A full refresh (rediscovery, tree and index rebuild) is pending.
    workspace_refresh_pending: bool,
    /// Partial refresh from file watching, applied when the window is not busy.
    pending_plan: Option<crate::refresh_plan::Plan>,
    ignore_cache: Arc<Mutex<crate::refresh_plan::IgnoreCache>>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for Prototype {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.preview_cancel.store(true, Ordering::Relaxed);
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

impl Prototype {
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
            if this.preview_diff.is_some() {
                this.load_diff(window, cx);
            }
        });
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() && this.root.is_some() {
                this.refresh_on_activation(window, cx);
            }
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
                if this.closing {
                    return false;
                }
                this.closing = true;
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "放弃未保存的编辑内容？",
                    Some("此原型的编辑不会写入磁盘，关闭后不保留。选择继续编辑可保留当前缓冲区。"),
                    &["继续编辑", "放弃并关闭"],
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    let discard = answer.await == Ok(1);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.closing = false;
                        if discard {
                            window.remove_window();
                        }
                        cx.notify();
                    });
                })
                .detach();
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
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("按文件名或路径搜索"));
        let search_subscription = cx.subscribe_in(
            &search_input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search_files(window, cx);
                }
            },
        );
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
            active: Pane::Welcome,
            documents: Vec::new(),
            owners,
            tree: Vec::new(),
            explorer_collapsed: false,
            decorations: HashMap::new(),
            tree_scroll: UniformListScrollHandle::new(),
            scm_list: ListState::new(0, ListAlignment::Top, px(200.)),
            reveal_pending: false,
            expanded: HashSet::new(),
            restore_expanded: HashSet::new(),
            tree_tasks: HashMap::new(),
            tree_generation: 0,
            tree_message: String::new(),
            search_input,
            search_results: SearchResults::default(),
            search_task: None,
            search_generation: 0,
            searching: false,
            index: None,
            index_task: None,
            index_cancel: Arc::new(AtomicBool::new(false)),
            index_generation: 0,
            file_task: None,
            file_generation: 0,
            path_prompt_open: false,
            closing: false,
            cursor: None,
            _cursor_observer: None,
            preview: None,
            symbols: None,
            symbols_task: None,
            symbols_cancel: Arc::new(AtomicBool::new(false)),
            symbols_requested: false,
            nav_generation: 0,
            nav_targets: (0, Vec::new()),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            nav_task: None,
            pending_place: None,
            diff_doc: None,
            diff_raw: None,
            diff_selection: None,
            diff_dragging: false,
            diff_focus: cx.focus_handle(),
            diff_source: None,
            diff_change: None,
            diff_inline: false,
            diff_scroll: UniformListScrollHandle::new(),
            preview_title: String::new(),
            preview_diff: None,
            preview_stale: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            loading: false,
            refresh_completed: false,
            excluded: 0,
            issues: vec![],
            message: String::new(),
            refresh_task: None,
            preview_task: None,
            preview_cancel: Arc::new(AtomicBool::new(false)),
            preview_generation: 0,
            watch: None,
            watch_task: None,
            watch_error: None,
            watch_debouncing: false,
            workspace_refresh_pending: false,
            pending_plan: None,
            ignore_cache: Default::default(),
            _subscriptions: vec![search_subscription, appearance, activation],
        };
        this.start_watching(window, cx);
        this.refresh_tree(window, cx);
        this.focus_handle.focus(window, cx);
        this.refresh(window, cx);
        eprintln!("event=window_opened number={number}");
        this
    }

    fn refresh_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pending = matches!(self.active, Pane::Document(_));
        self.tree_generation += 1;
        self.tree_tasks.clear();
        self.restore_expanded = std::mem::take(&mut self.expanded);
        self.tree.clear();
        if let Some(root) = self.root.clone() {
            self.tree.push(TreeRow {
                entry: Entry {
                    path: root.clone(),
                    directory: true,
                    symlink: false,
                },
                depth: 0,
            });
            self.load_directory(root.clone(), window, cx);
            self.build_index(root, window, cx);
        } else {
            self.tree_message = "点击顶部“打开文件夹”选择工作区；也可单独打开文件".into();
        }
        cx.notify();
    }

    fn load_directory(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.expanded.insert(path.clone());
        self.tree_message = "正在读取目录…".into();
        let generation = self.tree_generation;
        let directory = path.clone();
        let job = cx.background_spawn(async move { files::directory(&directory) });
        let key = path.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.tree_generation != generation {
                    return;
                }
                this.tree_tasks.remove(&path);
                if !this.expanded.contains(&path) {
                    return;
                }
                let Some(index) = this.tree.iter().position(|row| row.entry.path == path) else {
                    return;
                };
                match result {
                    Ok(entries) => {
                        let restore: Vec<_> = entries
                            .iter()
                            .filter(|entry| {
                                entry.directory && this.restore_expanded.remove(&entry.path)
                            })
                            .map(|entry| entry.path.clone())
                            .collect();
                        let depth = this.tree[index].depth + 1;
                        this.tree.splice(
                            index + 1..index + 1,
                            entries.into_iter().map(|entry| TreeRow { entry, depth }),
                        );
                        this.tree_message.clear();
                        this.reveal_current_file(window, cx);
                        for path in restore {
                            if !this.expanded.contains(&path) {
                                this.load_directory(path, window, cx);
                            }
                        }
                    }
                    Err(error) => {
                        this.expanded.remove(&path);
                        this.tree_message = format!("{}: {error}", path.display());
                        if this.documents.iter().any(|doc| {
                            this.active == Pane::Document(doc.id) && doc.path.starts_with(&path)
                        }) {
                            this.reveal_pending = false;
                        }
                    }
                }
                this.flush_workspace_refresh(window, cx);
                cx.notify();
            });
        });
        self.tree_tasks.insert(key, task);
    }

    fn toggle_directory(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pending = false;
        let path = self.tree[index].entry.path.clone();
        if self.expanded.remove(&path) {
            self.restore_expanded
                .retain(|restore| !restore.starts_with(&path));
            let depth = self.tree[index].depth;
            let end = self
                .tree
                .iter()
                .enumerate()
                .skip(index + 1)
                .find(|(_, row)| row.depth <= depth)
                .map(|(index, _)| index)
                .unwrap_or(self.tree.len());
            for row in self.tree.drain(index + 1..end) {
                self.expanded.remove(&row.entry.path);
                self.tree_tasks.remove(&row.entry.path);
            }
            self.tree_tasks.remove(&path);
        } else {
            self.load_directory(path, window, cx);
        }
        self.flush_workspace_refresh(window, cx);
        cx.notify();
    }

    /// Lists an expanded directory again (file watching), keeping expanded subdirectories.
    fn reload_directory(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tree.iter().position(|row| row.entry.path == path) else {
            return;
        };
        let depth = self.tree[index].depth;
        let end = self
            .tree
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, row)| row.depth <= depth)
            .map(|(index, _)| index)
            .unwrap_or(self.tree.len());
        for row in self.tree.drain(index + 1..end) {
            if self.expanded.remove(&row.entry.path) {
                self.restore_expanded.insert(row.entry.path.clone());
            }
            self.tree_tasks.remove(&row.entry.path);
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
                if this.symbols_requested {
                    this.build_symbol_index(cx);
                }
                this.search_files(window, cx);
                this.update_quick_open(window, cx);
                this.flush_workspace_refresh(window, cx);
            });
        }));
    }

    fn search_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation += 1;
        self.search_task = None;
        let query = self.search_input.read(cx).value().to_string();
        self.searching = false;
        if self.root.is_none() || query.trim().is_empty() {
            self.search_results = SearchResults::default();
            cx.notify();
            return;
        }
        self.searching = true;
        // Until the index is ready, keep the previous results; build_index re-runs the search.
        let Some(index) = self.index.clone() else {
            cx.notify();
            return;
        };
        let generation = self.search_generation;
        self.search_task = Some(cx.spawn_in(window, async move |this, cx| {
            // Coalesce bursts of typing; matching itself is in memory and cheap.
            cx.background_executor()
                .timer(Duration::from_millis(30))
                .await;
            let result = cx
                .background_spawn(async move { index.search(&query) })
                .await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.search_generation == generation {
                    this.searching = false;
                    this.search_results = result;
                    cx.notify();
                }
            });
        }));
        cx.notify();
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
            Pane::Diff => self.preview.clone(),
            Pane::Welcome => None,
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
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        } else if self.active == Pane::Diff {
            self.diff_focus.focus(window, cx);
        } else {
            self.focus_handle.focus(window, cx);
        }
    }

    fn select_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.file_generation += 1;
        self.file_task = None;
        self.active = pane;
        self.reveal_pending = matches!(pane, Pane::Document(_));
        self.focus_active_editor(window, cx);
        self.observe_cursor(cx);
        self.reveal_current_file(window, cx);
        cx.notify();
    }

    fn reveal_current_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.reveal_pending {
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
            if !self.tree.iter().any(|row| row.entry.path == parent) {
                return;
            }
            if !self.expanded.contains(&parent) {
                self.load_directory(parent, window, cx);
                return;
            }
            parent.push(component.as_os_str());
        }
        if let Some(index) = self.tree.iter().position(|row| row.entry.path == path) {
            self.tree_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
            self.reveal_pending = false;
        }
    }

    fn new_window(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = crate::open_workspace(
            None,
            self.service.clone(),
            self.owners.clone(),
            cx.windows().len(),
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
                            this.sidebar = Sidebar::Explorer;
                            this.start_watching(window, cx);
                            this.refresh_tree(window, cx);
                            this.refresh(window, cx);
                        } else if let Err(error) = crate::open_workspace(
                            Some(path),
                            this.service.clone(),
                            this.owners.clone(),
                            cx.windows().len(),
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
        let job = cx.background_spawn(async move { files::text_file(root.as_deref(), &path) });
        self.file_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.file_generation != generation {
                    return;
                }
                let loaded = match result {
                    Ok(loaded) => loaded,
                    Err(error) => {
                        this.message = format!("打开失败：{error}");
                        cx.notify();
                        return;
                    }
                };
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
                    return;
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
                        return;
                    }
                    this.owners.borrow_mut().remove(&existing);
                }
                if this.documents.len() >= 20
                    || this.documents.iter().map(|doc| doc.bytes).sum::<usize>() + loaded.bytes
                        > files::MAX_OPEN_BYTES
                {
                    this.message = "已达到 20 个文件 / 20 MiB 原文的原型上限，请先关闭标签".into();
                    cx.notify();
                    return;
                }
                let (language, language_name) = language_for(&loaded.path);
                let path = loaded.path.clone();
                let view = cx.weak_entity();
                let editor = cx.new(|cx| {
                    let mut state = EditorState::new(window, cx)
                        .language(language)
                        .default_value(loaded.text);
                    navigation::attach(&mut state, &path, view);
                    state
                });
                let subscription = cx.subscribe(
                    &editor,
                    move |this: &mut Self, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            if let Some(doc) = this.documents.iter_mut().find(|doc| doc.id == id) {
                                doc.dirty = true;
                            }
                            cx.notify();
                        }
                    },
                );
                this.documents.push(Document {
                    id,
                    path: loaded.path.clone(),
                    editor,
                    dirty: false,
                    readonly: loaded.readonly,
                    bytes: loaded.bytes,
                    language: language_name,
                    crlf: loaded.crlf,
                    bom: loaded.bom,
                    _subscription: subscription,
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
                this.select_pane(Pane::Document(id), window, cx);
                this.apply_pending_place(window, cx);
            });
        }));
        cx.notify();
    }

    fn remove_document(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        self.documents.retain(|doc| doc.id != id);
        self.owners.borrow_mut().remove(&id);
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

    fn close_document(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(document) = self.documents.iter().find(|doc| doc.id == id) else {
            return;
        };
        if !document.dirty {
            self.remove_document(id, window, cx);
            return;
        }
        let name = document
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("放弃「{name}」的未保存修改？"),
            Some("原文件未改动；关闭标签会丢弃当前缓冲区。"),
            &["继续编辑", "放弃并关闭"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update_in(cx, |this, window, cx| this.remove_document(id, window, cx));
            }
        })
        .detach();
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
        let watch = self.watch.clone();
        let (sender, receiver) = mpsc::sync_channel(64);
        self.loading = true;
        self.excluded = 0;
        self.preview_stale = self.preview_diff.as_ref().is_some_and(|diff| {
            targets
                .as_ref()
                .is_none_or(|ids| ids.contains(&diff.request.repo.id))
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
                            if sender.send(Event::Status(repo.id, result)).is_err() {
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
                                                .rows(2)
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
                                        });
                                    }
                                }
                                Event::Status(id, status) => {
                                    if let Some(group) =
                                        this.groups.iter_mut().find(|g| g.repo.id == id)
                                    {
                                        group.status = Some(status.map(Arc::new));
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
                            if this.preview_stale && !this.loading {
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
        for (g, group) in self.groups.iter().enumerate() {
            self.rows.push(Row::Group(g));
            if !group.expanded {
                continue;
            }
            if let Some(Ok(status)) = &group.status {
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
        }
    }

    /// Files keep their own decoration; each ancestor folder up to the workspace root takes the
    /// most severe decoration below it, as VS Code does.
    fn rebuild_decorations(&mut self) {
        self.decorations.clear();
        let root = self.root.clone();
        for group in &self.groups {
            let Some(Ok(status)) = &group.status else {
                continue;
            };
            for change in &status.changes {
                let decoration = decoration(change);
                let path = group.repo.worktree.join(&change.path);
                let mut ancestor = path.parent();
                self.decorations.insert(path.clone(), decoration);
                while let Some(folder) = ancestor {
                    if root.as_ref().is_some_and(|root| !folder.starts_with(root)) {
                        break;
                    }
                    let entry = self
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
        self.reveal_pending = false;
        let Some(root) = self.tree.first().map(|row| row.entry.path.clone()) else {
            return;
        };
        self.tree.retain(|row| row.depth <= 1);
        self.expanded.retain(|path| *path == root);
        self.restore_expanded.clear();
        self.tree_tasks.clear();
        self.flush_workspace_refresh(window, cx);
        cx.notify();
    }

    fn close_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_generation += 1;
        self.file_task = None;
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_generation += 1;
        self.preview_task = None;
        self.preview = None;
        self.diff_doc = None;
        self.diff_raw = None;
        self.diff_selection = None;
        self.diff_source = None;
        self.diff_change = None;
        self.diff_scroll = UniformListScrollHandle::new();
        self.preview_title.clear();
        self.preview_diff = None;
        self.preview_stale = false;
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
            request,
        };
        self.close_preview(window, cx);
        self.message.clear();
        self.preview_diff = Some(diff_tab);
        self.active = Pane::Diff;
        self.focus_handle.focus(window, cx);
        self.load_diff(window, cx);
    }

    /// Requery the selected comparison without discarding its last completed view.
    fn load_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(diff) = &self.preview_diff else {
            return;
        };
        let mut request = diff.request.clone();
        let title = diff.label.clone();
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_generation += 1;
        request.generation = self.preview_generation;
        self.preview_stale = false;
        self.preview_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.preview_cancel.clone();
        let service = self.service.clone();
        let version = self.preview_generation;
        let repo_id = request.repo.id.clone();
        let generation = request.generation;
        self.preview_title = format!("正在加载 {title}");
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
            let reply = service.execute(&request, &cancel)?;
            if reply.repo != repo_id || reply.generation != generation {
                return Err(std::io::Error::other("Diff 结果的仓库或版本不匹配"));
            }
            let text: Arc<str> = String::from_utf8(reply.output)
                .map_err(std::io::Error::other)?
                .into();
            let doc = crate::diff_doc::DiffDoc::parse(&text, language, &highlight, change_colors)
                .map(Arc::new);
            let raw = crate::partial_patch::parse(&text).map(Arc::new);
            Ok::<_, std::io::Error>((text, doc, raw))
        });
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.preview_generation != version {
                    return;
                }
                let restore_focus = this.active == Pane::Diff
                    && (this.focus_handle.is_focused(window)
                        || this.preview.as_ref().is_some_and(|editor| {
                            editor
                                .read(cx)
                                .focus_handle(cx)
                                .contains_focused(window, cx)
                        }));
                match result {
                    Ok((text, ..)) if text.is_empty() => {
                        this.preview = None;
                        this.diff_doc = None;
                        this.diff_source = None;
                        this.preview_title = "当前没有差异".into();
                    }
                    Ok((text, doc, raw)) => {
                        // Equal patches keep the scroll position through a refresh.
                        if this
                            .diff_source
                            .as_ref()
                            .is_some_and(|(old, was_dark)| *old == text && *was_dark == dark)
                        {
                            this.preview_title = title;
                            cx.notify();
                            return;
                        }
                        let fresh = this.diff_source.is_none();
                        this.diff_source = Some((text.clone(), dark));
                        this.diff_raw = raw;
                        this.diff_selection = None;
                        this.preview_title = title;
                        match doc {
                            Some(doc) => {
                                this.preview = None;
                                this.diff_doc = Some(doc);
                                if fresh {
                                    this.reveal_first_change();
                                }
                            }
                            None => {
                                this.diff_doc = None;
                                this.preview = Some(cx.new(|cx| {
                                    EditorState::new(window, cx)
                                        .language(crate::diff_syntax::LANGUAGE)
                                        .default_value(text.to_string())
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        this.preview = None;
                        this.diff_doc = None;
                        this.diff_source = None;
                        this.preview_title = format!("Diff 加载失败: {e}");
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

impl Render for Prototype {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let editor = self.render_editor_area(cx);
        let workbench = if self.sidebar_visible {
            h_resizable("workbench-panels")
                .child(
                    resizable_panel()
                        .size(theme::SIDEBAR_WIDTH)
                        .size_range(theme::SIDEBAR_MIN..theme::SIDEBAR_MAX)
                        .child(self.render_sidebar(cx)),
                )
                .child(
                    resizable_panel()
                        .size_range(theme::EDITOR_MIN..theme::EDITOR_MAX)
                        .child(editor),
                )
                .into_any_element()
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
            .on_action(cx.listener(|this, _: &SaveUnavailable, _, cx| {
                this.message = "尚未支持保存：修改只在内存中，原文件未改动".into();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuickOpenFile, window, cx| {
                this.open_quick_open(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| this.zoom(Some(1.), window, cx)))
            .on_action(
                cx.listener(|this, _: &ZoomOut, window, cx| this.zoom(Some(-1.), window, cx)),
            )
            .on_action(cx.listener(|this, _: &ZoomReset, window, cx| this.zoom(None, window, cx)))
            .on_action(cx.listener(|this, _: &navigation::GoToSymbol, window, cx| {
                this.go_to_symbol(window, cx)
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
            .bg(colors.editor)
            .text_color(colors.foreground)
            .child(self.render_title_bar(cx))
            .child(div().flex_1().min_h_0().w_full().child(workbench))
            .child(self.render_status_bar(cx))
            .children(self.render_quick_open(cx))
    }
}
