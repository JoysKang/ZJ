//! A fixed comparison keeps its own document, selection and viewport.
use super::*;

pub(super) struct SavedDiff {
    pub(super) id: DocumentId,
    pub(super) pane: diff_view::DiffPane,
    pub(super) image_diff: Option<image_view::ImageComparison>,
}

impl Workbench {
    pub(super) fn pin_current_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.tab.is_none() {
            return;
        }
        if self.archived_diffs.len() >= 8 {
            self.message = "最多保留 8 份固定比较，请先关闭不再需要的 Diff".into();
            cx.notify();
            return;
        }
        self.diff_pinned = true;
        self.remember_tabs(true, window, cx);
        cx.notify();
    }

    /// A new preview only replaces an unfixed preview. In-flight work is cancelled before moving it.
    pub(super) fn prepare_new_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_file_open();
        if self.diff_pinned && self.diff.tab.is_some() {
            self.diff.cancel.store(true, Ordering::Relaxed);
            self.diff.task = None;
            let mut next = diff_view::DiffPane::new(window, cx);
            let snapshot = self.diff.review.comparison.snapshot.take();
            // The repository comparison/history panel remains available while opening its next file.
            std::mem::swap(
                &mut next.review.comparison,
                &mut self.diff.review.comparison,
            );
            self.diff.review.comparison.snapshot = snapshot;
            let pane = std::mem::replace(&mut self.diff, next);
            let id = DocumentId::allocate();
            for key in &mut self.tabs.order {
                if *key == editor_area::TabKey::Pane(Pane::Diff) {
                    *key = editor_area::TabKey::Pane(Pane::SavedDiff(id));
                }
            }
            self.archived_diffs.push(SavedDiff {
                id,
                pane,
                image_diff: self.image_diff.take(),
            });
        } else {
            self.diff.cancel.store(true, Ordering::Relaxed);
            self.diff.task = None;
            self.diff.tab = None;
            self.diff.doc = None;
            self.diff.raw = None;
            self.diff.source = None;
            self.diff.fallback = None;
            self.diff.selection = None;
            self.diff.caret = None;
            self.diff.change = None;
            self.clear_image_diff(window);
        }
        self.diff_pinned = false;
        self.diff_review_changed(window, cx);
    }

    pub(super) fn activate_saved_diff(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.archived_diffs.iter().position(|saved| saved.id == id) else {
            return;
        };
        let saved = self.archived_diffs.remove(index);
        self.prepare_new_diff(window, cx);
        self.tabs
            .order
            .retain(|key| *key != editor_area::TabKey::Pane(Pane::Diff));
        for key in &mut self.tabs.order {
            if *key == editor_area::TabKey::Pane(Pane::SavedDiff(id)) {
                *key = editor_area::TabKey::Pane(Pane::Diff);
            }
        }
        self.diff = saved.pane;
        self.image_diff = saved.image_diff;
        self.diff_pinned = true;
        self.show_pane(Pane::Diff, window, cx);
        if self.diff.review.comparison.snapshot.is_none()
            && (self.diff.loading
                || self.diff.stale
                || self.diff.source.is_none()
                || self
                    .diff
                    .tab
                    .as_ref()
                    .is_some_and(|tab| matches!(tab.source, DiffSource::Git(_))))
        {
            self.load_diff(window, cx);
        }
    }

    pub(super) fn close_saved_diff(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.archived_diffs.iter().position(|saved| saved.id == id) {
            let saved = self.archived_diffs.remove(index);
            saved.pane.cancel.store(true, Ordering::Relaxed);
            if let Some(images) = saved.image_diff {
                for decoded in [images.left, images.right]
                    .into_iter()
                    .filter_map(|side| side.ok().flatten())
                {
                    let _ = window.drop_image(decoded.image);
                }
            }
            self.remember_tabs(true, window, cx);
            cx.notify();
        }
    }
}
