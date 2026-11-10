//! VS Code-style diff editor over a [`DiffDoc`]: two virtualized lists side by side that share
//! one scroll handle (so vertical and horizontal offsets stay in step without timers), or a
//! single inline list. Rows only slice precomputed text and style runs; nothing is parsed or
//! highlighted during render.
use super::diff_ops::{self, Block, BlockKind, CopyDiff, HunkAction, SelectAllDiff};
use super::*;
use crate::diff_doc::{DiffDoc, LineKind, RowKind, Side};
use crate::diff_doc::{DisplayRow, Whitespace};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Sizable, Theme,
        button::{Button, ButtonVariants},
        h_flex,
        scroll::Scrollbar,
        v_flex,
    },
};
use std::rc::Rc;

static NEXT_REVIEW_REQUEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub(super) struct DiffReview {
    pub(super) comparison: super::comparison::ComparisonState,
    find: Entity<InputState>,
    find_open: bool,
    hits: Vec<(bool, u32, std::ops::Range<usize>)>,
    current: Option<usize>,
    whitespace: Whitespace,
    collapse: bool,
    document: Option<Arc<DiffDoc>>,
    pair: Vec<DisplayRow>,
    inline: Vec<DisplayRow>,
    pair_changes: Vec<usize>,
    inline_changes: Vec<usize>,
    generation: u64,
    _subscription: Subscription,
}

