//! Project images are decoded off-thread and retained only by their open tabs.
use super::*;
use crate::image_file::{self, Decoded};
use gpui_kit::{
    base::TestSupportExt,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex,
    },
};
use std::collections::BTreeSet;

pub(super) struct ImageTab {
    pub id: DocumentId,
    pub path: PathBuf,
    pub decoded: Decoded,
    pub zoom: Option<f32>,
    scroll: ScrollHandle,
    generation: u64,
    task: Option<Task<()>>,
    pub error: Option<String>,
}

impl Workbench {
    pub(super) fn open_image(
        &mut self,
        path: PathBuf,
        root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_file_open();
        let generation = self.file_generation;
        self.message = format!("正在打开 {}", path.display());
        self.file_task = Some(self.read_image(
            path,
            root,
            Some(generation),
            window,
            cx,
            |this, result, _, cx| {
                this.message = result
                    .err()
                    .map(|error| format!("图片打开失败：{error}"))
                    .unwrap_or_default();
                cx.notify();
            },
        ));
        cx.notify();
    }

    pub(super) fn read_image(
        &mut self,
        path: PathBuf,
        root: Option<PathBuf>,
        generation: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, Result<DocumentId, String>, &mut Window, &mut Context<Self>)
        + 'static,
    ) -> Task<()> {
        let job = cx.background_spawn(async move { image_file::load(root.as_deref(), &path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if generation.is_some_and(|generation| generation != this.file_generation) { return; }
                let result = result.map_err(|e| e.to_string()).and_then(|(path, decoded)| {
                    if let Some(id) = this.images.iter().find(|image| image.path == path).map(|image| image.id) {
                        this.show_pane(Pane::Image(id), window, cx);
                        return Ok(id);
                    }
                    let replacement = this.preview_replacement(&path).and_then(|pane| match pane {
                        Pane::Image(id) => this.images.iter().find(|image| image.id == id),
                        _ => None,
                    });
                    let used = this.image_decoded_bytes().saturating_sub(replacement.map_or(0, |image| image.decoded.decoded_bytes()));
                    if this.images.len() - usize::from(replacement.is_some()) >= image_file::MAX_OPEN_IMAGES || used + decoded.decoded_bytes() > image_file::MAX_DECODED_BYTES {
                        return Err("已达到图片查看预算（最多 8 张、合计 64 MiB 解码像素），请先关闭图片标签".into());
                    }
                    let id = DocumentId::allocate();
                    this.pending_tabs.retain(|pending| *pending != path);
                    this.images.push(ImageTab { id, path, decoded, zoom: None, scroll: ScrollHandle::new(), generation: 0, task: None, error: None });
                    this.show_pane(Pane::Image(id), window, cx);
                    Ok(id)
                });
                then(this, result, window, cx);
                cx.notify();
            });
        })
    }

    pub(super) fn close_image(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.images.iter().position(|image| image.id == id) {
            let image = self.images.remove(index);
            let _ = window.drop_image(image.decoded.image);
            self.recent_files.close(id, image.path.clone());
            self.closed_tabs.push(super::tab_menu::ClosedTab {
                path: image.path.clone(),
                offset: 0,
            });
            self.tab_order.retain(|path| *path != image.path);
        }
        if self.active == Pane::Image(id) {
            self.show_remaining_tab(window, cx);
        }
        self.remember_tabs(true, window, cx);
        cx.notify();
    }

    pub(super) fn refresh_images(
        &mut self,
        paths: Option<&BTreeSet<PathBuf>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<_> = self
            .images
            .iter()
            .filter(|image| paths.is_none_or(|paths| paths.contains(&image.path)))
            .map(|image| image.id)
            .collect();
        for id in ids {
            let index = self.images.iter().position(|image| image.id == id).unwrap();
            let path = self.images[index].path.clone();
            let root = self.root_for(&path);
            self.images[index].generation += 1;
            let generation = self.images[index].generation;
            let job = cx.background_spawn(async move { image_file::load(root.as_deref(), &path) });
            let task = cx.spawn_in(window, async move |this, cx| {
                let result = job.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let used = this.image_decoded_bytes().saturating_sub(
                        this.images
                            .iter()
                            .find(|image| image.id == id)
                            .map_or(0, |image| image.decoded.decoded_bytes()),
                    );
                    let Some(image) = this
                        .images
                        .iter_mut()
                        .find(|image| image.id == id && image.generation == generation)
                    else {
                        return;
                    };
                    match result {
                        Ok((_, decoded))
                            if used + decoded.decoded_bytes() <= image_file::MAX_DECODED_BYTES =>
                        {
                            if decoded.width != image.decoded.width
                                || decoded.height != image.decoded.height
                                || decoded.image.as_bytes(0) != image.decoded.image.as_bytes(0)
                            {
                                let _ = window.drop_image(image.decoded.image.clone());
                                image.decoded = decoded;
                            }
                            image.error = None;
                        }
                        Ok(_) => image.error = Some("图片更新超过解码预算，仍显示上次版本".into()),
                        Err(error) => {
                            image.error = Some(format!("图片更新失败，仍显示上次版本：{error}"))
                        }
                    }
                    cx.notify();
                });
            });
            self.images[index].task = Some(task);
        }
    }

    pub(super) fn image_follow_move(&mut self, old: &std::path::Path, new: &std::path::Path) {
        for image in &mut self.images {
            if let Ok(relative) = image.path.strip_prefix(old) {
                image.path = new.join(relative);
                image.generation += 1;
                image.task = None;
            }
        }
    }

    pub(super) fn image_zoom(&mut self, id: DocumentId, zoom: Option<f32>, cx: &mut Context<Self>) {
        if let Some(image) = self.images.iter_mut().find(|image| image.id == id) {
            // Keep at least one pixel on the longer edge, without enlarging a fitted image.
            let minimum = (1. / image.decoded.width.max(image.decoded.height) as f32).min(0.1);
            image.zoom = zoom.map(|zoom| zoom.clamp(minimum, 8.));
            image.scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        }
    }

    fn image_step_zoom(&mut self, id: DocumentId, factor: f32, cx: &mut Context<Self>) {
        let Some(image) = self.images.iter().find(|image| image.id == id) else {
            return;
        };
        let bounds = image.scroll.bounds().size;
        let fit = (f32::from(bounds.width) / image.decoded.width as f32)
            .min(f32::from(bounds.height) / image.decoded.height as f32);
        let zoom = image.zoom.unwrap_or(fit);
        if zoom > 0. {
            self.image_zoom(id, Some(zoom * factor), cx);
        }
    }

    pub(super) fn render_image(&self, id: DocumentId, cx: &mut Context<Self>) -> AnyElement {
        let Some(image) = self.images.iter().find(|image| image.id == id) else {
            return div().into_any_element();
        };
        let colors = theme::colors(cx);
        let metadata = format!(
            "{} · {} × {} · {} bytes",
            image.decoded.format, image.decoded.width, image.decoded.height, image.decoded.bytes
        );
        let toolbar = h_flex()
            .min_h(theme::CODE_HEADER_HEIGHT)
            .flex_shrink_0()
            .flex_wrap()
            .gap_2()
            .px_2()
            .child(
                div()
                    .id("image-info")
                    .aria_label(metadata.clone())
                    .text_color(colors.muted)
                    .child(metadata)
                    .test_support(),
            )
            .child(
                Button::new("image-fit")
                    .ghost()
                    .xsmall()
                    .label("适应窗口")
                    .on_click(cx.listener(move |this, _, _, cx| this.image_zoom(id, None, cx))),
            )
            .child(
                Button::new("image-original")
                    .ghost()
                    .xsmall()
                    .label("100%")
                    .on_click(cx.listener(move |this, _, _, cx| this.image_zoom(id, Some(1.), cx))),
            )
            .child(
                Button::new("image-zoom-out")
                    .ghost()
                    .xsmall()
                    .label("缩小")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.image_step_zoom(id, 1. / 1.25, cx)),
                    ),
            )
            .child(
                Button::new("image-zoom-in")
                    .ghost()
                    .xsmall()
                    .label("放大")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.image_step_zoom(id, 1.25, cx)),
                    ),
            )
            .when(
                image
                    .path
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("svg")),
                |toolbar| {
                    let path = image.path.clone();
                    toolbar.child(
                        Button::new("image-source")
                            .ghost()
                            .xsmall()
                            .label("查看 SVG 源码")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_text_file(path.clone(), this.root_for(&path), window, cx);
                            })),
                    )
                },
            );
        let pixels = img(image.decoded.image.clone()).object_fit(ObjectFit::Contain);
        let pixels = match image.zoom {
            None => pixels.size_full(),
            Some(zoom) => pixels
                .w(px(image.decoded.width as f32 * zoom))
                .h(px(image.decoded.height as f32 * zoom))
                .flex_shrink_0(),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(toolbar)
            .when_some(image.error.as_ref(), |view, error| {
                view.child(div().px_2().text_color(colors.muted).child(error.clone()))
            })
            .child(
                div()
                    .id("image-canvas")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_scroll()
                    .track_scroll(&image.scroll)
                    .child(pixels.id("image-pixels").test_support()),
            )
            .into_any_element()
    }
}

