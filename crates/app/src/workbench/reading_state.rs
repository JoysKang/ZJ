//! Reading persistence and explicit review actions. Rendering only reads in-memory state.
use super::*;
use crate::session::review_progress::{
    self, ComparisonSource, FilePosition, PinnedComparison, ReadingSession, ReviewCheckpoint,
    ReviewStatus,
};
use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::RopeExt,
};
use std::{collections::HashSet, io, path::Path};

#[derive(Default)]
pub(super) struct ReviewStoreChanged;
impl Global for ReviewStoreChanged {}

#[derive(Default)]
pub(super) struct ReadingState {
    pub(super) saved: ReadingSession,
    statuses: Vec<ReviewStatus>,
    positions_pending: HashSet<PathBuf>,
    diff_positions_pending: Vec<PinnedComparison>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    restoring_pins: bool,
    review_edits: u64,
    store_tail: Option<async_channel::Receiver<()>>,
    store_generation: u64,
}

impl Drop for ReadingState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Workbench {
    /// Runs for every opened workspace, including one absent from the previous window session.
    pub(super) fn load_review_progress(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(root), Some(path)) = (self.root.clone(), crate::session::review_path(cx)) else {
            return;
        };
        self.reading.store_generation += 1;
        let generation = self.reading.store_generation;
        let edits = self.reading.review_edits;
        let mut roots = self.extra_repos(cx);
        roots.push(root.clone());
        let job = cx.background_spawn(async move { review_progress::load_store(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.root.as_ref() != Some(&root)
                    || this.reading.review_edits != edits
                    || this.reading.store_generation != generation
                {
                    return;
                }
                match result {
                    Ok(records) => {
                        this.reading.cancel.store(true, Ordering::Relaxed);
                        this.reading.generation += 1;
                        this.reading.task = None;
                        this.reading.saved.reviewed = records
                            .into_iter()
                            .filter(|record| {
                                roots.iter().any(|root| {
                                    record.repo.starts_with(root) || root.starts_with(&record.repo)
                                })
                            })
                            .take(review_progress::MAX_REVIEWED)
                            .collect();
                        this.reading.statuses =
                            vec![ReviewStatus::Pending; this.reading.saved.reviewed.len()];
                        this.refresh_review(None, window, cx);
                        this.remember_reading(false, window, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        this.message = format!("没能读取仓库审阅记录：{error}");
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    fn persist_review_checkpoint(
        &mut self,
        record: ReviewCheckpoint,
        reviewed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = crate::session::review_path(cx) else {
            return;
        };
        let previous = self.reading.store_tail.take();
        let (complete, tail) = async_channel::bounded::<()>(1);
        self.reading.store_tail = Some(tail.clone());
        crate::session::track_review_write(window, tail, cx);
        let job = cx.background_spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.recv().await;
            }
            let result = review_progress::update_store(&path, &record, reviewed);
            drop(complete);
            result
        });
        // The detached write outlives this workspace window; app quit awaits its tail.
        cx.spawn(async move |this, cx| match job.await {
            Ok(()) => cx.update_global::<ReviewStoreChanged, _>(|_, _| {}),
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    this.message = format!("没能保存仓库审阅记录：{error}");
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Install before restore_tabs, which keeps its existing active-first/lazy-file behavior.
    pub(crate) fn restore_reading(
        &mut self,
        saved: ReadingSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reading.positions_pending = saved
            .positions
            .iter()
            .map(|position| position.path.clone())
            .collect();
        self.reading.diff_positions_pending = saved.pinned.clone();
        self.reading.statuses = vec![ReviewStatus::Pending; saved.reviewed.len()];
        self.reading.saved = saved;
        self.refresh_review(None, window, cx);
        self.load_review_progress(window, cx);
        self.restore_pinned_comparisons(window, cx);
    }

    /// Call after install_loaded and before a navigation request applies its own placement.
    pub(super) fn restore_document_reading_position(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.document(id) else {
            return;
        };
        let path = document.path.clone();
        let editor = document.editor.clone();
        if !self.reading.positions_pending.remove(&path) {
            return;
        }
        let Some(position) = self
            .reading
            .saved
            .positions
            .iter()
            .find(|position| position.path == path)
            .cloned()
        else {
            return;
        };
        editor.update(cx, |state, cx| {
            let line = position
                .line
                .min(state.text().lines_len().saturating_sub(1) as u32);
            state.set_cursor_position(lsp_types::Position::new(line, position.column), window, cx);
            state.set_scroll_offset(point(px(position.scroll_x), px(position.scroll_y)), cx);
        });
    }

    /// Call when a DiffDoc is installed. Kit clamps the saved row to the current document.
    pub(super) fn restore_diff_reading_position(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self
            .diff
            .tab
            .as_ref()
            .and_then(|tab| tab.checkpoint.as_ref())
        else {
            return;
        };
        let Some(index) = self
            .reading
            .diff_positions_pending
            .iter()
            .position(|saved| &saved.source == source)
        else {
            return;
        };
        let saved = self.reading.diff_positions_pending.remove(index);
        if let Some(inline) = saved.inline {
            self.diff.inline = inline;
        }
        // `top_row` is a source row; folded display rows may still be loading.
        self.diff_scroll_to_source(saved.top_row, ScrollStrategy::Top);
        cx.notify();
    }

    /// Called by tab/quit recording and editor scroll/cursor events; no IO when persist=false.
    pub(super) fn remember_reading(&self, persist: bool, window: &Window, cx: &mut Context<Self>) {
        let mut saved = self.reading.saved.clone();
        for document in self.documents.iter().filter(|document| !document.untitled) {
            let editor = document.editor.read(cx);
            let cursor = editor.text().offset_to_position(editor.cursor());
            let scroll = editor.scroll_offset();
            let position = FilePosition {
                path: document.path.clone(),
                line: cursor.line,
                column: cursor.character,
                scroll_x: f32::from(scroll.x),
                scroll_y: f32::from(scroll.y),
            };
            saved.positions.retain(|saved| saved.path != position.path);
            saved.positions.push(position);
        }
        saved.positions.retain(|position| {
            self.documents
                .iter()
                .any(|document| document.path == position.path)
                || self.pending_tabs.contains(&position.path)
        });
        saved.positions.truncate(review_progress::MAX_POSITIONS);
        if !self.reading.restoring_pins {
            saved.pinned = self
                .archived_diffs
                .iter()
                .filter_map(|saved| pinned(&saved.pane))
                .collect();
            if self.diff_pinned
                && let Some(current) = pinned(&self.diff)
            {
                saved.pinned.push(current);
            }
            saved.pinned.truncate(review_progress::MAX_PINNED);
        }
        crate::session::remember_reading(window, saved, persist, cx);
    }

    /// Invoke for native watch batches (None means the bounded watcher overflowed), and on
    /// explicit refresh. Invalidation happens before background work, including equal-mtime edits.
    pub(super) fn refresh_review(
        &mut self,
        paths: Option<&[PathBuf]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let records: Vec<_> = self
            .reading
            .saved
            .reviewed
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                paths.is_none_or(|paths| {
                    paths.iter().any(|path| {
                        let full = record.repo.join(&record.path);
                        full.starts_with(path) || path.starts_with(&full) || path == &record.repo
                    })
                })
            })
            .map(|(index, record)| (index, record.clone()))
            .collect();
        if records.is_empty() {
            return;
        }
        self.reading.cancel.store(true, Ordering::Relaxed);
        self.reading.cancel = Arc::default();
        self.reading.generation += 1;
        let generation = self.reading.generation;
        for (index, _) in &records {
            if let Some(status) = self.reading.statuses.get_mut(*index) {
                *status = ReviewStatus::Pending;
            }
        }
        let cancel = self.reading.cancel.clone();
        let job = cx.background_spawn(async move {
            let checkpoints: Vec<_> = records.iter().map(|(_, record)| record.clone()).collect();
            let statuses = review_progress::verify_checkpoints(&checkpoints, &cancel);
            records
                .into_iter()
                .zip(statuses)
                .map(|((index, _), status)| (index, status))
                .collect::<Vec<_>>()
        });
        self.reading.task = Some(cx.spawn_in(window, async move |this, cx| {
            let statuses = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.reading.generation != generation {
                    return;
                }
                for (index, status) in statuses {
                    if let Some(current) = this.reading.statuses.get_mut(index) {
                        *current = status;
                    }
                }
                this.reading.task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn review_path(&self) -> Option<&Path> {
        match self.active {
            Pane::Document(id) => self
                .document(id)
                .filter(|document| !document.untitled)
                .map(|document| document.path.as_path()),
            Pane::Diff => self.diff.tab.as_ref().map(|tab| tab.path.as_path()),
            _ => None,
        }
    }

    fn review_index(&self, path: &Path) -> Option<usize> {
        self.reading
            .saved
            .reviewed
            .iter()
            .position(|record| record.repo.join(&record.path) == path)
    }

    pub(super) fn review_status(&self, path: &Path) -> &'static str {
        match self
            .review_index(path)
            .and_then(|index| self.reading.statuses.get(index))
        {
            Some(ReviewStatus::Reviewed) => "已读",
            Some(ReviewStatus::Unavailable(_)) => "待审 · 文件不可用",
            _ => "待审",
        }
    }

    pub(super) fn mark_document_reviewed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(document) = self
            .document(id)
            .filter(|document| !document.untitled && !document.dirty && !document.deleted)
        else {
            return;
        };
        let Some(displayed) = document.disk else {
            return;
        };
        let path = document.path.clone();
        let version = document.version;
        let Some(repo) = self
            .groups
            .iter()
            .filter(|group| path.starts_with(&group.repo.worktree))
            .max_by_key(|group| group.repo.worktree.components().count())
            .map(|group| group.repo.worktree.clone())
        else {
            self.message = "文件不属于已打开的仓库".into();
            cx.notify();
            return;
        };
        self.reading.cancel.store(true, Ordering::Relaxed);
        self.reading.cancel = Arc::default();
        self.reading.generation += 1;
        let generation = self.reading.generation;
        self.reading.review_edits += 1;
        let wanted = path.clone();
        let job = cx.background_spawn(async move {
            ReviewCheckpoint::capture(&repo, &wanted, Some(displayed))
        });
        self.reading.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reading.generation != generation {
                    return;
                }
                this.reading.task = None;
                if !this.document(id).is_some_and(|document| {
                    document.path == path
                        && document.version == version
                        && !document.dirty
                        && !document.deleted
                }) {
                    return;
                }
                match result {
                    Ok(record) => {
                        if let Some(index) = this.review_index(&path) {
                            this.reading.saved.reviewed.remove(index);
                            this.reading.statuses.remove(index);
                        }
                        if this.reading.saved.reviewed.len() == review_progress::MAX_REVIEWED {
                            this.reading.saved.reviewed.remove(0);
                            this.reading.statuses.remove(0);
                        }
                        this.persist_review_checkpoint(record.clone(), true, window, cx);
                        this.reading.saved.reviewed.push(record);
                        this.reading.statuses.push(ReviewStatus::Reviewed);
                        this.message = "已标记已读".into();
                        this.remember_reading(true, window, cx);
                    }
                    Err(error) => this.message = format!("未能标记已读：{error}"),
                }
                cx.notify();
            });
        }));
    }

    fn clear_reviewed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.review_path().map(Path::to_path_buf) else {
            return;
        };
        let Some(index) = self.review_index(&path) else {
            return;
        };
        self.reading.cancel.store(true, Ordering::Relaxed);
        self.reading.generation += 1;
        self.reading.task = None;
        self.reading.review_edits += 1;
        let record = self.reading.saved.reviewed.remove(index);
        self.persist_review_checkpoint(record, false, window, cx);
        self.reading.statuses.remove(index);
        self.remember_reading(true, window, cx);
        cx.notify();
    }

