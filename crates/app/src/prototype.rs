use crate::files::{self, Entry, PathIndex, SearchResults};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Editor, EditorState, Input, InputEvent, InputState},
        resizable::{h_resizable, resizable_panel},
        text::TextView,
        v_flex,
    },
    prelude::FluentBuilder,
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
    ChangeKind, DiffSide, Discovery, GitService, Operation, Request, Status,
};

mod chrome;
mod quick_open;

gpui_kit::actions!(
    workspace,
    [
        SaveUnavailable,
        OpenFile,
        OpenFolder,
        QuickOpenFile,
        ToggleSidebar
    ]
);

const SAMPLE: &str = "// 临时输入测试，退出不保留。不会写入工作区文件。\n// 试用中文 IME、选区替换、粘贴、⌘Z / ⇧⌘Z、⌘F / ⌘H。\nfn main() {\n    println!(\"你好，工作区！\");\n}\n";
const MARKDOWN: &str = "# Markdown 原生预览\n\n资源与输入原型 · P1\n\n- 标题与列表\n- **加粗**与 `行内代码`\n\n```rust\nfn main() { println!(\"你好\"); }\n```\n\n| 模块 | 状态 |\n| --- | --- |\n| Editor | 输入测试 |\n| Git | 只读 |\n\n本样例不含图片和外链。完整受限图片功能在 P6 验证。\n";

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
    Scratch,
    Document(DocumentId),
    Diff,
    Markdown,
}

struct Document {
    id: DocumentId,
    path: PathBuf,
    editor: Entity<EditorState>,
    dirty: bool,
    readonly: bool,
    bytes: usize,
    _subscription: Subscription,
}

struct TreeRow {
    entry: Entry,
    depth: usize,
}