pub(super) struct ImageComparison {
    pub left: Result<Option<Decoded>, String>,
    pub right: Result<Option<Decoded>, String>,
    pub left_label: String,
    pub right_label: String,
    pub loading: bool,
}

impl ImageComparison {
    pub(super) fn decoded_bytes(&self) -> u64 {
        [&self.left, &self.right]
            .into_iter()
            .filter_map(|side| side.as_ref().ok()?.as_ref())
            .map(Decoded::decoded_bytes)
            .sum()
    }
}

#[derive(Clone)]
pub(super) enum ImageVersion {
    Empty,
    Disk(PathBuf, PathBuf),
    Index(workspace_editor_core::Repository, PathBuf),
    Commit(workspace_editor_core::Repository, String, PathBuf),
    Revision(PathBuf, String, PathBuf),
}

impl ImageVersion {
    fn load_pair(
        self,
        right: Self,
        service: &GitService,
        cancel: &AtomicBool,
        budget: u64,
    ) -> [Result<Option<Decoded>, String>; 2] {
        let left = self
            .load(service, cancel, budget)
            .map_err(|e| e.to_string());
        let used = left
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map_or(0, Decoded::decoded_bytes);
        let right = right
            .load(service, cancel, budget.saturating_sub(used))
            .map_err(|e| e.to_string());
        [left, right]
    }