    /// Mount in the file tab toolbar and Diff actions. A Diff opens the current file for
    /// explicit review; historical/index text is never marked as the current disk version.
    pub(super) fn render_review_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(path) = self.review_path().map(Path::to_path_buf) else {
            return div().into_any_element();
        };
        let status = self.review_status(&path);
        let can_mark = match self.active {
            Pane::Document(id) => self.document(id).is_some_and(|document| {
                !document.dirty && !document.deleted && document.disk.is_some()
            }),
            _ => false,
        };
        let message = self
            .review_index(&path)
            .and_then(|index| self.reading.statuses.get(index))
            .and_then(|status| match status {
                ReviewStatus::Unavailable(error) => Some(error.clone()),
                _ => None,
            });
        let mut bar = h_flex()
            .gap_2()
            .text_size(theme::TEXT_CAPTION)
            .child(div().text_color(theme::colors(cx).muted).child(status));
        if let Some(message) = message {
            bar = bar.child(
                Button::new("review-error")
                    .small()
                    .ghost()
                    .label("查看原因")
                    .accessibility_label("查看审阅状态原因")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.message = message.clone();
                        cx.notify();
                    })),
            );
        }
        if self.active == Pane::Diff {
            bar = bar.child(
                Button::new("review-open-current")
                    .small()
                    .ghost()
                    .label("打开文件审阅")
                    .accessibility_label("打开当前磁盘文件进行审阅")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let root = this.root_for(&path);
                        this.open_file(path.clone(), root, window, cx);
                    })),
            )
        } else {
            bar = bar.child(
                Button::new("review-mark")
                    .small()
                    .ghost()
                    .label("标记已读")
                    .accessibility_label("标记当前文件已读")
                    .disabled(!can_mark)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.mark_document_reviewed(window, cx)),
                    ),
            );
        }
        if self
            .review_path()
            .is_some_and(|path| self.review_index(path).is_some())
        {
            bar = bar.child(
                Button::new("review-clear")
                    .small()
                    .ghost()
                    .label("标记待审")
                    .accessibility_label("标记当前文件待审")
                    .on_click(cx.listener(|this, _, window, cx| this.clear_reviewed(window, cx))),
            );
        }
        bar.into_any_element()
    }

    fn restore_pinned_comparisons(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.reading.saved.pinned.clone();
        if saved.is_empty() {
            return;
        }
        self.reading.restoring_pins = true;
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            for saved in saved {
                let source = saved.source.clone();
                let service = service.clone();
                let job = cx.background_spawn(async move { restore_comparison(&source, &service) });
                let result = job.await;
                if this
                    .update_in(cx, |this, window, cx| {
                        match result {
                            Ok(tab) => {
                                // A restored tab starts lazy, preserving both focus and in-flight opens.
                                let mut pane = diff_view::DiffPane::new(window, cx);
                                pane.tab = Some(tab);
                                if let Some(inline) = saved.inline {
                                    pane.inline = inline;
                                }
                                pane.loading = true;
                                // The pinned source row is applied once the loaded rows are folded.
                                this.archived_diffs.push(diff_tabs::SavedDiff {
                                    id: DocumentId::allocate(),
                                    pane,
                                    image_diff: None,
                                });
                                cx.notify();
                            }
                            Err(error) => {
                                this.message = format!("没能恢复固定比较：{error}");
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.reading.restoring_pins = false;
                this.remember_reading(false, window, cx);
            });
        })
        .detach();
    }
}

