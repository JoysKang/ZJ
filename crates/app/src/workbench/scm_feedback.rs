//! Repository-scoped feedback: a quiet result row with selectable diagnostics on demand.
use super::{Group, Workbench};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    base::{SelectableText, TestSupportExt},
    component::Selectable,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::FluentBuilder,
    *,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FeedbackKind {
    Info,
    Success,
    Error,
}

pub(super) struct GitFeedback {
    summary: String,
    kind: FeedbackKind,
    expanded: bool,
}

impl Group {
    pub(super) fn set_git_feedback(
        &mut self,
        summary: impl Into<String>,
        detail: impl Into<String>,
        kind: FeedbackKind,
    ) {
        self.write_message = workspace_editor_git::redact_git_output(&detail.into());
        self.write_feedback = Some(GitFeedback {
            summary: workspace_editor_git::redact_git_output(&summary.into()),
            kind,
            expanded: kind == FeedbackKind::Error,
        });
    }

    pub(super) fn clear_git_feedback(&mut self) {
        self.write_message.clear();
        self.write_feedback = None;
    }
}

impl Workbench {
    pub(super) fn render_git_feedback(&self, g: usize, cx: &mut Context<Self>) -> AnyElement {
        let group = &self.groups[g];
        let colors = theme::colors(cx);
        let feedback = group.write_feedback.as_ref();
        let summary = feedback
            .map(|f| f.summary.as_str())
            .unwrap_or_else(|| group.write_message.lines().next().unwrap_or_default());
        let failed = feedback.is_some_and(|f| f.kind == FeedbackKind::Error);
        let expanded = feedback.is_some_and(|f| f.expanded);
        let has_details = summary != group.write_message.trim();
        let repo = group.repo.id.clone();
        let toggle_repo = repo.clone();
        let refresh_repo = repo.clone();
        let copy = group.write_message.clone();
        v_flex()
            .id(("git-operation-result", g))
            .w_full()
            .min_w_0()
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted)
            .test_support()
            .child(
                h_flex()
                    .w_full()
                    .h(theme::ROW_HEIGHT)
                    .gap_1()
                    .child(
                        Icon::new(if failed {
                            IconName::TriangleAlert
                        } else if feedback.is_some_and(|f| f.kind == FeedbackKind::Success) {
                            IconName::Check
                        } else {
                            IconName::Info
                        })
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(if failed {
                            colors.deleted
                        } else {
                            colors.muted
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if failed { colors.deleted } else { colors.muted })
                            .child(summary.to_string()),
                    )
                    .when(has_details, |row| {
                        row.child(
                            Button::new(("git-result-toggle", g))
                                .xsmall()
                                .ghost()
                                .label("详情")
                                .selected(expanded)
                                .accessibility_label(if expanded {
                                    "收起 Git 操作详情"
                                } else {
                                    "展开 Git 操作详情"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(group) = this
                                        .groups
                                        .iter_mut()
                                        .find(|group| group.repo.id == toggle_repo)
                                        && let Some(feedback) = &mut group.write_feedback
                                    {
                                        feedback.expanded = !feedback.expanded;
                                        this.rebuild_rows();
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .when(has_details, |row| {
                        row.child(
                            Button::new(("git-copy-result", g))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Copy)
                                .accessibility_label("复制 Git 操作结果")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                }),
                        )
                    })
                    .child(
                        Button::new(("git-result-dismiss", g))
                            .xsmall()
                            .ghost()
                            .icon(IconName::X)
                            .accessibility_label("关闭 Git 操作结果")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(group) =
                                    this.groups.iter_mut().find(|group| group.repo.id == repo)
                                {
                                    group.clear_git_feedback();
                                    this.sidebar_focus.focus(window, cx);
                                    this.rebuild_rows();
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .when(expanded && has_details, |view| {
                view.child(
                    div()
                        .id(("git-result-detail", g))
                        .w_full()
                        .min_w_0()
                        .max_h(theme::SCM_NOTICE_MAX)
                        .overflow_y_scroll()
                        .p_2()
                        .rounded(theme::RADIUS)
                        .bg(colors.editor)
                        .text_color(colors.foreground)
                        .line_height(theme::SCM_DETAIL_LINE)
                        .cursor_text()
                        .test_support()
                        .child(SelectableText::new(
                            ("git-result-text", g),
                            group.write_message.clone(),
                        )),
                )
            })
            .when(failed, |view| {
                view.child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .gap_1()
                        .flex_wrap()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child("操作可能部分完成，刷新后检查"),
                        )
                        .child(
                            Button::new(("git-result-refresh", g))
                                .xsmall()
                                .ghost()
                                .label("刷新状态")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if this
                                        .groups
                                        .iter()
                                        .any(|group| group.repo.id == refresh_repo)
                                    {
                                        this.refresh(window, cx);
                                    }
                                })),
                        ),
                )
            })
            .into_any_element()
    }
}