impl DiffReview {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let find = cx.new(|cx| InputState::new(window, cx).placeholder("在 Diff 两侧查找"));
        let subscription = cx.subscribe_in(
            &find,
            window,
            |this: &mut Workbench, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.diff.review.current = None;
                    this.diff_review_changed(window, cx);
                }
                InputEvent::PressEnter { shift, .. } => this.diff_find_step(!shift, cx),
                _ => {}
            },
        );
        Self {
            comparison: super::comparison::ComparisonState::new(window, cx),
            find,
            find_open: false,
            hits: Vec::new(),
            current: None,
            whitespace: Whitespace::Exact,
            collapse: true,
            document: None,
            pair: Vec::new(),
            inline: Vec::new(),
            pair_changes: Vec::new(),
            inline_changes: Vec::new(),
            generation: 0,
            _subscription: subscription,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffList {
    Original,
    Modified,
    Inline,
}

/// A scroll that waits for the rows it belongs to.
#[derive(Clone, Copy)]
pub(super) enum PendingScroll {
    /// A source row, mapped to its display row once folding is ready.
    Row(usize, ScrollStrategy),
    /// The first change of the layout being built; resolved once its change list exists,
    /// so whitespace filtering and the chosen layout apply.
    FirstChange,
}

#[derive(Clone, Copy)]
enum Fill {
    Plain,
    Removed,
    Added,
    Filler,
}

#[derive(Clone)]
struct Paint {
    colors: theme::Colors,
    font: SharedString,
    metrics: theme::DiffMetrics,
    width: Pixels,
}

impl Workbench {
    pub(super) fn diff_reading_allows_writes(&self) -> bool {
        self.diff.review.whitespace == Whitespace::Exact
    }

    fn diff_review_current(&self) -> bool {
        self.diff
            .doc
            .as_ref()
            .zip(self.diff.review.document.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
    }

    pub(super) fn diff_display_rows(&self, inline: bool) -> Option<&[DisplayRow]> {
        self.diff_review_current().then_some({
            if inline {
                self.diff.review.inline.as_slice()
            } else {
                self.diff.review.pair.as_slice()
            }
        })
    }

    pub(super) fn diff_reveal_source(
        &mut self,
        source: usize,
        inline: bool,
        cx: &mut Context<Self>,
    ) -> usize {
        if self.diff_display_rows(inline).is_some_and(|rows| {
            rows.iter().any(|row| {
                row.folded
                    .is_some_and(|end| source >= row.source && source < end)
            })
        }) {
            self.diff.review.collapse = false;
            self.diff_refresh_review(cx);
        }
        self.diff_visible_row(source, inline)
    }

    pub(super) fn diff_visible_row(&self, source: usize, inline: bool) -> usize {
        self.diff_display_rows(inline).map_or(source, |rows| {
            rows.iter()
                .position(|row| row.source >= source || row.folded.is_some_and(|end| source < end))
                .unwrap_or(rows.len().saturating_sub(1))
        })
    }

    /// Called after installing a new DiffDoc as well as after changing reading options.
    pub(super) fn diff_review_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.diff_refresh_review(cx);
    }

    fn diff_refresh_review(&mut self, cx: &mut Context<Self>) {
        self.diff.review.generation = NEXT_REVIEW_REQUEST.fetch_add(1, Ordering::Relaxed);
        let generation = self.diff.review.generation;
        let top_source = self.diff_display_rows(self.diff_is_inline()).map(|rows| {
            let visible = self.diff.visible_top_row();
            rows.get(visible).map_or(visible, |row| row.source)
        });
        self.diff.review.hits.clear();
        self.diff.review.document = None;
        let Some(doc) = self.diff.doc.clone() else {
            self.diff.review.find_open = false;
            self.diff.review.pair = Vec::new();
            self.diff.review.inline = Vec::new();
            self.diff.review.pair_changes = Vec::new();
            self.diff.review.inline_changes = Vec::new();
            cx.notify();
            return;
        };
        let query = self.diff.review.find.read(cx).value().to_string();
        let whitespace = self.diff.review.whitespace;
        let collapse = self.diff.review.collapse;
        if whitespace == Whitespace::Exact && !collapse && !self.diff.review.find_open {
            self.diff.review.pair = Vec::new();
            self.diff.review.inline = Vec::new();
            self.diff.review.pair_changes = Vec::new();
            self.diff.review.inline_changes = Vec::new();
            if let Some(pending) = self.diff.pending_scroll.take() {
                match pending {
                    PendingScroll::Row(source, strategy) => {
                        self.diff.scroll.scroll_to_item_strict(source, strategy);
                    }
                    PendingScroll::FirstChange => {
                        let first = self.diff_change_starts().first().copied();
                        if let Some(row) = first {
                            self.diff
                                .scroll
                                .scroll_to_item_strict(row, ScrollStrategy::Center);
                        }
                    }
                }
            } else if let Some(source) = top_source {
                self.diff
                    .scroll
                    .scroll_to_item_strict(source, ScrollStrategy::Top);
            }
            cx.notify();
            return;
        }
        let wanted = doc.clone();
        let job = cx.background_spawn(async move {
            let pair = doc.display_rows(false, whitespace, collapse);
            let inline = doc.display_rows(true, whitespace, collapse);
            let changes = |rows: &[DisplayRow], inline: bool| {
                let mut result = Vec::new();
                let mut previous = false;
                for (i, row) in rows.iter().enumerate() {
                    let changed = !row.quiet
                        && row.folded.is_none()
                        && if inline {
                            doc.inline[row.source].kind != LineKind::Same
                        } else {
                            doc.rows[row.source].kind == RowKind::Changed
                        };
                    if changed && !previous {
                        result.push(i);
                    }
                    previous = changed;
                }
                result
            };
            let pair_changes = changes(&pair, false);
            let inline_changes = changes(&inline, true);
            let hits = doc.find_lines(&query, 19_999);
            (pair, inline, pair_changes, inline_changes, hits)
        });
        cx.spawn(async move |this, cx| {
            let (pair, inline, pair_changes, inline_changes, hits) = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.diff.review.generation != generation
                    || !this
                        .diff
                        .doc
                        .as_ref()
                        .is_some_and(|doc| Arc::ptr_eq(doc, &wanted))
                {
                    return;
                }
                let pending = this.diff.pending_scroll.take();
                let review = &mut this.diff.review;
                review.document = Some(wanted);
                review.pair = pair;
                review.inline = inline;
                review.pair_changes = pair_changes;
                review.inline_changes = inline_changes;
                review.hits = hits;
                review.current = review.current.filter(|&i| i < review.hits.len());
                let find_open = review.find_open && !review.hits.is_empty();
                if pending.is_none() {
                    this.diff.change = None;
                }
                if find_open {
                    this.diff_reveal_hit(cx);
                } else if let Some(pending) = pending {
                    match pending {
                        PendingScroll::Row(source, strategy) => {
                            let row = this.diff_visible_row(source, this.diff_is_inline());
                            this.diff.scroll.scroll_to_item_strict(row, strategy);
                        }
                        PendingScroll::FirstChange => {
                            let first = this.diff_change_starts().first().copied();
                            if let Some(row) = first {
                                this.diff
                                    .scroll
                                    .scroll_to_item_strict(row, ScrollStrategy::Center);
                            }
                        }
                    }
                } else if let Some(source) = top_source {
                    let (source, strategy) = this
                        .diff
                        .selection
                        .map_or((source, ScrollStrategy::Top), |selection| {
                            (selection.head, ScrollStrategy::Center)
                        });
                    let row = this.diff_visible_row(source, this.diff_is_inline());
                    this.diff.scroll.scroll_to_item_strict(row, strategy);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn diff_open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.diff.doc.is_none() {
            return;
        }
        self.diff.review.find_open = true;
        self.diff.review.find.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.diff_review_changed(window, cx);
    }

    pub(super) fn diff_find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let review = &mut self.diff.review;
        if review.hits.is_empty() {
            return;
        }
        let count = review.hits.len();
        review.current = Some(match review.current {
            None if forward => 0,
            None => count - 1,
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
        });
        self.diff_reveal_hit(cx);
    }

    fn diff_reveal_hit(&mut self, cx: &mut Context<Self>) {
        let review = &mut self.diff.review;
        if review.hits.is_empty() {
            return;
        }
        let current = *review.current.get_or_insert(0);
        let (modified, line, _) = review.hits[current];
        let Some(doc) = &self.diff.doc else { return };
        let inline = self.diff_is_inline();
        let source = if inline {
            doc.inline.iter().position(|r| {
                if modified {
                    (r.kind == LineKind::Added && r.line == line)
                        || (r.kind == LineKind::Same && r.other == Some(line))
                } else {
                    r.kind != LineKind::Added && r.line == line
                }
            })
        } else {
            doc.rows.iter().position(|r| {
                if modified {
                    r.new == Some(line)
                } else {
                    r.old == Some(line)
                }
            })
        };
        if let Some(source) = source {
            if self.diff_display_rows(inline).is_some_and(|rows| {
                rows.iter().any(|r| {
                    r.folded
                        .is_some_and(|end| source >= r.source && source < end)
                })
            }) {
                self.diff.review.collapse = false;
                self.diff_refresh_review(cx);
                return;
            }
            let row = self.diff_visible_row(source, inline);
            self.diff
                .scroll
                .scroll_to_item_strict(row, ScrollStrategy::Center);
            self.diff.selection = Some(diff_ops::DiffSelection {
                list: if inline {
                    DiffList::Inline
                } else if modified {
                    DiffList::Modified
                } else {
                    DiffList::Original
                },
                anchor: source,
                head: source,
                columns: None,
            });
        }
        cx.notify();
    }

    fn render_diff_find(&self, cx: &mut Context<Self>) -> AnyElement {
        let review = &self.diff.review;
        let label = if review.hits.is_empty() {
            "无结果".into()
        } else {
            format!(
                "{} / {}{}",
                review.current.map_or(1, |i| i + 1),
                review.hits.len(),
                if review.hits.len() == 19_999 { "+" } else { "" }
            )
        };
        h_flex()
            .w_full()
            .flex_shrink_0()
            .gap_2()
            .p_2()
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                this.diff.review.find_open = false;
                this.diff_refresh_review(cx);
                this.diff.focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&review.find).small()),
            )
            .child(div().text_size(theme::TEXT_CAPTION).child(label))
            .child(
                Button::new("diff-find-previous")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowUp)
                    .accessibility_label("上一个匹配")
                    .on_click(cx.listener(|this, _, _, cx| this.diff_find_step(false, cx))),
            )
            .child(
                Button::new("diff-find-next")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ArrowDown)
                    .accessibility_label("下一个匹配")
                    .on_click(cx.listener(|this, _, _, cx| this.diff_find_step(true, cx))),
            )
            .child(
                Button::new("diff-find-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .accessibility_label("关闭 Diff 查找")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.diff.review.find_open = false;
                        this.diff_refresh_review(cx);
                        this.diff.focus.focus(window, cx);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Inline when chosen, and always for added or deleted files (one side is empty).
    pub(super) fn diff_is_inline(&self) -> bool {
        self.diff.doc.as_ref().is_none_or(|doc| {
            self.diff.inline || doc.old.lines.is_empty() || doc.new.lines.is_empty()
        })
    }

    pub(super) fn diff_change_starts(&self) -> &[usize] {
        if self.diff_review_current() {
            return if self.diff_is_inline() {
                &self.diff.review.inline_changes
            } else {
                &self.diff.review.pair_changes
            };
        }
        match &self.diff.doc {
            Some(doc) if self.diff_is_inline() => &doc.inline_changes,
            Some(doc) => &doc.changes,
            None => &[],
        }
    }

    /// Reveals the first change of a freshly opened diff.
    pub(super) fn reveal_first_change(&mut self) {
        self.diff.change = None;
        let Some(&start) = self.diff_change_starts().first() else {
            return;
        };
        self.diff.change = Some(0);
        if self.diff_review_current() {
            self.diff
                .scroll
                .scroll_to_item_strict(start, ScrollStrategy::Center);
        } else {
            // The change list is still in source rows; folded rows arrive after the load.
            self.diff_scroll_to_source(start, ScrollStrategy::Center);
        }
    }

    /// Scrolls to a source row, waiting for folded display rows when a diff just loaded.
    pub(super) fn diff_scroll_to_source(&mut self, source: usize, strategy: ScrollStrategy) {
        if self.diff_display_rows(self.diff_is_inline()).is_some() {
            let row = self.diff_visible_row(source, self.diff_is_inline());
            self.diff.scroll.scroll_to_item_strict(row, strategy);
            self.diff.pending_scroll = None;
        } else {
            self.diff.pending_scroll = Some(PendingScroll::Row(source, strategy));
        }
    }

    pub(super) fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let starts = self.diff_change_starts();
        let count = starts.len();
        if count == 0 {
            return;
        }
        let next = match self.diff.change {
            None if forward => 0,
            None => count - 1,
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
        };
        let row = starts[next];
        self.diff.change = Some(next);
        self.diff
            .scroll
            .scroll_to_item_strict(row, ScrollStrategy::Center);
        cx.notify();
    }

    pub(super) fn toggle_diff_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_diff_layout(!self.diff.inline, window, cx);
    }

    /// Chosen by the toolbar toggle and by the Settings tab, which both save the preference.
    pub(super) fn set_diff_layout(
        &mut self,
        inline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_settings(window, cx, |settings| settings.diff_inline = inline);
        self.apply_diff_layout(inline, cx);
    }

    /// Another window (or the settings file) chose the layout: follow it without writing the
    /// setting again, which would notify every window back.
    pub(super) fn follow_saved_diff_layout(&mut self, inline: bool, cx: &mut Context<Self>) {
        self.apply_diff_layout(inline, cx);
    }

    /// Selection and caret are row-indexed, so they cannot survive the switch.
    fn apply_diff_layout(&mut self, inline: bool, cx: &mut Context<Self>) {
        if self.diff.inline == inline {
            return;
        }
        self.diff.inline = inline;
        self.diff.selection = None;
        self.diff.dragging = false;
        self.diff.caret = None;
        self.move_diff_layout_scroll(cx);
    }

    fn move_diff_layout_scroll(&mut self, cx: &mut Context<Self>) {
        // Row indexes differ between the layouts; keep the current change in view.
        self.diff.scroll = UniformListScrollHandle::new();
        if let Some(index) = self.diff.change
            && let Some(&row) = self.diff_change_starts().get(index)
        {
            self.diff
                .scroll
                .scroll_to_item_strict(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Actions stay with the diff context instead of taking space from file tabs.
    pub(super) fn render_diff_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let changes = !self.diff_change_starts().is_empty();
        let one_sided = self
            .diff
            .doc
            .as_ref()
            .is_none_or(|doc| doc.old.lines.is_empty() || doc.new.lines.is_empty());
        let inline = self.diff_is_inline();
        let action = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .flex_shrink_0()
                .icon(icon)
                .tooltip(label)
                .accessibility_label(label)
        };
        h_flex()
            .id("diff-actions")
            .test_support()
            .min_h(theme::CODE_HEADER_HEIGHT)
            .max_w_full()
            .flex_wrap()
            .gap_1()
            .when_some(self.diff.doc.as_ref(), |bar, doc| {
                bar.child(
                    div()
                        .px_1()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(format!("+{} −{}", doc.added, doc.removed)),
                )
            })
            .when(
                self.diff_partial_ok() && self.diff_selection_rows().is_some(),
                |bar| {
                    let rows = self.diff_selection_rows().unwrap_or(0..0);
                    if self.diff_staged() {
                        bar.child(
                            action("diff-unstage-lines", IconName::Minus, "取消暂存所选范围")
                                .on_click(cx.listener({
                                    let rows = rows.clone();
                                    move |this, _, window, cx| {
                                        this.diff_apply_rows(
                                            HunkAction::Unstage,
                                            rows.clone(),
                                            window,
                                            cx,
                                        )
                                    }
                                })),
                        )
                    } else {
                        bar.child(
                            action("diff-revert-lines", IconName::Undo2, "还原所选范围").on_click(
                                cx.listener({
                                    let rows = rows.clone();
                                    move |this, _, window, cx| {
                                        this.diff_apply_rows(
                                            HunkAction::Revert,
                                            rows.clone(),
                                            window,
                                            cx,
                                        )
                                    }
                                }),
                            ),
                        )
                        .child(
                            action("diff-stage-lines", IconName::Plus, "暂存所选范围").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    this.diff_apply_rows(
                                        HunkAction::Stage,
                                        rows.clone(),
                                        window,
                                        cx,
                                    )
                                }),
                            ),
                        )
                    }
                },
            )
            .when(!self.diff_reading_allows_writes(), |bar| {
                bar.child(
                    div()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child("恢复全部空白差异后可按行暂存／还原"),
                )
            })
            .child(
                action("diff-previous", IconName::ArrowUp, "上一个更改")
                    .disabled(!changes)
                    .on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx))),
            )
            .child(
                action("diff-next", IconName::ArrowDown, "下一个更改")
                    .disabled(!changes)
                    .on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx))),
            )
            .child(
                action(
                    "diff-layout",
                    if inline {
                        IconName::Columns2
                    } else {
                        IconName::Rows2
                    },
                    if inline {
                        "切换到并排视图"
                    } else {
                        "切换到内联视图"
                    },
                )
                .disabled(one_sided)
                .on_click(cx.listener(|this, _, window, cx| this.toggle_diff_layout(window, cx))),
            )
            .child(
                action("diff-find", IconName::Search, "在 Diff 中查找")
                    .disabled(self.diff.doc.is_none())
                    .on_click(cx.listener(|this, _, window, cx| this.diff_open_find(window, cx))),
            )
            .child(
                Button::new("diff-whitespace")
                    .ghost()
                    .xsmall()
                    .label(match self.diff.review.whitespace {
                        Whitespace::Exact => "空白：全部显示",
                        Whitespace::Trailing => "忽略行尾空白",
                        Whitespace::All => "忽略全部空白",
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.diff.review.whitespace = match this.diff.review.whitespace {
                            Whitespace::Exact => Whitespace::Trailing,
                            Whitespace::Trailing => Whitespace::All,
                            Whitespace::All => Whitespace::Exact,
                        };
                        this.diff.selection = None;
                        this.diff_review_changed(window, cx);
                    })),
            )
            .child(
                Button::new("diff-collapse")
                    .ghost()
                    .xsmall()
                    .label(if self.diff.review.collapse {
                        "展开未变区域"
                    } else {
                        "折叠未变区域"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        if this.diff.review.collapse {
                            // Expanding keeps whatever source row was on top; land on the
                            // first change instead, as when the diff opens.
                            this.diff.pending_scroll = Some(PendingScroll::FirstChange);
                            this.diff.review.collapse = false;
                        } else {
                            this.diff.pending_scroll = None;
                            this.diff.review.collapse = true;
                        }
                        this.diff.selection = None;
                        this.diff_review_changed(window, cx);
                    })),
            )
            .child(
                Button::new("diff-export-patch")
                    .ghost()
                    .xsmall()
                    .label("导出补丁")
                    .disabled(self.diff.source.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.export_diff_patch(window, cx)),
                    ),
            )
            .child(
                Button::new("diff-open-patch")
                    .ghost()
                    .xsmall()
                    .label("打开补丁")
                    .on_click(cx.listener(|this, _, window, cx| this.open_patch_file(window, cx))),
            )
            .child(
                action("diff-copy-patch", IconName::Copy, "复制完整 Diff")
                    .disabled(self.diff.source.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some((patch, _)) = &this.diff.source {
                            cx.write_to_clipboard(ClipboardItem::new_string(patch.to_string()));
                        }
                    })),
            )
            .child(
                action("diff-open-file", IconName::File, "打开文件")
                    .disabled(self.diff_file_deleted())
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(path) = this.diff.tab.as_ref().map(|d| d.path.clone()) {
                            this.scm_open_file(&path, window, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    /// Known working-tree deletions have no file to open; historical paths use the normal flow.
    fn diff_file_deleted(&self) -> bool {
        self.diff.tab.as_ref().is_some_and(|tab| {
            self.groups.iter().any(|group| {
                group
                    .status
                    .as_ref()
                    .and_then(|status| status.as_ref().ok())
                    .is_some_and(|status| {
                        status.changes.iter().any(|change| {
                            group.repo.worktree.join(&change.path) == tab.path
                                && (change.worktree == b'D'
                                    || (change.index == b'D' && change.worktree == b'.'))
                        }) && !status.changes.iter().any(|change| {
                            group.repo.worktree.join(&change.path) == tab.path
                                && change.kind == ChangeKind::Untracked
                        })
                    })
            })
        })
    }

    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let header = h_flex()
            .id("diff-context")
            .test_support()
            .w_full()
            .min_h(theme::CODE_HEADER_HEIGHT)
            .flex_shrink_0()
            .flex_wrap()
            .px_2()
            .gap_x_2()
            .bg(colors.panel)
            .border_b_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION)
            .when_some(self.diff.tab.as_ref(), |bar, tab| {
                let repo = tab
                    .request()
                    .map(|request| &request.repo)
                    .or_else(|| self.active_group().map(|index| &self.groups[index].repo));
                let path = match repo {
                    Some(repo) => {
                        let name = repo
                            .worktree
                            .file_name()
                            .unwrap_or(repo.worktree.as_os_str())
                            .to_string_lossy();
                        let path = tab.path.strip_prefix(&repo.worktree).unwrap_or(&tab.path);
                        format!("{name} / {}", path.display())
                    }
                    None => self.relative(&tab.path).display().to_string(),
                };
                let comparison = match &tab.source {
                    DiffSource::Local(_, comparison) => (*comparison).to_string(),
                    DiffSource::Snapshot(_) => "历史完整内容（只读）".to_string(),
                    DiffSource::Git(request) => match &request.operation {
                        Operation::Diff {
                            side: DiffSide::Staged,
                            ..
                        } => "HEAD → 暂存区".to_string(),
                        Operation::Diff {
                            side: DiffSide::Worktree,
                            ..
                        } => "暂存区 → 工作区".to_string(),
                        Operation::UntrackedDiff { .. } => "未跟踪文件".to_string(),
                        Operation::CommitDiff { commit, parent, .. } => format!(
                            "{} → {}",
                            parent
                                .as_deref()
                                .map(|p| &p[..p.len().min(8)])
                                .unwrap_or("空版本"),
                            &commit[..commit.len().min(8)]
                        ),
                        Operation::Status => "Git Diff".to_string(),
                    },
                };
                let path = path.replace(SINGLE_LINE, "⏎");
                let tooltip = tab.tooltip.clone();
                bar.child(
                    h_flex()
                        .flex_1()
                        .min_w(theme::DIFF_CONTEXT_MIN_WIDTH)
                        .h(theme::CODE_HEADER_HEIGHT)
                        .gap_2()
                        .child(
                            div()
                                .id("diff-file-path")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_color(colors.foreground)
                                .aria_label(path.clone())
                                .child(path)
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                        .build(window, cx)
                                }),
                        )
                        .child(
                            div()
                                .id("diff-comparison")
                                .test_support()
                                .flex_shrink_0()
                                .text_color(colors.muted)
                                .aria_label(comparison.clone())
                                .child(comparison),
                        ),
                )
            })
            .child(self.render_diff_actions(cx));
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .child(header)
            .when(self.diff.review.find_open, |view| {
                view.child(self.render_diff_find(cx))
            })
            .when(self.diff.review.comparison.open, |view| {
                view.child(self.render_comparison_panel(cx))
            })
            .child(div().flex_1().min_h_0().child(self.render_diff_body(cx)))
            .into_any_element()
    }

    fn render_diff_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        if let Some(snapshot) = &self.diff.review.comparison.snapshot {
            return gpui_kit::component::input::Editor::new(snapshot)
                .readonly(true)
                .bordered(false)
                .size_full()
                .into_any_element();
        }
        let Some(doc) = &self.diff.doc else {
            // Binary, combined or failed diffs: Git's own text, or the status message.
            return match &self.diff.fallback {
                Some(editor) => gpui_kit::component::input::Editor::new(editor)
                    .readonly(true)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div()
                    .p_4()
                    .text_size(theme::TEXT_BODY)
                    .text_color(colors.muted)
                    .child(self.diff.title.clone())
                    .into_any_element(),
            };
        };
        let font = Theme::global(cx).mono_font_family.clone();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let paint = |gutters: f32| Paint {
            colors,
            font: font.clone(),
            metrics,
            width: theme::DIFF_GUTTER * gutters
                + theme::DIFF_INDICATOR
                + metrics.column * doc.columns as f32
                + theme::DIFF_TEXT_END,
        };
        let inline = self.diff_is_inline();
        let columns = h_flex()
            .id("diff-editor")
            .size_full()
            .min_w_0()
            .min_h_0()
            .key_context("DiffEditor")
            .track_focus(&self.diff.focus)
            .on_action(cx.listener(
                |this, _: &gpui_kit::component::input::GoToDefinition, window, cx| {
                    this.diff_go_to_definition(window, cx)
                },
            ))
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Copy, _, cx| {
                    this.copy_diff_selection(cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::SelectAll, _, cx| {
                    this.diff_select_all(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &CopyDiff, _, cx| this.copy_diff_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectAllDiff, _, cx| this.diff_select_all(cx)))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.diff.dragging = false),
            );
        let pair_count = self
            .diff_display_rows(false)
            .map_or(doc.rows.len(), |rows| rows.len());
        let inline_count = self
            .diff_display_rows(true)
            .map_or(doc.inline.len(), |rows| rows.len());
        let columns = if inline {
            columns.child(self.render_diff_list(DiffList::Inline, inline_count, paint(2.), cx))
        } else {
            columns
                .child(self.render_diff_list(DiffList::Original, pair_count, paint(1.), cx))
                .child(self.render_diff_list(DiffList::Modified, pair_count, paint(1.), cx))
        };
        columns
            .child(self.render_overview_ruler(doc, inline, cx))
            .into_any_element()
    }

    /// VS Code's overview ruler: every change block as a mark at its relative position.
    fn render_overview_ruler(
        &self,
        doc: &DiffDoc,
        inline: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let total = self
            .diff_display_rows(inline)
            .map_or_else(
                || {
                    if inline {
                        doc.inline.len()
                    } else {
                        doc.rows.len()
                    }
                },
                |rows| rows.len(),
            )
            .max(1) as f32;
        let blocks: Vec<_> = diff_ops::blocks(doc, inline)
            .into_iter()
            .filter(|block| {
                self.diff_display_rows(inline).is_none_or(|rows| {
                    let start = rows.partition_point(|row| row.source < block.start);
                    rows[start..]
                        .iter()
                        .take_while(|row| row.source < block.end)
                        .any(|row| !row.quiet && row.folded.is_none())
                })
            })
            .map(|mut block| {
                block.start = self.diff_visible_row(block.start, inline);
                block.end = self
                    .diff_visible_row(block.end, inline)
                    .max(block.start + 1);
                block
            })
            .collect();
        div()
            .relative()
            .w(theme::DIFF_RULER)
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(colors.border)
            .bg(colors.editor)
            .children(blocks.into_iter().enumerate().map(|(i, block)| {
                let color = match block.kind {
                    BlockKind::Added => colors.added,
                    BlockKind::Removed => colors.deleted,
                    BlockKind::Modified => colors.modified,
                };
                div()
                    .id(("diff-ruler", i))
                    .absolute()
                    .left_1()
                    .right_1()
                    .top(relative(block.start as f32 / total))
                    .h(relative((block.end - block.start) as f32 / total))
                    .min_h(theme::DIFF_RULER_MIN)
                    .bg(color)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.diff.change = Some(i);
                        this.diff
                            .scroll
                            .scroll_to_item_strict(block.start, ScrollStrategy::Center);
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }

    fn render_diff_list(
        &self,
        kind: DiffList,
        count: usize,
        paint: Paint,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match kind {
            DiffList::Original => "diff-original",
            DiffList::Modified => "diff-modified",
            DiffList::Inline => "diff-inline",
        };
        let blocks: Rc<Vec<Block>> = Rc::new(
            self.diff
                .doc
                .as_ref()
                .map(|doc| diff_ops::blocks(doc, kind == DiffList::Inline))
                .unwrap_or_default(),
        );
        // Block actions sit in the gutter of the list that shows the new text.
        let actions = kind != DiffList::Original && self.diff_partial_ok();
        let staged = self.diff_staged();
        let list = uniform_list(
            id,
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(doc) = this.diff.doc.clone() else {
                    return Vec::new();
                };
                let colors = paint.colors;
                range
                    .filter_map(|visible| {
                        let display = this
                            .diff_display_rows(kind == DiffList::Inline)
                            .and_then(|rows| rows.get(visible))
                            .copied()
                            .unwrap_or(DisplayRow {
                                source: visible,
                                folded: None,
                                quiet: false,
                            });
                        let index = display.source;
                        if let Some(end) = display.folded {
                            return Some(
                                h_flex()
                                    .id((id, index))
                                    .h(paint.metrics.row)
                                    .min_w_full()
                                    .px_3()
                                    .text_size(theme::TEXT_CAPTION)
                                    .text_color(colors.muted)
                                    .bg(colors.panel)
                                    .child(format!("已折叠 {} 行，点击展开", end - index))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.diff.review.collapse = false;
                                        this.diff_review_changed(window, cx);
                                    }))
                                    .into_any_element(),
                            );
                        }
                        let selected = this.diff.selection.and_then(|s| s.range(kind, index));
                        let row_kind = |kind| if display.quiet { RowKind::Same } else { kind };
                        let (row, layout) = match kind {
                            DiffList::Original => doc.rows.get(index).map(|row| {
                                pair_cell(
                                    &doc,
                                    row.old,
                                    row_kind(row.kind),
                                    false,
                                    &paint,
                                    selected,
                                )
                            }),
                            DiffList::Modified => doc.rows.get(index).map(|row| {
                                pair_cell(&doc, row.new, row_kind(row.kind), true, &paint, selected)
                            }),
                            DiffList::Inline => doc.inline.get(index).map(|row| {
                                let (side, fill, numbers) = match row.kind {
                                    LineKind::Same => (
                                        &doc.old,
                                        Fill::Plain,
                                        [
                                            Some(doc.old.lines[row.line as usize].number),
                                            row.other.map(|n| doc.new.lines[n as usize].number),
                                        ],
                                    ),
                                    LineKind::Removed => (
                                        &doc.old,
                                        Fill::Removed,
                                        [Some(doc.old.lines[row.line as usize].number), None],
                                    ),
                                    LineKind::Added => (
                                        &doc.new,
                                        Fill::Added,
                                        [None, Some(doc.new.lines[row.line as usize].number)],
                                    ),
                                };
                                line_row(
                                    &paint,
                                    if display.quiet { Fill::Plain } else { fill },
                                    &numbers,
                                    Some((side, row.line as usize)),
                                    selected,
                                )
                            }),
                        }?;
                        let group: SharedString = format!("{id}-{index}").into();
                        let block = actions
                            .then(|| {
                                blocks
                                    .binary_search_by_key(&index, |b| b.start)
                                    .ok()
                                    .map(|i| blocks[i])
                            })
                            .flatten();
                        let caret = super::diff_navigation::row_caret(&doc, kind, index);
                        let drag_layout = layout.clone();
                        Some(
                            row.id((id, index))
                                .relative()
                                .group(group.clone())
                                .cursor_text()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        let column = layout.as_ref().and_then(|layout| {
                                            let start = layout.position_for_index(0)?;
                                            (event.position.x >= start.x).then(|| {
                                                layout
                                                    .index_for_position(event.position)
                                                    .unwrap_or_else(|index| index)
                                            })
                                        });
                                        this.diff_select(
                                            kind,
                                            index,
                                            column,
                                            event.modifiers.shift,
                                            window,
                                            cx,
                                        );
                                        this.diff.caret = caret.and_then(|mut caret| {
                                            caret.column = layout
                                                .as_ref()?
                                                .index_for_position(event.position)
                                                .ok()?;
                                            Some(caret)
                                        });
                                        if event.modifiers.platform {
                                            this.diff.dragging = false;
                                            this.diff_go_to_definition(window, cx);
                                        }
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    move |this, event: &MouseMoveEvent, _, cx| {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            let column = drag_layout.as_ref().map_or(0, |layout| {
                                                layout
                                                    .index_for_position(event.position)
                                                    .unwrap_or_else(|index| index)
                                            });
                                            this.diff_drag_to(kind, index, column, cx);
                                        }
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, _| this.diff.dragging = false),
                                )
                                .when_some(block, |row, block| {
                                    row.child(block_actions(id, block, staged, group, colors, cx))
                                })
                                .map(|row| {
                                    #[cfg(test)]
                                    let row = {
                                        use gpui_kit::test::TestSupportExt;
                                        row.test_support()
                                    };
                                    row
                                })
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.diff.scroll)
        .size_full();
        let colors = theme::colors(cx);
        v_flex()
            .flex_1()
            .w_0()
            .min_w_0()
            .h_full()
            .bg(colors.editor)
            .when(kind == DiffList::Modified, |column| {
                column.border_l_1().border_color(colors.border)
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(list)
                    .child(
                        div().absolute().inset_0().child(
                            Scrollbar::vertical(&self.diff.scroll)
                                .id(match kind {
                                    DiffList::Original => "diff-original-scrollbar",
                                    DiffList::Modified => "diff-modified-scrollbar",
                                    DiffList::Inline => "diff-inline-scrollbar",
                                })
                                .viewport_from_layout(),
                        ),
                    ),
            )
            .child(
                div()
                    .relative()
                    .h(theme::DIFF_SCROLLBAR_HEIGHT)
                    .w_full()
                    .flex_shrink_0()
                    .child(
                        Scrollbar::horizontal(&self.diff.scroll)
                            .id(match kind {
                                DiffList::Original => "diff-original-horizontal-scrollbar",
                                DiffList::Modified => "diff-modified-horizontal-scrollbar",
                                DiffList::Inline => "diff-inline-horizontal-scrollbar",
                            })
                            .styles(|styles| {
                                styles.track(|track| track.width(theme::DIFF_SCROLLBAR_HEIGHT))
                            })
                            .viewport_from_layout(),
                    ),
            )
            .into_any_element()
    }
}

fn pair_cell(
    doc: &DiffDoc,
    line: Option<u32>,
    kind: RowKind,
    modified: bool,
    paint: &Paint,
    selected: Option<std::ops::Range<usize>>,
) -> (Div, Option<TextLayout>) {
    let side = if modified { &doc.new } else { &doc.old };
    match line {
        None => line_row(paint, Fill::Filler, &[None], None, selected),
        Some(index) => {
            let fill = match (kind, modified) {
                (RowKind::Same, _) => Fill::Plain,
                (RowKind::Changed, true) => Fill::Added,
                (RowKind::Changed, false) => Fill::Removed,
            };
            let number = side.lines[index as usize].number;
            line_row(
                paint,
                fill,
                &[Some(number)],
                Some((side, index as usize)),
                selected,
            )
        }
    }
}

fn line_row(
    paint: &Paint,
    fill: Fill,
    numbers: &[Option<u32>],
    line: Option<(&Side, usize)>,
    selected: Option<std::ops::Range<usize>>,
) -> (Div, Option<TextLayout>) {
    let text = line.map(|(side, index)| {
        let text: SharedString = side.line_text(index).to_owned().into();
        let selection = selected
            .as_ref()
            .map(|range| range.start.min(text.len())..range.end.min(text.len()));
        let runs = side.lines[index]
            .runs
            .iter()
            .cloned()
            .map(|(range, mut style)| {
                if matches!(fill, Fill::Plain) {
                    style.background_color = None;
                }
                (range, style)
            });
        // Split syntax runs at selection boundaries, then give selection the final background.
        let runs = gpui_kit::combine_highlights(
            runs,
            selection
                .clone()
                .map(|range| (range, HighlightStyle::default())),
        )
        .map(|(range, mut style)| {
            if selection
                .as_ref()
                .is_some_and(|s| s.start <= range.start && range.end <= s.end)
            {
                style.background_color = Some(paint.colors.selection);
            }
            (range, style)
        });
        StyledText::new(text).with_highlights(runs)
    });
    let layout = text.as_ref().map(|text| text.layout().clone());
    let colors = &paint.colors;
    let indicator = match fill {
        Fill::Removed => "−",
        Fill::Added => "+",
        Fill::Plain | Fill::Filler => "",
    };
    let row = h_flex()
        .h(paint.metrics.row)
        .w(paint.width)
        .min_w_full()
        .flex_shrink_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .font_family(paint.font.clone())
        .text_size(paint.metrics.text)
        .line_height(paint.metrics.row)
        .map(|row| match fill {
            _ if selected
                .as_ref()
                .is_some_and(|range| range.start == 0 && range.end == usize::MAX) =>
            {
                row.bg(colors.selection)
            }
            Fill::Plain => row,
            Fill::Removed => row.bg(colors.diff_deleted),
            Fill::Added => row.bg(colors.diff_added),
            Fill::Filler => row.bg(pattern_slash(
                colors.diff_filler,
                theme::DIFF_HATCH.0,
                theme::DIFF_HATCH.1,
            )),
        })
        .children(numbers.iter().map(|number| {
            div()
                .w(theme::DIFF_GUTTER)
                .flex_shrink_0()
                .text_right()
                .text_color(colors.muted)
                .children(number.map(|n| n.to_string()))
        }))
        .child(
            div()
                .w(theme::DIFF_INDICATOR)
                .flex_shrink_0()
                .text_center()
                .text_color(colors.muted)
                .child(indicator),
        )
        .when_some(text, |row, text| {
            row.child(div().flex_shrink_0().text_color(colors.code).child(text))
        });
    (row, layout)
}

/// "暂存块 / 还原块" (or "取消暂存块") in the gutter of a block's first row, on hover.
fn block_actions(
    list: &'static str,
    block: Block,
    staged: bool,
    group: SharedString,
    colors: theme::Colors,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let button = |action: HunkAction, icon: IconName, label: &'static str| {
        Button::new((SharedString::from(format!("{list}-{label}")), block.start))
            .xsmall()
            .ghost()
            .icon(icon)
            .tooltip(label)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.diff_apply_rows(action, block.start..block.end, window, cx);
            }))
    };
    h_flex()
        .absolute()
        .left_0()
        .top_0()
        .h_full()
        .w(theme::DIFF_GUTTER)
        .justify_end()
        .bg(colors.panel)
        .opacity(0.)
        .group_hover(group, |actions| actions.opacity(1.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .map(|actions| {
            if staged {
                actions.child(button(HunkAction::Unstage, IconName::Minus, "取消暂存块"))
            } else {
                actions
                    .child(button(HunkAction::Revert, IconName::Undo2, "还原块"))
                    .child(button(HunkAction::Stage, IconName::Plus, "暂存块"))
            }
        })
        .into_any_element()
}

impl DiffPane {
    pub(super) fn visible_top_row(&self) -> usize {
        let scroll = self.scroll.0.borrow();
        if let Some(deferred) = scroll.deferred_scroll_to_item {
            return deferred.item_index;
        }
        let Some(doc) = &self.doc else {
            return 0;
        };
        let inline = self.inline || doc.old.lines.is_empty() || doc.new.lines.is_empty();
        let current = self
            .review
            .document
            .as_ref()
            .is_some_and(|other| Arc::ptr_eq(doc, other));
        let count = if current {
            if inline {
                self.review.inline.len()
            } else {
                self.review.pair.len()
            }
        } else if inline {
            doc.inline.len()
        } else {
            doc.rows.len()
        };
        if count == 0 {
            return 0;
        }
        let Some(size) = scroll.last_item_size else {
            return 0;
        };
        // GPUI's item is the viewport; contents spans all uniform rows.
        let row_height = f32::from(size.contents.height) / count as f32;
        if row_height <= 0. || !row_height.is_finite() {
            return 0;
        }
        ((f32::from(-scroll.base_handle.offset().y) / row_height)
            .max(0.)
            .floor() as usize)
            .min(count - 1)
    }

    pub(super) fn source_row(&self, visible: usize) -> usize {
        let current = self
            .doc
            .as_ref()
            .zip(self.review.document.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b));
        if !current {
            return visible;
        }
        let inline = self
            .doc
            .as_ref()
            .is_none_or(|doc| self.inline || doc.old.lines.is_empty() || doc.new.lines.is_empty());
        let rows = if inline {
            &self.review.inline
        } else {
            &self.review.pair
        };
        rows.get(visible).map_or(visible, |row| row.source)
    }

    pub(super) fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        Self {
            tab: None,
            review: DiffReview::new(window, cx),
            title: String::new(),
            fallback: None,
            doc: None,
            raw: None,
            selection: None,
            caret: None,
            dragging: false,
            focus: cx.focus_handle(),
            source: None,
            change: None,
            inline: cx.global::<crate::settings::Settings>().diff_inline,
            scroll: UniformListScrollHandle::new(),
            pending_scroll: None,
            stale: false,
            loading: false,
            task: None,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
        }
    }
}

