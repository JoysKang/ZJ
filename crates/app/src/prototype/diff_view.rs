//! VS Code-style diff editor over a [`DiffDoc`]: two virtualized lists side by side that share
//! one scroll handle (so vertical and horizontal offsets stay in step without timers), or a
//! single inline list. Rows only slice precomputed text and style runs; nothing is parsed or
//! highlighted during render.
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiffList {
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
    width: Pixels,
}

impl Prototype {
    /// Inline when chosen, and always for added or deleted files (one side is empty).
    fn diff_is_inline(&self) -> bool {
        self.diff_doc.as_ref().is_none_or(|doc| {
            self.diff_inline || doc.old.lines.is_empty() || doc.new.lines.is_empty()
        })
    }

    fn diff_change_starts(&self) -> &[usize] {
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

    fn step_change(&mut self, forward: bool, cx: &mut Context<Self>) {
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

    fn toggle_diff_layout(&mut self, cx: &mut Context<Self>) {
        self.diff_inline = !self.diff_inline;
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
                .on_click(cx.listener(|this, _, _, cx| this.toggle_diff_layout(cx))),
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
        let paint = |gutters: f32| Paint {
            colors,
            font: font.clone(),
            width: theme::DIFF_GUTTER * gutters
                + theme::DIFF_INDICATOR
                + theme::DIFF_COLUMN_WIDTH * doc.columns as f32
                + theme::DIFF_TEXT_END,
        };
        let columns = h_flex().size_full().min_w_0().min_h_0();
        if self.diff_is_inline() {
            return columns
                .child(self.render_diff_list(DiffList::Inline, doc.inline.len(), paint(2.), cx))
                .into_any_element();
        }
        columns
            .child(self.render_diff_list(DiffList::Original, doc.rows.len(), paint(1.), cx))
            .child(self.render_diff_list(DiffList::Modified, doc.rows.len(), paint(1.), cx))
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
        let list = uniform_list(
            id,
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                let Some(doc) = &this.diff_doc else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| match kind {
                        DiffList::Original => doc
                            .rows
                            .get(index)
                            .map(|row| pair_cell(doc, row.old, row.kind, false, &paint)),
                        DiffList::Modified => doc
                            .rows
                            .get(index)
                            .map(|row| pair_cell(doc, row.new, row.kind, true, &paint)),
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
                            line_row(&paint, fill, &numbers, Some((side, row.line as usize)))
                        }),
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
) -> AnyElement {
    let side = if modified { &doc.new } else { &doc.old };
    match line {
        None => line_row(paint, Fill::Filler, &[None], None),
        Some(index) => {
            let fill = match (kind, modified) {
                (RowKind::Same, _) => Fill::Plain,
                (RowKind::Changed, true) => Fill::Added,
                (RowKind::Changed, false) => Fill::Removed,
            };
            let number = side.lines[index as usize].number;
            line_row(paint, fill, &[Some(number)], Some((side, index as usize)))
        }
    }
}

fn line_row(
    paint: &Paint,
    fill: Fill,
    numbers: &[Option<u32>],
    line: Option<(&Side, usize)>,
) -> AnyElement {
    let colors = &paint.colors;
    let indicator = match fill {
        Fill::Removed => "−",
        Fill::Added => "+",
        Fill::Plain | Fill::Filler => "",
    };
    h_flex()
        .h(theme::DIFF_ROW_HEIGHT)
        .w(paint.width)
        .min_w_full()
        .flex_shrink_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .font_family(paint.font.clone())
        .text_size(theme::DIFF_TEXT)
        .line_height(theme::DIFF_ROW_HEIGHT)
        .map(|row| match fill {
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
        .into_any_element()
}