fn pinned(pane: &diff_view::DiffPane) -> Option<PinnedComparison> {
    let top_row = pane.visible_top_row();
    Some(PinnedComparison {
        source: pane.tab.as_ref()?.checkpoint.clone()?,
        top_row: pane.source_row(top_row),
        inline: Some(pane.inline),
    })
}

fn restore_comparison(source: &ComparisonSource, service: &GitService) -> io::Result<DiffTab> {
    use workspace_editor_git::ComparisonMode;
    if !source.valid() {
        return Err(io::Error::other("比较来源无效"));
    }
    let cancel = AtomicBool::new(false);
    let path = source.path();
    let diff_source = match source {
        ComparisonSource::Commit {
            repo,
            commit,
            parent,
            path,
            original_path,
        } => DiffSource::Git(Request {
            repo: service.identify(repo, &cancel)?,
            generation: 0,
            operation: Operation::CommitDiff {
                commit: commit.clone(),
                parent: parent.clone(),
                path: path.clone(),
                original_path: original_path.clone(),
            },
        }),
        ComparisonSource::History {
            repo,
            commit,
            path: historical_path,
            current_path,
        } => {
            let repo = service.identify(repo, &cancel)?;
            let bytes = service.read_blob(&repo, commit, historical_path, &cancel)?;
            if crate::image_file::supported(historical_path) {
                DiffSource::Local("".into(), "恢复的历史图片")
            } else {
                let old = super::comparison::history_text(&bytes)?;
                if let Some(current_path) = current_path {
                    let new = crate::files::text_file(
                        Some(&repo.worktree),
                        &repo.worktree.join(current_path),
                    )?
                    .text;
                    DiffSource::Local(
                        super::comparison::local_patch(&old, &new, current_path),
                        "历史版本 → 工作区文件",
                    )
                } else {
                    DiffSource::Snapshot(old.into())
                }
            }
        }
        ComparisonSource::Files { left, right } => {
            if crate::image_file::supported(left) || crate::image_file::supported(right) {
                for path in [left, right] {
                    let metadata = std::fs::metadata(path)?;
                    if !metadata.is_file() || metadata.len() > crate::image_file::MAX_FILE_BYTES {
                        return Err(io::Error::other("固定图片比较来源不是受支持的普通文件"));
                    }
                }
                DiffSource::Local("".into(), "恢复的图片比较")
            } else {
                let old = crate::files::text_file(None, left)?.text;
                let new = crate::files::text_file(None, right)?.text;
                DiffSource::Local(
                    super::comparison::local_patch(&old, &new, right),
                    "恢复的文件比较",
                )
            }
        }
        ComparisonSource::Git {
            repo,
            base,
            target,
            path,
            original_path,
        } => {
            let repo = service.identify(repo, &cancel)?;
            let comparison =
                service.compare_revisions(&repo, base, target, ComparisonMode::Direct, &cancel)?;
            let file = comparison
                .files
                .iter()
                .find(|file| &file.path == path && &file.original_path == original_path)
                .ok_or_else(|| io::Error::other("比较双方已无该文件差异，请重新选择比较来源"))?;
            let patch = service.comparison_patch(&repo, &comparison, Some(file), &cancel)?;
            DiffSource::Local(
                String::from_utf8(patch)
                    .map_err(|_| io::Error::other("比较补丁不是 UTF-8"))?
                    .into(),
                "恢复的 Git 版本比较",
            )
        }
        ComparisonSource::GitChange {
            repo,
            path,
            original_path,
            staged,
            untracked,
        } => {
            let repo = service.identify(repo, &cancel)?;
            let operation = if *untracked {
                Operation::UntrackedDiff { path: path.clone() }
            } else {
                Operation::Diff {
                    side: if *staged {
                        workspace_editor_git::DiffSide::Staged
                    } else {
                        workspace_editor_git::DiffSide::Worktree
                    },
                    path: path.clone(),
                    original_path: original_path.clone(),
                }
            };
            DiffSource::Git(Request {
                repo,
                generation: 0,
                operation,
            })
        }
    };
    Ok(DiffTab {
        label: format!(
            "固定比较 {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
        tooltip: format!("重新读取的比较 · {}", path.display()),
        path,
        source: diff_source,
        checkpoint: Some(source.clone()),
    })
}

#[cfg(test)]
#[path = "reading_state_tests.rs"]
mod tests;
