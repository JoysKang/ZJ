//! VS Code-style diff editor over a [`DiffDoc`]: two virtualized lists side by side that share
//! one scroll handle (so vertical and horizontal offsets stay in step without timers), or a
//! single inline list. Rows only slice precomputed text and style runs; nothing is parsed or
//! highlighted during render.
use super::diff_ops::{self, Block, BlockKind, CopyDiff, HunkAction, SelectAllDiff};
use super::*;
use crate::diff_doc::{DiffDoc, LineKind, RowKind, Side};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Sizable, Theme,
        button::{Button, ButtonVariants},
        h_flex,
        scroll::Scrollbar,
    },
};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiffList {
    Original,
    Modified,
    Inline,
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

impl Prototype {
    /// Inline when chosen, and always for added or deleted files (one side is empty).
    pub(super) fn diff_is_inline(&self) -> bool {
        self.diff_doc.as_ref().is_none_or(|doc| {
            self.diff_inline || doc.old.lines.is_empty() || doc.new.lines.is_empty()
        })
    }

    pub(super) fn diff_change_starts(&self) -> &[usize] {
        match &self.diff_doc {
            Some(doc) if self.diff_is_inline() => &doc.inline_changes,
            Some(doc) => &doc.changes,
            None => &[],
        }
    }

    /// Reveals the first change of a freshly opened diff.
    pub(super) fn reveal_first_change(&mut self) {
        self.diff_change = None;
        if let Some(&row) = self.diff_change_starts().first() {
            self.diff_change = Some(0);
            self.diff_scroll
                .scroll_to_item_strict(row, ScrollStrategy::Center);
        }
    }

    pub(super) fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
        let starts = self.diff_change_starts();
        let count = starts.len();
        if count == 0 {
            return;
        }
        let next = match self.diff_change {
            None if forward => 0,
            None => count - 1,
            Some(index) if forward => (index + 1) % count,
            Some(index) => (index + count - 1) % count,
        };
        let row = starts[next];
        self.diff_change = Some(next);
        self.diff_scroll
            .scroll_to_item_strict(row, ScrollStrategy::Center);
        cx.notify();
    }

    pub(super) fn toggle_diff_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_inline = !self.diff_inline;
        let inline = self.diff_inline;
        self.change_settings(window, cx, |settings| settings.diff_inline = inline);
        // Row indexes differ between the layouts; keep the current change in view.
        self.diff_scroll = UniformListScrollHandle::new();
        if let Some(index) = self.diff_change
            && let Some(&row) = self.diff_change_starts().get(index)
        {
            self.diff_scroll
                .scroll_to_item_strict(row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Editor actions at the right end of the tab strip, as VS Code shows for a diff.
    pub(super) fn render_diff_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let changes = !self.diff_change_starts().is_empty();
        let one_sided = self
            .diff_doc
            .as_ref()
            .is_none_or(|doc| doc.old.lines.is_empty() || doc.new.lines.is_empty());
        let inline = self.diff_is_inline();
        let action = |id: &'static str, icon: IconName, label: &'static str| {
            Button::new(id)
                .ghost()
                .small()
                .icon(icon)
                .tooltip(label)
                .accessibility_label(label)
        };
        h_flex()
            .h_full()
            .flex_shrink_0()
            .px_2()
            .gap_1()
            .bg(colors.tabs)
            .when_some(self.diff_doc.as_ref(), |bar, doc| {
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
                action("diff-open-file", IconName::File, "打开文件").on_click(cx.listener(
                    |this, _, window, cx| {
                        if let Some(path) = this.preview_diff.as_ref().map(|d| d.path.clone()) {
                            this.scm_open_file(&path, window, cx);
                        }
                    },
                )),
            )
            .into_any_element()
    }

    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(doc) = &self.diff_doc else {
            // Binary, combined or failed diffs: Git's own text, or the status message.
            return match &self.preview {
                Some(editor) => gpui_kit::component::input::Editor::new(editor)
                    .readonly(true)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div()
                    .p_4()
                    .text_size(theme::TEXT_BODY)
                    .text_color(colors.muted)
                    .child(self.preview_title.clone())
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
            .track_focus(&self.diff_focus)
            .on_action(cx.listener(|this, _: &CopyDiff, _, cx| this.copy_diff_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectAllDiff, _, cx| this.diff_select_all(cx)))
            .on_action(cx.listener(
                |this, _: &super::agent_review::AcceptAgentChange, window, cx| {
                    this.agent_review_current(true, window, cx)
                },
            ))
            .on_action(cx.listener(
                |this, _: &super::agent_review::RejectAgentChange, window, cx| {
                    this.agent_review_current(false, window, cx)
                },
            ))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.diff_dragging = false),
            );
        let columns = if inline {
            columns.child(self.render_diff_list(DiffList::Inline, doc.inline.len(), paint(2.), cx))
        } else {
            columns
                .child(self.render_diff_list(DiffList::Original, doc.rows.len(), paint(1.), cx))
                .child(self.render_diff_list(DiffList::Modified, doc.rows.len(), paint(1.), cx))
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
        let total = if inline {
            doc.inline.len()
        } else {
            doc.rows.len()
        }
        .max(1) as f32;
        let blocks = diff_ops::blocks(doc, inline);
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
                        this.diff_change = Some(i);
                        this.diff_scroll
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
            self.diff_doc
                .as_ref()
                .map(|doc| diff_ops::blocks(doc, kind == DiffList::Inline))
                .unwrap_or_default(),
        );
        // Block actions sit in the gutter of the list that shows the new text.
        let actions = kind != DiffList::Original && self.diff_partial_ok();
        // Agent reviews: 接受 / 拒绝 instead of staging (the right list in side-by-side).
        let agent_actions = kind != DiffList::Original && self.diff_is_agent_review();
        let staged = self.diff_staged();
        let list = uniform_list(
            id,
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let Some(doc) = this.diff_doc.clone() else {
                    return Vec::new();
                };
                let colors = paint.colors;
                // Rows are laid out after the list settles its scroll offset and bounds, so
                // these are this frame's values.
                let viewport = agent_actions.then(|| {
                    let handle = &this.diff_scroll.0.borrow().base_handle;
                    (handle.offset().x, handle.bounds().size.width)
                });
                range
                    .filter_map(|index| {
                        let selected = this.diff_selection.is_some_and(|s| s.contains(kind, index));
                        let row = match kind {
                            DiffList::Original => doc.rows.get(index).map(|row| {
                                pair_cell(&doc, row.old, row.kind, false, &paint, selected)
                            }),
                            DiffList::Modified => doc.rows.get(index).map(|row| {
                                pair_cell(&doc, row.new, row.kind, true, &paint, selected)
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
                                    fill,
                                    &numbers,
                                    Some((side, row.line as usize)),
                                    selected,
                                )
                            }),
                        }?;
                        let group: SharedString = format!("{id}-{index}").into();
                        let block = (actions || agent_actions)
                            .then(|| {
                                blocks
                                    .binary_search_by_key(&index, |b| b.start)
                                    .ok()
                                    .map(|i| (i, blocks[i]))
                            })
                            .flatten();
                        let current_change = this.diff_change;
                        Some(
                            row.id((id, index))
                                .relative()
                                .group(group.clone())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                        this.diff_select(
                                            kind,
                                            index,
                                            event.modifiers.shift,
                                            window,
                                            cx,
                                        )
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    move |this, event: &MouseMoveEvent, _, cx| {
                                        if event.pressed_button == Some(MouseButton::Left) {
                                            this.diff_drag_to(kind, index, cx);
                                        }
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, _| this.diff_dragging = false),
                                )
                                .when_some(block, |row, (i, block)| {
                                    if agent_actions {
                                        row.child(super::agent_review::agent_block_actions(
                                            id,
                                            i,
                                            block.start,
                                            current_change == Some(i),
                                            viewport.unwrap_or_default(),
                                            colors,
                                            cx,
                                        ))
                                    } else {
                                        row.child(block_actions(
                                            id, block, staged, group, colors, cx,
                                        ))
                                    }
                                })
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.diff_scroll)
        .size_full();
        let colors = theme::colors(cx);
        div()
            .relative()
            .flex_1()
            .w_0()
            .min_w_0()
            .h_full()
            .bg(colors.editor)
            .when(kind == DiffList::Modified, |column| {
                column.border_l_1().border_color(colors.border)
            })
            .child(list)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&self.diff_scroll)
                        .id(match kind {
                            DiffList::Original => "diff-original-scrollbar",
                            DiffList::Modified => "diff-modified-scrollbar",
                            DiffList::Inline => "diff-inline-scrollbar",
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
    selected: bool,
) -> Div {
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
    selected: bool,
) -> Div {
    let colors = &paint.colors;
    let indicator = match fill {
        Fill::Removed => "−",
        Fill::Added => "+",
        Fill::Plain | Fill::Filler => "",
    };
    h_flex()
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
            _ if selected => row.bg(colors.selection),
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
        .when_some(line, |row, (side, index)| {
            let line = &side.lines[index];
            let text: SharedString = side.line_text(index).to_owned().into();
            row.child(
                div()
                    .flex_shrink_0()
                    .text_color(colors.code)
                    .child(StyledText::new(text).with_highlights(line.runs.iter().cloned())),
            )
        })
}

/// "暂存块 / 还原块" (or "取消暂存块") in the gutter of a block's first row, on hover.
fn block_actions(
    list: &'static str,
    block: Block,
    staged: bool,
    group: SharedString,
    colors: theme::Colors,
    cx: &mut Context<Prototype>,
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