    fn load(
        self,
        service: &GitService,
        cancel: &AtomicBool,
        budget: u64,
    ) -> std::io::Result<Option<Decoded>> {
        let (path, bytes) = match self {
            Self::Empty => return Ok(None),
            Self::Disk(root, path) => {
                return image_file::load_with_budget(Some(&root), &path, budget)
                    .map(|(_, decoded)| Some(decoded));
            }
            Self::Index(repo, path) => {
                let bytes = service.read_index_blob(&repo, &path, cancel)?;
                (path, bytes)
            }
            Self::Commit(repo, revision, path) => {
                let bytes = service.read_blob(&repo, &revision, &path, cancel)?;
                (path, bytes)
            }
            Self::Revision(root, revision, path) => {
                let repo = service.identify(&root, cancel)?;
                let bytes = service.read_blob(&repo, &revision, &path, cancel)?;
                (path, bytes)
            }
        };
        image_file::decode_with_budget(&path, &bytes, budget).map(Some)
    }
}

impl Workbench {
    pub(super) fn clear_image_diff(&mut self, window: &mut Window) {
        if let Some(images) = self.image_diff.take() {
            for image in [images.left, images.right]
                .into_iter()
                .filter_map(|side| side.ok().flatten())
            {
                let _ = window.drop_image(image.image);
            }
        }
    }

