//! VS Code-style paired, virtualized patch view with a shared vertical scroll position.
use super::*;
use crate::diff_model::{Cell, Row};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        scroll::{Scrollbar, ScrollbarMode},
        v_flex,
    },
};

impl Prototype {
    pub(super) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let inline = self.diff_inline || self.diff_model.is_none();
        let staged = self.preview_diff.as_ref().is_some_and(|tab| {
            matches!(
                tab.request.operation,
                Operation::Diff {
                    side: DiffSide::Staged,
                    ..
                }
            )
        });
        let header = h_flex()
            .h(theme::TAB_HEIGHT)
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION)
            .child(div().flex_1().child(if staged {
                "HEAD · 原始版本"
            } else {
                "暂存区 · 原始版本"
            }))
            .child(div().flex_1().child(if staged {
                "暂存区 · 已暂存"
            } else {
                "工作区 · 磁盘版本"
            }))
            .child(
                Button::new("diff-mode")
                    .xsmall()
                    .ghost()
                    .label(if inline { "左右对照" } else { "内联" })
                    .disabled(self.diff_model.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.diff_inline = !this.diff_inline;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("diff-open-file")
                    .xsmall()
                    .ghost()
                    .icon(IconName::File)
                    .tooltip("打开文件")
                    .accessibility_label("打开文件")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(path) = this.preview_diff.as_ref().map(|d| d.path.clone()) {
                            this.scm_open_file(&path, window, cx);
                        }
                    })),
            );
        let body = if inline {
            match &self.preview {
                Some(editor) => gpui_kit::component::input::Editor::new(editor)
                    .readonly(true)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div()
                    .p_4()
                    .text_size(theme::TEXT_BODY)
                    .child(self.preview_title.clone())
                    .into_any_element(),
            }
        } else {
            h_flex()
                .size_full()
                .min_w_0()
                .min_h_0()
                .child(self.render_diff_column(false, cx))
                .child(self.render_diff_column(true, cx))
                .into_any_element()
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }
    fn render_diff_column(&self, new_side: bool, cx: &mut Context<Self>) -> AnyElement {
        let count = self.diff_model.as_ref().map_or(0, |model| model.rows.len());
        let list = uniform_list(
            if new_side { "diff-modified" } else { "diff-original" },
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let colors = theme::colors(cx);
                let Some(model) = &this.diff_model else {
                    return Vec::new();
                };
                range.filter_map(|i| model.rows.get(i)).map(|row| {
                    let content = match row {
                        Row::Hunk(range) => div()
                            .px_3()
                            .bg(colors.panel)
                            .text_color(colors.muted)
                            .text_size(theme::TEXT_CAPTION)
                            .whitespace_nowrap()
                            .child(format!("{} · 未更改区域已折叠", &model.patch[range.clone()]))
                            .into_any_element(),
                        Row::Lines { old, new } => diff_cell(
                            if new_side { new.as_ref() } else { old.as_ref() },
                            model,
                            new_side,
                            colors,
                        ),
                    };
                    h_flex()
                        .h(theme::ROW_HEIGHT)
                        .w(theme::DIFF_COLUMN_WIDTH * model.columns as f32 + theme::DIFF_GUTTER)
                        .min_w_full()
                        .child(content)
                        .into_any_element()
                }).collect::<Vec<_>>()
            }),
        )
            .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
            // Equal row heights, widths and viewport sizes allow Kit's native scroll handle
            // to keep both vertical and horizontal offsets synchronized without timers.
            .track_scroll(&self.diff_scroll).size_full();
        div()
            .relative()
            .flex_1()
            .w_0()
            .min_w_0()
            .h_full()
            .child(list)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&self.diff_scroll)
                        .mode(ScrollbarMode::Always)
                        .id(if new_side {
                            "diff-modified-scrollbar"
                        } else {
                            "diff-original-scrollbar"
                        })
                        .viewport_from_layout(),
                ),
            )
            .into_any_element()
    }
}

fn diff_cell(
    cell: Option<&Cell>,
    model: &crate::diff_model::DiffModel,
    new: bool,
    colors: theme::Colors,
) -> AnyElement {
    let bg = match cell {
        Some(cell) if cell.changed => {
            if new {
                colors.diff_added
            } else {
                colors.diff_deleted
            }
        }
        None => colors.panel,
        _ => colors.editor,
    };
    h_flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(bg)
        .border_r_1()
        .border_color(colors.border)
        .text_size(theme::DIFF_TEXT)
        .font_family("Menlo")
        .overflow_hidden()
        .child(
            div()
                .w(theme::DIFF_GUTTER)
                .flex_shrink_0()
                .pr_3()
                .text_right()
                .text_color(colors.muted)
                .child(cell.map(|c| c.number.to_string()).unwrap_or_default()),
        )
        .child(
            div().flex_1().min_w_0().whitespace_nowrap().child(
                cell.map(|c| model.patch[c.text.clone()].replace('\t', "    "))
                    .unwrap_or_default(),
            ),
        )
        .into_any_element()
}
