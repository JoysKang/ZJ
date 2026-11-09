//! Comparison and history entry points. All reads and patch construction run off the UI thread.
use super::*;
use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use std::{
    io::{self, Write},
    path::Path,
};
use workspace_editor_git::{Comparison, ComparisonMode, FileHistoryEntry};

static NEXT_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn next_request() -> u64 {
    NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Revisions,
    History,
    Patch,
}

pub(super) struct ComparisonState {
    pub(super) open: bool,
    panel: Panel,
    pub(super) selected: Option<PathBuf>,
    base: Entity<InputState>,
    target: Entity<InputState>,
    mode: ComparisonMode,
    repo: Option<Repository>,
    comparison: Option<Comparison>,
    history: Vec<FileHistoryEntry>,
    history_path: Option<PathBuf>,
    history_more: bool,
    history_selected: Option<usize>,
    pub(super) snapshot: Option<Entity<EditorState>>,
    imported: Option<(Arc<str>, PathBuf, Arc<Status>)>,
    status: String,
    loading: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for ComparisonState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ComparisonState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let base = cx.new(|cx| InputState::new(window, cx).placeholder("基线分支或提交"));
        let target = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("目标分支或提交")
                .default_value("HEAD")
        });
        let subscriptions = [&base, &target]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this: &mut Workbench, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            this.run_revision_comparison(window, cx);
                        } else if matches!(event, InputEvent::Change) {
                            let state = &mut this.diff.review.comparison;
                            state.invalidate_result();
                            cx.notify();
                        }
                    },
                )
            })
            .collect();
        Self {
            open: false,
            panel: Panel::Revisions,
            selected: None,
            base,
            target,
            mode: ComparisonMode::Direct,
            repo: None,
            comparison: None,
            history: Vec::new(),
            history_path: None,
            history_more: false,
            history_selected: None,
            snapshot: None,
            imported: None,
            status: String::new(),
            loading: false,
            generation: 0,
            cancel: Arc::default(),
            _subscriptions: subscriptions,
        }
    }

    fn invalidate_result(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.generation = next_request();
        self.comparison = None;
        self.status.clear();
        self.loading = false;
    }

    fn begin(&mut self) -> (u64, Arc<AtomicBool>) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self.generation = next_request();
        self.loading = true;
        self.status.clear();
        (self.generation, self.cancel.clone())
    }
}

pub(super) fn local_patch(old: &str, new: &str, name: &Path) -> Arc<str> {
    // A local comparison exports a standard patch with the target filename, never an absolute path.
    let name = name
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("comparison.txt"));
    let quote = |prefix: &str| {
        use std::os::unix::ffi::OsStrExt;
        let mut result = format!("\"{prefix}/");
        for &byte in name.as_bytes() {
            match byte {
                b'"' => result.push_str("\\\""),
                b'\\' => result.push_str("\\\\"),
                32..=126 => result.push(byte as char),
                _ => result.push_str(&format!("\\{byte:03o}")),
            }
        }
        result.push('"');
        result
    };
    let (old_name, new_name) = (quote("a"), quote("b"));
    format!(
        "diff --git {old_name} {new_name}\n--- {old_name}\n+++ {new_name}\n{}",
        crate::save::text_patch(old, new)
    )
    .into()
}

pub(super) fn history_text(bytes: &[u8]) -> io::Result<String> {
    if bytes.len() > crate::files::MAX_FILE_BYTES {
        return Err(io::Error::other("历史文本超过 8 MiB，未载入编辑器"));
    }
    let (text, _) = crate::encoded_text::decode(bytes, None)?;
    if text.len() > crate::files::MAX_FILE_BYTES
        || text.split('\n').any(|line| line.len() > 256 * 1024)
    {
        return Err(io::Error::other(
            "历史文本超过 8 MiB 或单行超过 256 KiB，未载入编辑器",
        ));
    }
    Ok(text)
}

fn read_text(path: &Path) -> io::Result<String> {
    crate::files::text_file(None, path).map(|file| file.text)
}