    pub(super) fn image_decoded_bytes(&self) -> u64 {
        self.images
            .iter()
            .map(|image| image.decoded.decoded_bytes())
            .sum::<u64>()
            + self
                .image_diff
                .as_ref()
                .map_or(0, ImageComparison::decoded_bytes)
            + self
                .archived_diffs
                .iter()
                .filter_map(|saved| saved.image_diff.as_ref())
                .map(ImageComparison::decoded_bytes)
                .sum::<u64>()
    }

    pub(super) fn open_image_comparison(
        &mut self,
        tab: DiffTab,
        left: ImageVersion,
        right: ImageVersion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prepare_new_diff(window, cx);
        self.diff.review.comparison.snapshot = None;
        self.diff.title = tab.label.clone();
        self.diff.tab = Some(tab);
        self.show_pane(Pane::Diff, window, cx);
        self.load_image_versions(left, right, window, cx);
    }

    pub(super) fn reload_image_checkpoint(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::session::review_progress::ComparisonSource;
        let Some(tab) = self.diff.tab.as_ref() else {
            return false;
        };
        let Some(checkpoint) = tab.checkpoint.clone() else {
            return false;
        };
        let sides = match checkpoint {
            ComparisonSource::Files { left, right } => {
                let root = |path: &std::path::Path| {
                    self.root_for(path).unwrap_or_else(|| {
                        path.parent()
                            .unwrap_or(std::path::Path::new("/"))
                            .to_path_buf()
                    })
                };
                (
                    ImageVersion::Disk(root(&left), left),
                    ImageVersion::Disk(root(&right), right),
                )
            }
            ComparisonSource::Git {
                repo,
                base,
                target,
                path,
                original_path,
            } => {
                let patch = match &tab.source {
                    DiffSource::Local(patch, _) => patch.as_ref(),
                    _ => "",
                };
                let added = patch.lines().any(|line| line.starts_with("new file mode "));
                let deleted = patch
                    .lines()
                    .any(|line| line.starts_with("deleted file mode "));
                (
                    if added {
                        ImageVersion::Empty
                    } else {
                        ImageVersion::Revision(
                            repo.clone(),
                            base,
                            original_path.unwrap_or_else(|| path.clone()),
                        )
                    },
                    if deleted {
                        ImageVersion::Empty
                    } else {
                        ImageVersion::Revision(repo, target, path)
                    },
                )
            }
            ComparisonSource::History {
                repo,
                commit,
                path,
                current_path,
            } => {
                let historical = ImageVersion::Revision(repo.clone(), commit, path);
                match current_path {
                    Some(path) => (
                        historical,
                        ImageVersion::Disk(repo.clone(), repo.join(path)),
                    ),
                    None => (ImageVersion::Empty, historical),
                }
            }
            ComparisonSource::Commit { .. } | ComparisonSource::GitChange { .. } => return false,
        };
        self.load_image_versions(sides.0, sides.1, window, cx);
        true
    }