#[cfg(test)]
#[path = "diff_review_ui_tests.rs"]
mod reading_tests;

/// The diff tab: what it shows, the parsed document, selection and scrolling, and its loading task.
pub(super) struct DiffPane {
    pub(super) tab: Option<DiffTab>,
    pub(super) review: DiffReview,
    pub(super) title: String,
    /// Git's raw text in a read-only editor, shown only when the patch cannot be parsed.
    pub(super) fallback: Option<Entity<EditorState>>,
    /// The parsed diff editor document.
    pub(super) doc: Option<Arc<crate::diff_doc::DiffDoc>>,
    /// Git's own patch lines, for copying exact text and staging selected lines.
    pub(super) raw: Option<Arc<crate::partial_patch::RawPatch>>,
    pub(super) selection: Option<diff_ops::DiffSelection>,
    pub(super) caret: Option<super::diff_navigation::Caret>,
    pub(super) dragging: bool,
    pub(super) focus: FocusHandle,
    /// Patch text and dark mode the document was built from; equal reloads keep the view.
    pub(super) source: Option<(Arc<str>, bool)>,
    pub(super) change: Option<usize>,
    pub(super) inline: bool,
    pub(super) scroll: UniformListScrollHandle,
    /// A scroll target, applied once the display rows it refers to are ready.
    pub(super) pending_scroll: Option<PendingScroll>,
    pub(super) stale: bool,
    /// The patch is being (re)loaded: what is shown may be older than the status.
    pub(super) loading: bool,
    pub(super) task: Option<Task<()>>,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) generation: u64,
}