struct Group {
    repo: Repository,
    status: Option<Result<Status, String>>,
    expanded: bool,
    stale: bool,
}
#[derive(Clone, Copy)]
enum Row {
    Group(usize),
    Heading(DiffSide),
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
    root: Option<PathBuf>,
    service: GitService,
    groups: Vec<Group>,
    rows: Vec<Row>,
    scratch: Entity<EditorState>,
    sidebar: Sidebar,
    sidebar_visible: bool,
    quick_open: Option<quick_open::QuickOpen>,
    active: Pane,
    documents: Vec<Document>,
    owners: DocumentOwners,
    tree: Vec<TreeRow>,
    tree_scroll: UniformListScrollHandle,
    reveal_pending: bool,
    expanded: HashSet<PathBuf>,
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
    dirty: bool,
    closing: bool,
    preview: Option<Entity<EditorState>>,
    preview_title: String,
    generation: u64,
    cancel: Arc<AtomicBool>,
    loading: bool,
    excluded: usize,
    issues: Vec<String>,
    message: String,
    refresh_task: Option<Task<()>>,
    preview_task: Option<Task<()>>,
    preview_cancel: Arc<AtomicBool>,
    preview_generation: u64,
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
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            theme::follow_appearance(Some(window), cx)
        });
        let scratch = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("rust")
                .default_value(SAMPLE)
        });
        let subscription = cx.subscribe(&scratch, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty = true;
                cx.notify();
            }
        });
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| {
                if !this.dirty && !this.documents.iter().any(|document| document.dirty) {
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
                            this.dirty = false;
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
            .unwrap_or_else(|| format!("输入测试 {number}"));
        window.set_window_title(&format!("workspace-editor · {name}"));
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
            root,
            service,
            groups: vec![],
            rows: vec![],
            scratch,
            sidebar: Sidebar::Explorer,
            sidebar_visible: true,
            quick_open: None,
            active: Pane::Scratch,
            documents: Vec::new(),
            owners,
            tree: Vec::new(),
            tree_scroll: UniformListScrollHandle::new(),
            reveal_pending: false,
            expanded: HashSet::new(),
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
            dirty: false,
            closing: false,
            preview: None,
            preview_title: String::new(),
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            loading: false,
            excluded: 0,
            issues: vec![],
            message: "原型编辑不写磁盘；关闭和退出不保留内容".into(),
            refresh_task: None,
            preview_task: None,
            preview_cancel: Arc::new(AtomicBool::new(false)),
            preview_generation: 0,
            _subscriptions: vec![subscription, search_subscription, appearance],
        };
        this.refresh_tree(window, cx);
        this.refresh(window, cx);
        eprintln!("event=window_opened number={number}");
        this
    }

    fn refresh_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pending = matches!(self.active, Pane::Document(_));
        self.tree_generation += 1;
        self.tree_tasks.clear();
        self.expanded.clear();
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
                        let depth = this.tree[index].depth + 1;
                        this.tree.splice(
                            index + 1..index + 1,
                            entries.into_iter().map(|entry| TreeRow { entry, depth }),
                        );
                        this.tree_message.clear();
                        this.reveal_current_file(window, cx);
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
                cx.notify();
            });
        });
        self.tree_tasks.insert(key, task);
    }

    fn toggle_directory(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_pending = false;
        let path = self.tree[index].entry.path.clone();
        if self.expanded.remove(&path) {
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
        cx.notify();
    }

    /// Builds the quick-open path index once per root; the refresh button rebuilds it.
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
                "event=index_built entries={} incomplete={} seconds={:.3}",
                index.len(),
                index.incomplete,
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
                this.index = Some(Arc::new(index));
                this.search_files(window, cx);
                this.update_quick_open(window, cx);
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

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        cx.notify();
    }

    fn active_editor(&self) -> Option<Entity<EditorState>> {
        match self.active {
            Pane::Scratch => Some(self.scratch.clone()),
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| doc.editor.clone()),
            Pane::Diff => self.preview.clone(),
            Pane::Markdown => None,
        }
    }

    fn focus_active_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
    }

    fn select_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        self.file_generation += 1;
        self.file_task = None;
        self.active = pane;
        self.reveal_pending = matches!(pane, Pane::Document(_));
        let editor = match pane {
            Pane::Scratch => Some(self.scratch.clone()),
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| doc.editor.clone()),
            _ => None,
        };
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
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

    fn choose_path(&mut self, directory: bool, window: &mut Window, cx: &mut Context<Self>) {
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
                            window.set_window_title(&format!(
                                "workspace-editor · {}",
                                path.display()
                            ));
                            this.root = Some(path);
                            this.sidebar = Sidebar::Explorer;
                            this.refresh_tree(window, cx);
                            this.refresh(window, cx);
                        } else if cx.windows().len() >= 5 {
                            this.message =
                                "原型最多打开 5 个窗口，请先关闭一个窗口再打开文件夹".into();
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
                let language = match loaded.path.extension().and_then(|ext| ext.to_str()) {
                    Some("rs") => "rust",
                    Some("md") => "markdown",
                    _ => "plain",
                };
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language(language)
                        .default_value(loaded.text)
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
                this.message = "原型编辑不写磁盘；关闭和退出不保留内容".into();
                this.select_pane(Pane::Document(id), window, cx);
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
                .unwrap_or(Pane::Scratch);
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
        let Some(root) = self.root.clone() else {
            return;
        };
        self.close_preview(cx);
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        let generation = self.generation;
        let cancel = self.cancel.clone();
        let service = self.service.clone();
        let (sender, receiver) = mpsc::sync_channel(64);
        self.loading = true;
        self.excluded = 0;
        self.issues.clear();
        for group in &mut self.groups {
            group.stale = true;
        }
        std::thread::spawn(move || {
            let started = Instant::now();
            let mut repos = Vec::new();
            service.discover(&[root], &cancel, |event| match event {
                Discovery::Repository(repo) => {
                    let _ = sender.send(Event::Repo(repo.clone()));
                    repos.push(repo);
                }
                Discovery::Issue(path, e) => {
                    let _ = sender.send(Event::Issue(format!("{}: {e}", path.display())));
                }
                Discovery::Excluded(_) => {
                    let _ = sender.send(Event::Excluded);
                }
                Discovery::Cancelled => {}
            });
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
                "event=refresh_finished generation={generation} seconds={:.3}",
                started.elapsed().as_secs_f64()
            );
        });
        // Only poll while a bounded job is in flight; idle windows have no refresh timer.
        self.refresh_task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut seen = std::collections::HashSet::new();
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
                    .update_in(cx, |this, _, cx| {
                        if this.generation != generation {
                            done = true;
                            return;
                        }
                        for event in events {
                            match event {
                                Event::Repo(repo) => {
                                    seen.insert(repo.id.clone());
                                    if !this.groups.iter().any(|g| g.repo.id == repo.id) {
                                        this.groups.push(Group {
                                            repo,
                                            status: None,
                                            expanded: true,
                                            stale: true,
                                        });
                                    }
                                }
                                Event::Status(id, status) => {
                                    if let Some(group) =
                                        this.groups.iter_mut().find(|g| g.repo.id == id)
                                    {
                                        group.stale = status.is_err();
                                        group.status = Some(status);
                                    }
                                }
                                Event::Issue(issue) => {
                                    if this.issues.len() < 100 {
                                        this.issues.push(issue);
                                    }
                                }
                                Event::Excluded => this.excluded += 1,
                                Event::Done => {
                                    if this.issues.is_empty()
                                        && !this.cancel.load(Ordering::Relaxed)
                                    {
                                        this.groups.retain(|g| seen.contains(&g.repo.id));
                                    }
                                    done = true;
                                }
                            }
                        }
                        if done {
                            this.loading = false;
                        }
                        this.rebuild_rows();
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
                        self.rows.push(Row::Heading(side));
                    }
                    self.rows
                        .extend(changes.into_iter().map(|i| Row::File(g, i, side)));
                }
            }
        }
    }

    fn close_preview(&mut self, cx: &mut Context<Self>) {
        self.file_generation += 1;
        self.file_task = None;
        self.preview_cancel.store(true, Ordering::Relaxed);
        self.preview_generation += 1;
        self.preview_task = None;
        self.preview = None;
        self.preview_title.clear();
        if matches!(self.active, Pane::Diff | Pane::Markdown) {
            self.active = Pane::Scratch;
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
        if matches!(change.kind, ChangeKind::Untracked | ChangeKind::Conflict) {
            self.message = "未跟踪文件内容和冲突视图将在 P5 接入；当前状态已列出".into();
            cx.notify();
            return;
        }
        let request = Request {
            repo: g.repo.clone(),
            generation: self.generation,
            operation: Operation::Diff {
                side,
                path: change.path.clone(),
                original_path: change.original_path.clone(),
            },
        };
        let title = format!("{:?} · {}", side, change.path.display());
        self.close_preview(cx);
        self.preview_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.preview_cancel.clone();
        let service = self.service.clone();
        let version = self.preview_generation;
        let repo_id = request.repo.id.clone();
        let generation = request.generation;
        self.preview_title = format!("正在加载 {title}");
        self.active = Pane::Diff;
        let job = cx.background_spawn(async move { service.execute(&request, &cancel) });
        self.preview_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.preview_generation != version || this.generation != generation {
                    return;
                }
                match result.and_then(|r| {
                    if r.repo != repo_id || r.generation != generation {
                        return Err(std::io::Error::other("Diff 结果的仓库或版本不匹配"));
                    }
                    String::from_utf8(r.output).map_err(std::io::Error::other)
                }) {
                    Ok(text) => {
                        this.preview = Some(cx.new(|cx| {
                            EditorState::new(window, cx)
                                .language("diff")
                                .default_value(text)
                        }));
                        this.preview_title = title;
                    }
                    Err(e) => {
                        this.preview_title = format!("Diff 加载失败: {e}");
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn tree_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.tree[index];
        let entry = &row.entry;
        let path = entry.path.clone();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let directory = entry.directory;
        let expanded = self.expanded.contains(&path);
        let selected = self
            .documents
            .iter()
            .any(|doc| self.active == Pane::Document(doc.id) && doc.path == path);
        let colors = theme::colors(cx);
        h_flex()
            .id(("tree-row", index))
            .h(theme::ROW_HEIGHT)
            .w_full()
            .gap_1()
            .pl(theme::SPACE_2 + theme::TREE_INDENT * row.depth as f32)
            .pr_2()
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .role(Role::Button)
            .aria_label(format!(
                "{} {}",
                if directory { "目录" } else { "文件" },
                path.display()
            ))
            .when(selected, |row| row.bg(colors.selected))
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .child(
                div()
                    .w(theme::TWISTY_WIDTH)
                    .flex_shrink_0()
                    .when(directory, |twisty| twisty.child(chevron(expanded, colors))),
            )
            .child(file_icons::icon(if directory {
                if expanded {
                    file_icons::FOLDER_OPEN
                } else {
                    file_icons::FOLDER
                }
            } else {
                file_icons::for_file(&name)
            }))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .child(format!("{name}{}", if entry.symlink { " ↗" } else { "" })),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if directory {
                    this.toggle_directory(index, window, cx);
                } else {
                    this.open_file(path.clone(), this.root.clone(), window, cx);
                }
            }))
            .into_any_element()
    }

    fn search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let path = self.search_results.paths[index].clone();
        let label = self
            .root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let colors = theme::colors(cx);
        h_flex()
            .id(("search-row", index))
            .h(theme::ROW_HEIGHT)
            .w_full()
            .px_2()
            .gap_2()
            .text_size(theme::TEXT_BODY)
            .overflow_hidden()
            .role(Role::Button)
            .aria_label(format!("打开文件 {label}"))
            .hover(|row| row.bg(colors.hover))
            .child(file_icons::icon(file_icons::for_file(
                &path.file_name().unwrap_or_default().to_string_lossy(),
            )))
            .child(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_file(path.clone(), this.root.clone(), window, cx)
            }))
            .into_any_element()
    }

    fn row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let base = div()
            .id(index)
            .h(theme::ROW_HEIGHT)
            .w_full()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .overflow_hidden()
            .text_size(theme::TEXT_BODY);
        match self.rows[index] {
            Row::Group(g) => {
                let group = &self.groups[g];
                let path = self
                    .root
                    .as_ref()
                    .and_then(|root| group.repo.worktree.strip_prefix(root).ok())
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(&group.repo.worktree);
                let name = path.to_string_lossy();
                let status = match &group.status {
                    None => "待确认".into(),
                    Some(Err(e)) => format!("错误: {e}"),
                    Some(Ok(s)) => format!(
                        "{} · {} 项{}",
                        s.branch.as_deref().unwrap_or("未知分支"),
                        s.changes.len(),
                        if group.stale { " · 陈旧" } else { "" }
                    ),
                };
                base.role(Role::Button)
                    .aria_label(format!("仓库 {name} · {status}"))
                    .bg(colors.panel)
                    .hover(|row| row.bg(colors.hover))
                    .child(chevron(group.expanded, colors))
                    .child(format!("{name} · {status}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.groups[g].expanded = !this.groups[g].expanded;
                        this.rebuild_rows();
                        cx.notify();
                    }))
                    .into_any_element()
            }
            Row::Heading(side) => base
                .pl_4()
                .text_color(cx.theme().muted_foreground)
                .child(match side {
                    DiffSide::Staged => "已暂存",
                    DiffSide::Worktree => "Changes（磁盘）",
                })
                .into_any_element(),
            Row::File(g, i, side) => {
                let Some(Ok(status)) = &self.groups[g].status else {
                    return base.into_any_element();
                };
                let c = &status.changes[i];
                let code = match side {
                    DiffSide::Staged => c.index,
                    DiffSide::Worktree => c.worktree,
                };
                let (state, color) = match (c.kind, code) {
                    (ChangeKind::Conflict, _) => ("!".to_string(), colors.conflict),
                    (ChangeKind::Untracked, _) => ("U".to_string(), colors.untracked),
                    (_, b'A') => ("A".to_string(), colors.added),
                    (_, b'D') => ("D".to_string(), colors.deleted),
                    (_, code) => (char::from(code).to_string(), colors.modified),
                };
                base.role(Role::Button)
                    .aria_label(format!(
                        "{:?} · {} · {}",
                        side,
                        self.groups[g].repo.worktree.display(),
                        c.path.display()
                    ))
                    .pl_4()
                    .hover(|row| row.bg(colors.hover))
                    .child(
                        div()
                            .w(theme::STATUS_GLYPH_WIDTH)
                            .flex_shrink_0()
                            .text_color(color)
                            .child(state),
                    )
                    .child(c.path.display().to_string())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_diff(g, i, side, window, cx)
                    }))
                    .into_any_element()
            }
        }
    }
}