    fn load_image_versions(
        &mut self,
        left: ImageVersion,
        right: ImageVersion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = |version: &ImageVersion| match version {
            ImageVersion::Empty => "空版本".into(),
            ImageVersion::Disk(_, path) => format!("磁盘 · {}", path.display()),
            ImageVersion::Index(_, path) => format!("暂存区 · {}", path.display()),
            ImageVersion::Commit(_, revision, path) | ImageVersion::Revision(_, revision, path) => {
                format!("{revision} · {}", path.display())
            }
        };
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.diff.generation += 1;
        let generation = self.diff.generation;
        self.diff.cancel = Arc::default();
        let cancel = self.diff.cancel.clone();
        let service = self.service.clone();
        let left_label = label(&left);
        let right_label = label(&right);
        self.clear_image_diff(window);
        let budget = image_file::MAX_DECODED_BYTES.saturating_sub(self.image_decoded_bytes());
        self.diff.loading = true;
        self.image_diff = Some(ImageComparison {
            left: Ok(None),
            right: Ok(None),
            left_label,
            right_label,
            loading: true,
        });
        let job = cx.background_spawn(async move {
            let [left, right] = left.load_pair(right, &service, &cancel, budget);
            (left, right)
        });
        self.diff.task = Some(cx.spawn_in(window, async move |this, cx| {
            let (left, right) = job.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.diff.generation != generation {
                    return;
                }
                let decoded = [left.as_ref(), right.as_ref()]
                    .into_iter()
                    .filter_map(|side| side.as_ref().ok().and_then(|side| side.as_ref()))
                    .map(|image| image.decoded_bytes())
                    .sum::<u64>();
                let remaining =
                    image_file::MAX_DECODED_BYTES.saturating_sub(this.image_decoded_bytes());
                if let Some(comparison) = this.image_diff.as_mut() {
                    comparison.loading = false;
                    if decoded > remaining {
                        comparison.left = Err("图片总预算已用尽，请先关闭其他图片".into());
                        comparison.right = Err("图片总预算已用尽，请先关闭其他图片".into());
                    } else {
                        comparison.left = left;
                        comparison.right = right;
                    }
                }
                this.diff.loading = false;
                this.diff.source = Some((
                    Arc::from(""),
                    gpui_kit::component::Theme::global(cx).is_dark(),
                ));
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn load_image_diff(
        &mut self,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_image_diff(window);
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.diff.generation += 1;
        self.diff.cancel = Arc::new(AtomicBool::new(false));
        self.diff.loading = true;
        self.diff.doc = None;
        self.diff.fallback = None;
        self.diff.raw = None;
        let generation = self.diff.generation;
        let cancel = self.diff.cancel.clone();
        let service = self.service.clone();
        let (left_label, right_label) = match &request.operation {
            Operation::Diff {
                side: DiffSide::Staged,
                ..
            } => ("HEAD".to_string(), "暂存区".to_string()),
            Operation::Diff {
                side: DiffSide::Worktree,
                ..
            } => ("暂存区".to_string(), "工作区".to_string()),
            Operation::UntrackedDiff { .. } => ("未跟踪前".to_string(), "工作区".to_string()),
            Operation::CommitDiff { commit, parent, .. } => (
                parent.clone().unwrap_or_else(|| "空版本".into()),
                commit.clone(),
            ),
            Operation::Status => return,
        };
        self.image_diff = Some(ImageComparison {
            left: Ok(None),
            right: Ok(None),
            left_label: left_label.clone(),
            right_label: right_label.clone(),
            loading: true,
        });
        let used = self.image_decoded_bytes();
        let budget = image_file::MAX_DECODED_BYTES.saturating_sub(used);
        let job = cx.background_spawn(async move {
            let reply = service.execute(&request, &cancel)?;
            if reply.repo != request.repo.id || reply.generation != request.generation {
                return Err(std::io::Error::other("图片比较返回了其他仓库或版本"));
            }
            let patch: Arc<str> = String::from_utf8(reply.output)
                .map_err(std::io::Error::other)?
                .into();
            let added = patch.lines().any(|line| line.starts_with("new file mode "));
            let deleted = patch
                .lines()
                .any(|line| line.starts_with("deleted file mode "));
            let repo = request.repo;
            let (left, right) = match request.operation {
                Operation::UntrackedDiff { path } => (
                    ImageVersion::Empty,
                    ImageVersion::Disk(repo.worktree.clone(), repo.worktree.join(path)),
                ),
                Operation::Diff {
                    side,
                    path,
                    original_path,
                } => {
                    let old_path = original_path.unwrap_or_else(|| path.clone());
                    let left = if added {
                        ImageVersion::Empty
                    } else {
                        match side {
                            DiffSide::Staged => {
                                ImageVersion::Commit(repo.clone(), "HEAD".into(), old_path)
                            }
                            DiffSide::Worktree => ImageVersion::Index(repo.clone(), old_path),
                        }
                    };
                    let right = if deleted {
                        ImageVersion::Empty
                    } else {
                        match side {
                            DiffSide::Staged => ImageVersion::Index(repo.clone(), path),
                            DiffSide::Worktree => {
                                ImageVersion::Disk(repo.worktree.clone(), repo.worktree.join(path))
                            }
                        }
                    };
                    (left, right)
                }
                Operation::CommitDiff {
                    commit,
                    parent,
                    path,
                    original_path,
                } => {
                    let old_path = original_path.unwrap_or_else(|| path.clone());
                    let left = if added {
                        ImageVersion::Empty
                    } else {
                        parent.map_or(ImageVersion::Empty, |parent| {
                            ImageVersion::Commit(repo.clone(), parent, old_path)
                        })
                    };
                    let right = if deleted {
                        ImageVersion::Empty
                    } else {
                        ImageVersion::Commit(repo, commit, path)
                    };
                    (left, right)
                }
                Operation::Status => return Err(std::io::Error::other("状态查询不是图片比较")),
            };
            let [left, right] = left.load_pair(right, &service, &cancel, budget);
            if cancel.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "图片比较已取消",
                ));
            }
            Ok((
                patch,
                ImageComparison {
                    left,
                    right,
                    left_label,
                    right_label,
                    loading: false,
                },
            ))
        });
        self.diff.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.diff.generation != generation {
                    return;
                }
                this.diff.loading = false;
                match result {
                    Ok((patch, comparison)) => {
                        this.diff.source =
                            Some((patch, gpui_kit::component::Theme::global(cx).is_dark()));
                        this.image_diff = Some(comparison);
                    }
                    Err(error) => {
                        if let Some(comparison) = this.image_diff.as_mut() {
                            comparison.loading = false;
                            comparison.left = Err(error.to_string());
                            comparison.right = Err(error.to_string());
                        }
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn render_image_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(comparison) = self.image_diff.as_ref() else {
            return div().into_any_element();
        };
        let colors = theme::colors(cx);
        let side = |name: &'static str, label: &str, data: &Result<Option<Decoded>, String>| {
            let body = match data {
                Ok(Some(decoded)) => v_flex()
                    .size_full()
                    .min_h_0()
                    .child(div().text_color(colors.muted).child(format!(
                        "{} × {} · {} bytes",
                        decoded.width, decoded.height, decoded.bytes
                    )))
                    .child(
                        img(decoded.image.clone())
                            .object_fit(ObjectFit::Contain)
                            .flex_1()
                            .min_h_0()
                            .size_full()
                            .id(name)
                            .test_support(),
                    )
                    .into_any_element(),
                Ok(None) => div()
                    .child(if comparison.loading {
                        "正在读取图片"
                    } else {
                        "此版本无图片"
                    })
                    .into_any_element(),
                Err(error) => div()
                    .text_color(colors.muted)
                    .child(format!("无法显示此版本：{error}"))
                    .into_any_element(),
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .h_full()
                .px_2()
                .child(div().h(theme::CODE_HEADER_HEIGHT).child(label.to_string()))
                .child(body)
        };
        v_flex()
            .size_full()
            .min_h_0()
            .when(self.diff.review.comparison.open, |view| {
                view.child(self.render_comparison_panel(cx))
            })
            .child(self.render_diff_actions(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(side(
                        "image-diff-left",
                        &comparison.left_label,
                        &comparison.left,
                    ))
                    .child(side(
                        "image-diff-right",
                        &comparison.right_label,
                        &comparison.right,
                    )),
            )
            .into_any_element()
    }
}
