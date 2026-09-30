//! Source Control view: every repository of the workspace in one tree, VS Code style.

use super::SINGLE_LINE;
use super::{Prototype, Row, sidebar::chevron};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::sync::atomic::Ordering;
use workspace_editor_git::DiffSide;

fn count_badge(count: usize, colors: theme::Colors) -> impl IntoElement {
    div()
        .h(theme::BADGE_SIZE + theme::ROW_INSET)
        .min_w(theme::BADGE_SIZE + theme::ROW_INSET)
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(colors.keycap)
        .text_color(colors.foreground)
        .text_size(theme::TEXT_SECTION)
        .child(count.to_string())
}

impl Prototype {
    pub(super) fn render_scm(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let status_line = if self.loading {
            Some(format!("正在刷新 {} 个仓库…", self.groups.len()))
        } else if !self.issues.is_empty() {
            Some(format!("{} 项仓库错误", self.issues.len()))
        } else if self.groups.is_empty() && self.root.is_some() {
            Some("未发现 Git 仓库".into())
        } else {
            None
        };
        v_flex()
            .size_full()
            .min_h_0()
            .when_some(status_line, |view, line| {
                view.child(
                    h_flex()
                        .h(theme::ROW_HEIGHT)
                        .flex_shrink_0()
                        .pl(theme::TREE_BASE)
                        .pr_2()
                        .justify_between()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(line)
                        .when(self.loading, |row| {
                            row.child(
                                Button::new("cancel-git")
                                    .xsmall()
                                    .ghost()
                                    .label("取消")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel.store(true, Ordering::Relaxed);
                                        this.generation += 1;
                                        this.refresh_task = None;
                                        this.loading = false;
                                        this.close_preview(cx);
                                        this.message =
                                            "已取消；当前结果可能不完整，请重新扫描".into();
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .child(
                uniform_list(
                    "changes",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| this.scm_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }

    fn scm_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let base = h_flex()
            .id(("scm-row", index))
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pr_2()
            .gap_1()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .hover(|row| row.bg(colors.hover));
        let row = match self.rows[index] {
            Row::Group(g) => {
                let group = &self.groups[g];
                let path = self
                    .root
                    .as_ref()
                    .and_then(|root| group.repo.worktree.strip_prefix(root).ok())
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(&group.repo.worktree);
                let name = path.to_string_lossy().replace(SINGLE_LINE, "⏎");
                let (detail, detail_color, count) = match &group.status {
                    None => ("待确认".to_string(), colors.muted, None),
                    Some(Err(error)) => (format!("错误：{error}"), colors.deleted, None),
                    Some(Ok(status)) => (
                        format!(
                            "{}{}",
                            status.branch.as_deref().unwrap_or("未知分支"),
                            if group.stale { " · 陈旧" } else { "" }
                        ),
                        colors.muted,
                        Some(status.changes.len()),
                    ),
                };
                base.pl(theme::ROW_INSET)
                    .cursor_pointer()
                    .role(Role::TreeItem)
                    .aria_label(format!("仓库 {name} · {detail}"))
                    .child(chevron(group.expanded, colors.muted))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        Icon::new(IconName::GitBranch)
                            .size(theme::SMALL_ICON_SIZE)
                            .text_color(colors.muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(detail_color)
                            .child(detail),
                    )
                    .when_some(count.filter(|count| *count > 0), |row, count| {
                        row.child(count_badge(count, colors))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.groups[g].expanded = !this.groups[g].expanded;
                        this.rebuild_rows();
                        cx.notify();
                    }))
            }
            Row::Heading(side, count) => base
                .pl(theme::TREE_BASE + theme::TREE_STEP)
                .text_size(theme::TEXT_SECTION)
                .font_weight(FontWeight::BOLD)
                .text_color(colors.muted)
                .child(div().flex_1().child(match side {
                    DiffSide::Staged => "暂存的更改",
                    DiffSide::Worktree => "更改",
                }))
                .child(count_badge(count, colors)),
            Row::File(g, i, side) => {
                let Some(Ok(status)) = &self.groups[g].status else {
                    return base.into_any_element();
                };
                let change = &status.changes[i];
                let mut decoration = super::decoration(change);
                if matches!(side, DiffSide::Staged) && change.index != b'.' {
                    // The staged row reflects the index side (e.g. A or R), not the worktree.
                    let staged = workspace_editor_git::Change {
                        worktree: b'.',
                        ..change.clone()
                    };
                    decoration = super::decoration(&staged);
                }
                let color = colors.decoration(decoration.kind);
                let name = change
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎");
                let directory = change
                    .path
                    .parent()
                    .map(|parent| parent.to_string_lossy().replace(SINGLE_LINE, "⏎"))
                    .unwrap_or_default();
                base.pl(theme::TREE_BASE + theme::TREE_STEP + theme::TWISTY_WIDTH)
                    .gap_2()
                    .cursor_pointer()
                    .role(Role::Button)
                    .aria_label(format!(
                        "{:?} · {} · {}",
                        side,
                        self.groups[g].repo.worktree.display(),
                        change.path.display()
                    ))
                    .child(file_icons::icon(file_icons::for_file(&name)))
                    .child(div().flex_shrink_0().text_color(color).child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(colors.muted)
                            .child(directory),
                    )
                    .child(
                        div()
                            .w(theme::DECORATION_WIDTH)
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(color)
                            .child(decoration.letter.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_diff(g, i, side, window, cx)
                    }))
            }
        };
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}