impl Workbench {
    fn ensure_comparison_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.diff.tab.is_none() {
            self.show_diff_tab(
                DiffTab {
                    label: "比较".into(),
                    tooltip: "比较与文件历史".into(),
                    path,
                    source: DiffSource::Local("".into(), "比较"),
                    checkpoint: None,
                },
                window,
                cx,
            );
        } else {
            self.select_pane(Pane::Diff, window, cx);
        }
    }

    /// File-tree and tab menus call this for their exact selected path, including pending tabs.
    pub(super) fn select_for_comparison(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.diff.review.comparison.selected = Some(path.clone());
        self.message = format!("已选择用于比较：{}", path.display());
        cx.notify();
    }

    pub(super) fn compare_with_selected(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.diff.review.comparison.selected.clone() else {
            self.message = "请先选择用于比较的文件".into();
            cx.notify();
            return;
        };
        if crate::image_file::supported(&selected) && crate::image_file::supported(&path) {
            self.diff.review.comparison.open = false;
            use super::image_view::ImageVersion;
            let checkpoint = Some(crate::session::review_progress::ComparisonSource::Files {
                left: selected.clone(),
                right: path.clone(),
            });
            let tab = DiffTab {
                label: "图片比较".into(),
                tooltip: format!("{} → {}", selected.display(), path.display()),
                path: path.clone(),
                source: DiffSource::Local("".into(), "所选图片 → 图片"),
                checkpoint,
            };
            let left = ImageVersion::Disk(
                selected.parent().unwrap_or(Path::new("/")).to_path_buf(),
                selected,
            );
            let right =
                ImageVersion::Disk(path.parent().unwrap_or(Path::new("/")).to_path_buf(), path);
            self.open_image_comparison(tab, left, right, window, cx);
            return;
        }
        self.compare_local_text(selected, path, None, window, cx);
    }

    /// Clipboard text is captured at invocation; later clipboard changes cannot alter the comparison.
    pub(super) fn compare_with_clipboard(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.message = "剪贴板没有文本".into();
            cx.notify();
            return;
        };
        if text.len() > crate::files::MAX_FILE_BYTES {
            self.message = "剪贴板文本超过 8 MiB".into();
            cx.notify();
            return;
        }
        self.compare_local_text(PathBuf::from("剪贴板"), path, Some(text), window, cx);
    }

    fn compare_local_text(
        &mut self,
        selected: PathBuf,
        path: PathBuf,
        clipboard: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_file_open();
        let (generation, cancel) = self.diff.review.comparison.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let wanted = path.clone();
        let left_buffer = self
            .documents
            .iter()
            .find(|doc| doc.path == selected)
            .map(|doc| doc.editor.read(cx).text().to_string());
        let right_buffer = self
            .documents
            .iter()
            .find(|doc| doc.path == path)
            .map(|doc| doc.editor.read(cx).text().to_string());
        let dirty_buffer = self
            .documents
            .iter()
            .any(|doc| (doc.path == selected || doc.path == path) && doc.dirty);
        let tooltip = format!(
            "{} → {}{}",
            selected.display(),
            path.display(),
            if dirty_buffer {
                " · 包含发起比较时的未保存内容"
            } else {
                ""
            }
        );
        let checkpoint = (clipboard.is_none() && !dirty_buffer).then(|| {
            crate::session::review_progress::ComparisonSource::Files {
                left: selected.clone(),
                right: path.clone(),
            }
        });
        let label = if clipboard.is_some() {
            "剪贴板 → 文件"
        } else {
            "所选文件 → 文件"
        };
        let job = cx.background_spawn(async move {
            let old = match clipboard.or(left_buffer) {
                Some(text) => text,
                None => read_text(&selected)?,
            };
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "比较已取消"));
            }
            let new = match right_buffer {
                Some(text) => text,
                None => read_text(&wanted)?,
            };
            if old.len() > crate::files::MAX_FILE_BYTES || new.len() > crate::files::MAX_FILE_BYTES
            {
                return Err(io::Error::other("比较文本超过 8 MiB"));
            }
            Ok::<_, io::Error>(local_patch(&old, &new, &wanted))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.diff.review.comparison.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if this.diff.review.comparison.generation == generation {
                        this.diff.review.comparison.loading = false;
                        cx.notify();
                    }
                    return;
                }
                this.diff.review.comparison.loading = false;
                match result {
                    Ok(patch) => {
                        this.diff.review.comparison.open = false;
                        this.show_diff_tab(
                            DiffTab {
                                label: format!(
                                    "比较 {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                ),
                                tooltip,
                                path,
                                source: DiffSource::Local(patch, label),
                                checkpoint,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => this.message = format!("比较失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_revision_comparison(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let repo = group.repo.clone();
        self.ensure_comparison_tab(repo.worktree.clone(), window, cx);
        let state = &mut self.diff.review.comparison;
        state.cancel.store(true, Ordering::Relaxed);
        state.generation = next_request();
        state.open = true;
        state.loading = false;
        state.panel = Panel::Revisions;
        state.repo = Some(repo);
        state.comparison = None;
        state.status.clear();
        state.base.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }

    fn run_revision_comparison(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.diff.review.comparison;
        let Some(repo) = state.repo.clone() else {
            return;
        };
        let base = state.base.read(cx).value().to_string();
        let target = state.target.read(cx).value().to_string();
        let mode = state.mode;
        state.comparison = None;
        let (generation, cancel) = state.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let service = self.service.clone();
        let job = cx.background_spawn(async move {
            service.compare_revisions(&repo, &base, &target, mode, &cancel)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.diff.review.comparison;
                if state.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if state.generation == generation {
                        state.loading = false;
                        cx.notify();
                    }
                    return;
                }
                state.loading = false;
                match result {
                    Ok(comparison) => {
                        state.status = format!(
                            "{} → {} · {} 个文件",
                            &comparison.base[..8],
                            &comparison.target[..8],
                            comparison.files.len()
                        );
                        state.comparison = Some(comparison);
                    }
                    Err(error) => state.status = format!("比较失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_comparison_file(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.diff.review.comparison;
        let (Some(repo), Some(comparison)) = (state.repo.clone(), state.comparison.clone()) else {
            return;
        };
        let Some(file) = comparison.files.get(index).cloned() else {
            return;
        };
        if crate::image_file::supported(&file.path) {
            use super::image_view::ImageVersion;
            let checkpoint = Some(crate::session::review_progress::ComparisonSource::Git {
                repo: repo.worktree.clone(),
                base: comparison.base.clone(),
                target: comparison.target.clone(),
                path: file.path.clone(),
                original_path: file.original_path.clone(),
            });
            let path = repo.worktree.join(&file.path);
            let left = if file.status == 'A' {
                ImageVersion::Empty
            } else {
                ImageVersion::Commit(
                    repo.clone(),
                    comparison.base.clone(),
                    file.original_path
                        .clone()
                        .unwrap_or_else(|| file.path.clone()),
                )
            };
            let right = if file.status == 'D' {
                ImageVersion::Empty
            } else {
                ImageVersion::Commit(repo, comparison.target.clone(), file.path)
            };
            let tab = DiffTab {
                label: "图片版本比较".into(),
                tooltip: format!(
                    "{} · {} → {}",
                    path.display(),
                    &comparison.base[..8],
                    &comparison.target[..8]
                ),
                path,
                source: DiffSource::Local("".into(), "基线 → 目标提交"),
                checkpoint,
            };
            self.open_image_comparison(tab, left, right, window, cx);
            return;
        }
        let (generation, cancel) = state.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let service = self.service.clone();
        let path = repo.worktree.join(&file.path);
        let checkpoint = Some(crate::session::review_progress::ComparisonSource::Git {
            repo: repo.worktree.clone(),
            base: comparison.base.clone(),
            target: comparison.target.clone(),
            path: file.path.clone(),
            original_path: file.original_path.clone(),
        });
        let tooltip = format!(
            "{} · {} → {}",
            path.display(),
            &comparison.base[..8],
            &comparison.target[..8]
        );
        let job = cx.background_spawn(async move {
            let bytes = service.comparison_patch(&repo, &comparison, Some(&file), &cancel)?;
            String::from_utf8(bytes)
                .map(Arc::<str>::from)
                .map_err(|_| io::Error::other("比较补丁不是 UTF-8"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.diff.review.comparison;
                if state.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if state.generation == generation {
                        state.loading = false;
                        cx.notify();
                    }
                    return;
                }
                state.loading = false;
                match result {
                    Ok(patch) => {
                        this.show_diff_tab(
                            DiffTab {
                                label: format!(
                                    "比较 {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                ),
                                tooltip,
                                path,
                                source: DiffSource::Local(patch, "基线 → 目标提交"),
                                checkpoint,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => state.status = format!("Diff 加载失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn open_file_history(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .groups
            .iter()
            .filter(|g| path.starts_with(&g.repo.worktree))
            .max_by_key(|g| g.repo.worktree.components().count())
        else {
            self.message = "文件不属于已打开的 Git 仓库".into();
            cx.notify();
            return;
        };
        let repo = group.repo.clone();
        let relative = path.strip_prefix(&repo.worktree).unwrap().to_path_buf();
        self.ensure_comparison_tab(path, window, cx);
        let state = &mut self.diff.review.comparison;
        state.open = true;
        state.panel = Panel::History;
        state.repo = Some(repo);
        state.history_path = Some(relative);
        state.history.clear();
        state.history_selected = None;
        self.load_file_history(window, cx);
    }

    fn load_file_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.diff.review.comparison;
        let (Some(repo), Some(path)) = (state.repo.clone(), state.history_path.clone()) else {
            return;
        };
        let skip = state.history.len();
        let (generation, cancel) = state.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let service = self.service.clone();
        let job =
            cx.background_spawn(
                async move { service.file_history(&repo, &path, skip, 200, &cancel) },
            );
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.diff.review.comparison;
                if state.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if state.generation == generation {
                        state.loading = false;
                        cx.notify();
                    }
                    return;
                }
                state.loading = false;
                match result {
                    Ok(entries) => {
                        state.history_more = entries.len() == 200;
                        state.history.extend(entries);
                        state.status =
                            format!("已加载 {} 条历史（跟随重命名）", state.history.len());
                    }
                    Err(error) => state.status = format!("历史加载失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open_history_entry(
        &mut self,
        index: usize,
        compare: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = &mut self.diff.review.comparison;
        let (Some(repo), Some(entry), Some(current_path)) = (
            state.repo.clone(),
            state.history.get(index).cloned(),
            state.history_path.clone(),
        ) else {
            return;
        };
        if entry.deleted {
            state.status = "该提交删除了文件；请选择此前版本查看完整内容".into();
            cx.notify();
            return;
        }
        let checkpoint = Some(crate::session::review_progress::ComparisonSource::History {
            repo: repo.worktree.clone(),
            commit: entry.commit.hash.clone(),
            path: entry.path.clone(),
            current_path: compare.then_some(current_path.clone()),
        });
        state.history_selected = Some(index);
        if crate::image_file::supported(&entry.path) {
            use super::image_view::ImageVersion;
            let path = repo.worktree.join(&current_path);
            let historical =
                ImageVersion::Commit(repo.clone(), entry.commit.hash.clone(), entry.path.clone());
            let (left, right) = if compare {
                (
                    historical,
                    ImageVersion::Disk(repo.worktree.clone(), path.clone()),
                )
            } else {
                (ImageVersion::Empty, historical)
            };
            let tab = DiffTab {
                label: if compare {
                    "历史图片比较".into()
                } else {
                    "历史图片（只读）".into()
                },
                tooltip: format!("{} · {}", entry.path.display(), &entry.commit.hash[..8]),
                path,
                source: DiffSource::Local(
                    "".into(),
                    if compare {
                        "历史版本 → 工作区图片"
                    } else {
                        "历史图片完整内容"
                    },
                ),
                checkpoint,
            };
            self.open_image_comparison(tab, left, right, window, cx);
            return;
        }
        let (generation, cancel) = state.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let service = self.service.clone();
        let title = format!(
            "{} · {}（只读）",
            entry.path.display(),
            &entry.commit.hash[..8]
        );
        let path = repo.worktree.join(current_path);
        let wanted = path.clone();
        let job = cx.background_spawn(async move {
            let bytes = service.read_blob(&repo, &entry.commit.hash, &entry.path, &cancel)?;
            let text = history_text(&bytes)?;
            if compare {
                let current = read_text(&wanted)?;
                Ok::<_, io::Error>((text.clone(), Some(local_patch(&text, &current, &wanted))))
            } else {
                Ok((text, None))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.diff.review.comparison;
                if state.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if state.generation == generation {
                        state.loading = false;
                        cx.notify();
                    }
                    return;
                }
                state.loading = false;
                match result {
                    Ok((text, Some(patch))) => {
                        let _ = text;
                        this.show_diff_tab(
                            DiffTab {
                                label: "历史比较".into(),
                                tooltip: title,
                                path,
                                source: DiffSource::Local(patch, "历史版本 → 工作区文件"),
                                checkpoint,
                            },
                            window,
                            cx,
                        );
                    }
                    Ok((text, None)) => {
                        this.prepare_new_diff(window, cx);
                        this.diff.generation += 1;
                        this.diff.loading = false;
                        this.diff.tab = Some(DiffTab {
                            label: "历史完整内容（只读）".into(),
                            tooltip: title.clone(),
                            path,
                            source: DiffSource::Snapshot(text.clone().into()),
                            checkpoint,
                        });
                        this.diff.title = title.clone();
                        this.diff.review.comparison.status = title;
                        this.diff.review.comparison.snapshot =
                            Some(cx.new(|cx| EditorState::new(window, cx).default_value(text)));
                        this.active = Pane::Diff;
                    }
                    Err(error) => state.status = format!("历史版本加载失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn export_diff_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((patch, _)) = self.diff.source.clone() else {
            return;
        };
        let directory = self
            .diff
            .tab
            .as_ref()
            .and_then(|tab| tab.path.parent())
            .unwrap_or_else(|| self.root.as_deref().unwrap_or(Path::new(".")))
            .to_path_buf();
        let chosen = cx.prompt_for_new_path(&directory, Some("comparison.patch"));
        cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        this.message = format!("无法选择导出路径：{other:?}");
                        cx.notify();
                    });
                    return;
                }
            };
            let job = cx.background_spawn(async move {
                // create_new preserves an existing file even if a platform dialog omits overwrite confirmation.
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                file.write_all(patch.as_bytes())?;
                file.sync_all()?;
                Ok::<_, io::Error>(path)
            });
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.message = match result {
                    Ok(path) => format!("补丁已导出：{}", path.display()),
                    Err(error) => format!("补丁导出失败（请选择尚不存在的文件）：{error}"),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_patch_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.active_group() else {
            self.message = "请先选择目标 Git 仓库".into();
            cx.notify();
            return;
        };
        let repo = self.groups[g].repo.clone();
        self.diff.review.comparison.imported = None;
        let (generation, cancel) = self.diff.review.comparison.begin();
        let diff_generation = self.diff.generation;
        let file_generation = self.file_generation;
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("打开补丁并预检目标仓库".into()),
        });
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(paths))) => match paths.into_iter().next() {
                    Some(path) => path,
                    None => return,
                },
                _ => return,
            };
            let wanted_repo = repo.clone();
            let job = cx.background_spawn(async move {
                let bytes = crate::files::read_prefix(None, &path, 16_000_001)?;
                if bytes.len() > 16_000_000 {
                    return Err(io::Error::other("补丁超过 16 MB"));
                }
                let patch =
                    String::from_utf8(bytes).map_err(|_| io::Error::other("补丁不是 UTF-8"))?;
                let target = service.patch_target(&repo, &patch, &cancel)?;
                let status = Arc::new(service.status(&repo, 0, &cancel)?);
                service.check_patch(&repo, &patch, false, &cancel)?;
                Ok::<_, io::Error>((Arc::<str>::from(patch), target, status))
            });
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let state = &mut this.diff.review.comparison;
                if state.generation != generation
                    || this.diff.generation != diff_generation
                    || this.file_generation != file_generation
                {
                    if state.generation == generation {
                        state.loading = false;
                        cx.notify();
                    }
                    return;
                }
                state.loading = false;
                match result {
                    Ok((patch, path, status)) => {
                        state.repo = Some(wanted_repo.clone());
                        state.panel = Panel::Patch;
                        state.open = true;
                        state.status = format!(
                            "预检通过 · {} · {}",
                            wanted_repo.worktree.display(),
                            path.display()
                        );
                        state.imported = Some((patch.clone(), path.clone(), status));
                        this.show_diff_tab(
                            DiffTab {
                                label: "补丁预览".into(),
                                tooltip: format!("目标仓库：{}", wanted_repo.worktree.display()),
                                path: wanted_repo.worktree.join(path),
                                source: DiffSource::Local(patch, "补丁应用预览（尚未写入）"),
                                checkpoint: None,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(error) => this.message = format!("补丁预检失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_imported_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = &mut self.diff.review.comparison;
        let (Some(repo), Some((patch, path, expected))) =
            (state.repo.clone(), state.imported.clone())
        else {
            return;
        };
        self.request_git_write(
            WriteRequest {
                repo,
                generation: 0,
                expected,
                operation: WriteOperation::ApplyExternalPatch {
                    path,
                    patch: patch.to_string(),
                },
            },
            window,
            cx,
        );
    }

    pub(super) fn render_comparison_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.diff.review.comparison;
        let colors = theme::colors(cx);
        let bar = h_flex()
            .flex_wrap()
            .w_full()
            .gap_2()
            .p_2()
            .text_size(theme::TEXT_CAPTION)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(colors.muted)
                    .child(if state.loading {
                        "正在读取…".into()
                    } else {
                        state.status.clone()
                    }),
            )
            .child(
                Button::new("comparison-panel-close")
                    .ghost()
                    .xsmall()
                    .label("关闭比较面板")
                    .on_click(cx.listener(|this, _, _, cx| {
                        let state = &mut this.diff.review.comparison;
                        state.open = false;
                        state.cancel.store(true, Ordering::Relaxed);
                        state.generation = next_request();
                        state.loading = false;
                        cx.notify();
                    })),
            );
        let mut panel = v_flex()
            .w_full()
            .flex_shrink_0()
            .border_b_1()
            .border_color(colors.border)
            .child(bar);
        match state.panel {
            Panel::Revisions => {
                panel = panel.child(
                    h_flex()
                        .flex_wrap()
                        .w_full()
                        .gap_2()
                        .px_2()
                        .pb_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w(theme::DIFF_CONTEXT_MIN_WIDTH)
                                .child(Input::new(&state.base).small()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(theme::DIFF_CONTEXT_MIN_WIDTH)
                                .child(Input::new(&state.target).small()),
                        )
                        .child(
                            Button::new("comparison-mode")
                                .ghost()
                                .small()
                                .label(if state.mode == ComparisonMode::Direct {
                                    "直接比较两个版本"
                                } else {
                                    "共同祖先 → 目标"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let state = &mut this.diff.review.comparison;
                                    state.invalidate_result();
                                    state.mode = if state.mode == ComparisonMode::Direct {
                                        ComparisonMode::MergeBase
                                    } else {
                                        ComparisonMode::Direct
                                    };
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("comparison-run")
                                .ghost()
                                .small()
                                .label("列出变更文件")
                                .disabled(state.loading)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.run_revision_comparison(window, cx)
                                })),
                        ),
                );
                if let Some(comparison) = &state.comparison {
                    let count = comparison.files.len();
                    panel = panel.child(
                        uniform_list(
                            "revision-comparison-files",
                            count,
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                let Some(comparison) =
                                    this.diff.review.comparison.comparison.as_ref()
                                else {
                                    return Vec::new();
                                };
                                range
                                    .filter_map(|i| {
                                        comparison.files.get(i).map(|file| {
                                            h_flex()
                                                .id(("revision-file", i))
                                                .h(theme::ROW_HEIGHT)
                                                .px_3()
                                                .gap_2()
                                                .text_size(theme::TEXT_BODY)
                                                .child(
                                                    format!(
                                                        "{} {}",
                                                        file.status,
                                                        file.path.display()
                                                    )
                                                    .replace(SINGLE_LINE, "⏎"),
                                                )
                                                .cursor_pointer()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.open_comparison_file(i, window, cx)
                                                    },
                                                ))
                                                .test_support()
                                                .into_any_element()
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h(theme::ROW_HEIGHT * 8.),
                    );
                }
            }
            Panel::History => {
                panel = panel
                    .child(
                        uniform_list(
                            "file-history",
                            state.history.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .filter_map(|i| {
                                        this.diff.review.comparison.history.get(i).map(|entry| {
                                            h_flex()
                                                .id(("history-entry", i))
                                                .h(theme::ROW_HEIGHT)
                                                .px_3()
                                                .text_size(theme::TEXT_BODY)
                                                .child(
                                                    format!(
                                                        "{} · {} · {} · {} · {}",
                                                        &entry.commit.hash[..8],
                                                        entry.commit.author,
                                                        crate::time::relative_time(
                                                            entry.commit.time * 1000,
                                                            crate::time::now_ms(),
                                                            crate::time::local_offset(
                                                                entry.commit.time * 1000
                                                            )
                                                        ),
                                                        entry.commit.subject,
                                                        entry.path.display()
                                                    )
                                                    .replace(SINGLE_LINE, "⏎"),
                                                )
                                                .cursor_pointer()
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.open_history_entry(
                                                            i, false, window, cx,
                                                        )
                                                    },
                                                ))
                                                .test_support()
                                                .into_any_element()
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .h(theme::ROW_HEIGHT * 8.),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .p_2()
                            .child(
                                Button::new("history-more")
                                    .ghost()
                                    .small()
                                    .label("加载更多历史")
                                    .disabled(!state.history_more || state.loading)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.load_file_history(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("history-compare-current")
                                    .ghost()
                                    .small()
                                    .label("与当前文件比较")
                                    .disabled(state.history_selected.is_none() || state.loading)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(i) =
                                            this.diff.review.comparison.history_selected
                                        {
                                            this.open_history_entry(i, true, window, cx);
                                        }
                                    })),
                            ),
                    );
            }
            Panel::Patch => {
                panel =
                    panel.child(
                        Button::new("patch-apply")
                            .ghost()
                            .small()
                            .label("确认应用到目标仓库…")
                            .disabled(state.imported.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_imported_patch(window, cx)
                            })),
                    );
            }
        }
        panel.into_any_element()
    }
}

#[cfg(test)]
#[path = "comparison_ui_tests.rs"]
mod ui_tests;

#[cfg(test)]
mod tests {
    use super::local_patch;
    #[test]
    fn local_patch_is_complete_and_quotes_target_names() {
        let patch = local_patch("one\n", "two", std::path::Path::new("a\tname.txt"));
        assert!(patch.contains("a/a\\011name.txt"));
        assert!(patch.contains("\\ No newline at end of file"));
        assert!(!patch.contains("/Users/"));
    }
}