fn chevron(expanded: bool, colors: theme::Colors) -> Icon {
    Icon::new(if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
    .size(theme::ICON_SIZE)
    .text_color(colors.muted)
}

impl Render for Prototype {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let sidebar_title = match self.sidebar {
            Sidebar::Explorer => "资源管理器",
            Sidebar::Search => "搜索",
            Sidebar::SourceControl => "源代码管理",
        };
        let sidebar_content = match self.sidebar {
            Sidebar::Explorer => v_flex()
                .size_full()
                .min_h_0()
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child(self.tree_message.clone()),
                )
                .child(
                    uniform_list(
                        "file-tree",
                        self.tree.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.tree_row(index, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.tree_scroll)
                    .flex_1()
                    .w_full(),
                )
                .into_any_element(),
            Sidebar::Search => v_flex()
                .size_full()
                .min_h_0()
                .gap_2()
                .child(div().px_2().child(Input::new(&self.search_input).small()))
                .child(
                    div()
                        .px_2()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child("模糊匹配文件名与路径 · 当前文件查找 ⌘F"),
                )
                .child(div().px_2().text_size(theme::TEXT_CAPTION).child(
                    if self.searching && self.index.is_none() {
                        "正在建立文件索引…".into()
                    } else if self.searching {
                        "搜索中…".into()
                    } else if self.search_input.read(cx).value().trim().is_empty() {
                        "输入文件名或相对路径".into()
                    } else {
                        format!(
                            "{} 个结果{} · {} 项读取错误",
                            self.search_results.paths.len(),
                            if self.search_results.incomplete {
                                "（部分结果，请缩小搜索范围）"
                            } else {
                                ""
                            },
                            self.search_results.errors
                        )
                    },
                ))
                .child(
                    uniform_list(
                        "file-search",
                        self.search_results.paths.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.search_row(index, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .w_full(),
                )
                .child(
                    div()
                        .px_2()
                        .pb_2()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            "Git 仓库遵循 .gitignore；其他目录跳过依赖与构建目录；不遍历目录链接",
                        ),
                )
                .into_any_element(),
            Sidebar::SourceControl => v_flex()
                .size_full()
                .min_h_0()
                .child(
                    h_flex()
                        .px_2()
                        .pb_2()
                        .gap_1()
                        .child(
                            Button::new("refresh-git")
                                .small()
                                .ghost()
                                .label(if self.loading {
                                    "重新扫描"
                                } else {
                                    "刷新"
                                })
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.refresh(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("cancel-git")
                                .small()
                                .ghost()
                                .label("取消")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel.store(true, Ordering::Relaxed);
                                    this.generation += 1;
                                    this.refresh_task = None;
                                    this.loading = false;
                                    this.close_preview(cx);
                                    this.message = "已取消；当前结果可能不完整，请重新扫描".into();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .pb_1()
                        .text_size(theme::TEXT_CAPTION)
                        .child(format!(
                            "全部仓库 · {} · {}",
                            self.groups.len(),
                            if self.loading {
                                "刷新中"
                            } else {
                                "磁盘快照"
                            }
                        )),
                )
                .child(
                    uniform_list(
                        "changes",
                        self.rows.len(),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range.map(|index| this.row(index, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .flex_1()
                    .w_full(),
                )
                .into_any_element(),
        };
        let content = match self.active {
            Pane::Markdown => div()
                .id("markdown-preview")
                .size_full()
                .overflow_y_scroll()
                .p_4()
                .child(TextView::markdown("sample-markdown", MARKDOWN))
                .into_any_element(),
            Pane::Diff => match &self.preview {
                Some(editor) => Editor::new(editor)
                    .readonly(true)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div()
                    .p_4()
                    .child(self.preview_title.clone())
                    .into_any_element(),
            },
            Pane::Document(id) => match self.documents.iter().find(|document| document.id == id) {
                Some(document) => Editor::new(&document.editor)
                    .readonly(document.readonly)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Pane::Scratch => Editor::new(&self.scratch)
                .bordered(false)
                .size_full()
                .into_any_element(),
        };
        let title = match self.active {
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| {
                    format!(
                        "{} · {}",
                        self.root
                            .as_ref()
                            .and_then(|root| doc.path.strip_prefix(root).ok())
                            .unwrap_or(&doc.path)
                            .display(),
                        if doc.readonly {
                            "硬链接：只读"
                        } else {
                            "临时编辑：不写磁盘"
                        }
                    )
                })
                .unwrap_or_default(),
            Pane::Diff => format!("{} · 只读", self.preview_title),
            Pane::Markdown => "Markdown 原生预览样例".into(),
            Pane::Scratch => "输入测试.rs · 临时编辑：不写磁盘".into(),
        };
        let mut tabs = vec![
            Button::new("scratch-tab")
                .small()
                .ghost()
                .label(if self.dirty {
                    "输入测试.rs ●"
                } else {
                    "输入测试.rs"
                })
                .on_click(
                    cx.listener(|this, _, window, cx| this.select_pane(Pane::Scratch, window, cx)),
                )
                .into_any_element(),
        ];
        for (index, document) in self.documents.iter().enumerate() {
            let id = document.id;
            let label = format!(
                "{}{}",
                document
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                if document.dirty { " ●" } else { "" }
            );
            tabs.push(
                h_flex()
                    .id(("document-tab", index))
                    .flex_shrink_0()
                    .h(theme::BAR_HEIGHT)
                    .border_t(theme::INDICATOR)
                    .border_color(if self.active == Pane::Document(id) {
                        colors.accent
                    } else {
                        colors.tabs
                    })
                    .bg(if self.active == Pane::Document(id) {
                        colors.editor
                    } else {
                        colors.tabs
                    })
                    .child(
                        Button::new(("select-tab", index))
                            .small()
                            .ghost()
                            .label(label)
                            .tooltip(document.path.to_string_lossy().into_owned())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_pane(Pane::Document(id), window, cx)
                            })),
                    )
                    .child(
                        Button::new(("close-tab", index))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("关闭文件")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_document(id, window, cx)
                            })),
                    )
                    .into_any_element(),
            );
        }
        if !self.preview_title.is_empty() {
            tabs.push(
                Button::new("diff-tab")
                    .small()
                    .ghost()
                    .label("Diff · 只读")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.select_pane(Pane::Diff, window, cx)),
                    )
                    .into_any_element(),
            );
            tabs.push(
                Button::new("close-diff")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("关闭 Diff")
                    .on_click(cx.listener(|this, _, _, cx| this.close_preview(cx)))
                    .into_any_element(),
            );
        }
        tabs.push(
            Button::new("markdown-tab")
                .small()
                .ghost()
                .label("Markdown 样例")
                .on_click(
                    cx.listener(|this, _, window, cx| this.select_pane(Pane::Markdown, window, cx)),
                )
                .into_any_element(),
        );
        let rail_item = |active: bool| {
            if active {
                colors.selected
            } else {
                colors.panel
            }
        };
        let rail = v_flex()
            .w(theme::RAIL_WIDTH)
            .h_full()
            .flex_shrink_0()
            .py_2()
            .gap_2()
            .items_center()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(
                Button::new("activity-explorer")
                    .ghost()
                    .accessibility_label("资源管理器")
                    .bg(rail_item(self.sidebar == Sidebar::Explorer))
                    .icon(IconName::Folder)
                    .tooltip("资源管理器")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar = Sidebar::Explorer;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("activity-search")
                    .ghost()
                    .accessibility_label("搜索")
                    .bg(rail_item(self.sidebar == Sidebar::Search))
                    .icon(IconName::Search)
                    .tooltip("搜索")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.sidebar = Sidebar::Search;
                        this.search_input
                            .update(cx, |input, cx| input.focus(window, cx));
                        cx.notify();
                    })),
            )
            .child(
                Button::new("activity-scm")
                    .ghost()
                    .accessibility_label("源代码管理")
                    .bg(rail_item(self.sidebar == Sidebar::SourceControl))
                    .icon(IconName::GitBranch)
                    .tooltip("源代码管理")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sidebar = Sidebar::SourceControl;
                        cx.notify();
                    })),
            );
        let sidebar =
            v_flex()
                .size_full()
                .min_h_0()
                .bg(colors.panel)
                .child(
                    h_flex()
                        .h(theme::BAR_HEIGHT)
                        .px_3()
                        .justify_between()
                        .flex_shrink_0()
                        .text_size(theme::TEXT_BODY)
                        .child(sidebar_title)
                        .when(self.sidebar == Sidebar::Explorer, |header| {
                            header.child(
                                Button::new("refresh-tree")
                                    .small()
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .accessibility_label("刷新目录")
                                    .tooltip("刷新目录")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.refresh_tree(window, cx)
                                    })),
                            )
                        }),
                )
                .child(sidebar_content);
        let editor = v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(
                h_flex()
                    .id("file-tabs")
                    .h(theme::BAR_HEIGHT)
                    .w_full()
                    .flex_shrink_0()
                    .overflow_x_scroll()
                    .bg(colors.tabs)
                    .children(tabs),
            )
            .child(
                h_flex()
                    .h(theme::ROW_HEIGHT)
                    .w_full()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .px_3()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(cx.theme().muted_foreground)
                    .child(title),
            )
            .child(div().flex_1().min_h_0().child(content));
        v_flex()
            .size_full()
            .key_context("WorkspaceEditor")
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.choose_path(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFolder, window, cx| {
                this.choose_path(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SaveUnavailable, _, cx| {
                this.message = "当前原型尚未支持保存：修改仅在内存中，原文件未改动".into();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &QuickOpenFile, window, cx| {
                this.open_quick_open(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .relative()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_title_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.sidebar_visible, |row| row.child(rail))
                    .child(div().flex_1().h_full().min_w_0().map(|area| {
                        if self.sidebar_visible {
                            area.child(
                                h_resizable("workbench-panels")
                                    .child(
                                        resizable_panel()
                                            .size(theme::SIDEBAR_WIDTH)
                                            .size_range(theme::SIDEBAR_MIN..theme::SIDEBAR_MAX)
                                            .child(sidebar),
                                    )
                                    .child(
                                        resizable_panel()
                                            .size_range(theme::EDITOR_MIN..theme::EDITOR_MAX)
                                            .child(editor),
                                    ),
                            )
                        } else {
                            area.child(editor)
                        }
                    })),
            )
            .child(
                h_flex()
                    .h(theme::STATUS_HEIGHT)
                    .flex_shrink_0()
                    .px_2()
                    .overflow_hidden()
                    .bg(colors.panel)
                    .text_color(colors.muted)
                    .text_size(theme::TEXT_CAPTION)
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(format!(
                        "{} · {} 个打开文件 · 仓库 {} · 查询错误 {}{}",
                        self.message,
                        self.documents.len(),
                        self.groups.len(),
                        self.issues.len(),
                        self.issues
                            .first()
                            .map(|issue| format!(" · {issue}"))
                            .unwrap_or_default()
                    )),
            )
            .children(self.render_quick_open(cx))
    }
}
