//! Source Control view: every repository of the workspace in one tree, VS Code style.

use super::SINGLE_LINE;
use super::{Prototype, Row, sidebar::chevron};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Textarea,
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
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
    pub(super) fn scm_more(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let selected = self
            .scm_repo
            .as_ref()
            .and_then(|id| self.groups.iter().position(|g| &g.repo.id == id))
            .or_else(|| (!self.groups.is_empty()).then_some(0));
        let stage = selected.and_then(|g| self.scm_paths(g, None, DiffSide::Worktree));
        let unstage = selected.and_then(|g| self.scm_paths(g, None, DiffSide::Staged));
        Button::new("scm-more-button")
            .xsmall()
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip("源码管理操作")
            .accessibility_label("源码管理操作")
            .dropdown_menu(move |menu, _, _| {
                let view = weak.clone();
                menu.item(PopupMenuItem::new("刷新").on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.refresh(window, cx));
                }))
                .when_some(stage.clone(), |menu, request| {
                    menu.item(git_menu_item("全部暂存", request, weak.clone()))
                })
                .when_some(unstage.clone(), |menu, request| {
                    menu.item(git_menu_item("全部取消暂存", request, weak.clone()))
                })
            })
            .into_any_element()
    }

    fn scm_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let selected = self
            .scm_repo
            .as_ref()
            .and_then(|id| self.groups.iter().find(|g| &g.repo.id == id))
            .or_else(|| self.groups.first());
        let Some(group) = selected else {
            return div().into_any_element();
        };
        let id = group.repo.id.clone();
        let push_id = id.clone();
        let status = group.status.as_ref().and_then(|s| s.as_ref().ok());
        let can_commit = status.is_some_and(|s| {
            s.changes.iter().any(|c| c.staged())
                && !s
                    .changes
                    .iter()
                    .any(|c| c.kind == workspace_editor_git::ChangeKind::Conflict)
        });
        let message = group.commit_input.read(cx).value();
        let target = group
            .repo
            .worktree
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let branch = status
            .and_then(|s| s.branch.as_deref())
            .unwrap_or("未知分支");
        let tracking = status
            .map(|s| {
                format!(
                    "{} · ↑{} ↓{}",
                    s.upstream.as_deref().unwrap_or("未配置上游"),
                    s.ahead,
                    s.behind
                )
            })
            .unwrap_or_default();
        v_flex()
            .flex_shrink_0()
            .p_2()
            .gap_2()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.foreground)
                    .child(format!("{target} · {branch}")),
            )
            .child(
                Textarea::new(&group.commit_input)
                    .h(theme::COMMIT_HEIGHT)
                    .disabled(group.write_pending)
                    .aria_label(format!("{target} 的提交信息")),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("scm-commit")
                            .small()
                            .primary()
                            .icon(IconName::Check)
                            .label("提交")
                            .tooltip("仅提交当前仓库暂存区")
                            .disabled(
                                group.write_pending || !can_commit || message.trim().is_empty(),
                            )
                            .flex_1()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scm_commit(id.clone(), false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("scm-push")
                            .small()
                            .ghost()
                            .icon(IconName::ArrowUp)
                            .label("推送")
                            .tooltip(tracking.clone())
                            .disabled(group.write_pending || status.is_none())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scm_commit(push_id.clone(), true, window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(tracking),
            )
            .when(!group.write_message.is_empty(), |view| {
                view.child(
                    div()
                        .id("git-operation-result")
                        .max_h(theme::SCM_NOTICE_MAX)
                        .overflow_y_scroll()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.foreground)
                        .child(group.write_message.clone()),
                )
            })
            .into_any_element()
    }

    pub(super) fn render_scm(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let status_line = if !self.issues.is_empty() {
            Some(format!("{} 项仓库错误", self.issues.len()))
        } else if self.refresh_completed && self.groups.is_empty() && self.root.is_some() {
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
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(line),
                )
            })
            .child(self.scm_controls(cx))
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
                    .unwrap_or_else(|| {
                        std::path::Path::new(group.repo.worktree.file_name().unwrap_or_default())
                    });
                let name = path.to_string_lossy().replace(SINGLE_LINE, "⏎");
                let (detail, detail_color, count) = match &group.status {
                    None => (String::new(), colors.muted, None),
                    Some(Err(error)) => (format!("错误：{error}"), colors.deleted, None),
                    Some(Ok(status)) => (
                        status.branch.as_deref().unwrap_or("未知分支").to_string(),
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
                        if this.scm_repo.as_ref() == Some(&this.groups[g].repo.id) {
                            this.groups[g].expanded = !this.groups[g].expanded;
                        }
                        this.scm_repo = Some(this.groups[g].repo.id.clone());
                        this.rebuild_rows();
                        cx.notify();
                    }))
            }
            Row::Heading(g, side, count) => base
                .pl(theme::TREE_BASE + theme::TREE_STEP)
                .text_size(theme::TEXT_SECTION)
                .font_weight(FontWeight::BOLD)
                .text_color(colors.muted)
                .child(div().flex_1().child(match side {
                    DiffSide::Staged => "暂存的更改",
                    DiffSide::Worktree => "更改",
                }))
                .child(count_badge(count, colors))
                .when_some(self.scm_paths(g, None, side), |row, request| {
                    row.child(
                        Button::new(("scm-all", index))
                            .xsmall()
                            .ghost()
                            .icon(match side {
                                DiffSide::Staged => IconName::Minus,
                                DiffSide::Worktree => IconName::Plus,
                            })
                            .accessibility_label(match side {
                                DiffSide::Staged => "取消暂存",
                                DiffSide::Worktree => "暂存更改",
                            })
                            .tooltip(match side {
                                DiffSide::Staged => "全部取消暂存",
                                DiffSide::Worktree => "全部暂存",
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.request_git_write(request.clone(), window, cx);
                            })),
                    )
                }),
            Row::File(g, i, side) => {
                let Some(Ok(status)) = &self.groups[g].status else {
                    return base.into_any_element();
                };
                let change = &status.changes[i];
                let file_path = self.groups[g].repo.worktree.join(&change.path);
                let mut discard = self.scm_paths(g, Some(i), DiffSide::Worktree);
                if let Some(request) = &mut discard
                    && let workspace_editor_git::WriteOperation::Stage { paths } =
                        &request.operation
                {
                    request.operation = workspace_editor_git::WriteOperation::Discard {
                        paths: paths.clone(),
                    };
                }
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
                let weak = cx.weak_entity();
                let context_file = file_path.clone();
                let context_stage = self.scm_paths(g, Some(i), side);
                let context_discard = discard.clone().filter(|_| {
                    matches!(side, DiffSide::Worktree)
                        && change.kind != workspace_editor_git::ChangeKind::Untracked
                        && change.kind != workspace_editor_git::ChangeKind::Conflict
                });
                let row = base
                    .pl(theme::TREE_BASE + theme::TREE_STEP + theme::TWISTY_WIDTH)
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
                    .child(
                        Button::new(("scm-open", index))
                            .xsmall()
                            .ghost()
                            .icon(IconName::File)
                            .tooltip("打开文件")
                            .accessibility_label("打开文件")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.scm_open_file(&file_path, window, cx);
                            })),
                    )
                    .when(
                        matches!(side, DiffSide::Worktree)
                            && change.kind != workspace_editor_git::ChangeKind::Untracked
                            && change.kind != workspace_editor_git::ChangeKind::Conflict,
                        |row| {
                            row.when_some(discard, |row, request| {
                                row.child(
                                    Button::new(("scm-discard", index))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Undo2)
                                        .tooltip("放弃更改")
                                        .accessibility_label("放弃更改")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.request_git_write(request.clone(), window, cx);
                                        })),
                                )
                            })
                        },
                    )
                    .when_some(self.scm_paths(g, Some(i), side), |row, request| {
                        row.child(
                            Button::new(("scm-stage", index))
                                .xsmall()
                                .ghost()
                                .icon(match side {
                                    DiffSide::Staged => IconName::Minus,
                                    DiffSide::Worktree => IconName::Plus,
                                })
                                .accessibility_label(match side {
                                    DiffSide::Staged => "取消暂存",
                                    DiffSide::Worktree => "暂存更改",
                                })
                                .tooltip(match side {
                                    DiffSide::Staged => "取消暂存",
                                    DiffSide::Worktree => "暂存更改",
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_git_write(request.clone(), window, cx);
                                })),
                        )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_diff(g, i, side, window, cx)
                    }));
                return row
                    .context_menu(move |menu, _, _| {
                        let view = weak.clone();
                        let path = context_file.clone();
                        menu.item(
                            PopupMenuItem::new("打开文件").on_click(move |_, window, cx| {
                                let _ = view
                                    .update(cx, |this, cx| this.scm_open_file(&path, window, cx));
                            }),
                        )
                        .when_some(context_stage.clone(), |menu, request| {
                            menu.item(git_menu_item(
                                match side {
                                    DiffSide::Staged => "取消暂存",
                                    DiffSide::Worktree => "暂存更改",
                                },
                                request,
                                weak.clone(),
                            ))
                        })
                        .when_some(
                            context_discard.clone(),
                            |menu, request| {
                                menu.item(git_menu_item("放弃更改…", request, weak.clone()))
                            },
                        )
                    })
                    .into_any_element();
            }
        };
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}

fn git_menu_item(
    label: &str,
    request: workspace_editor_git::WriteRequest,
    view: WeakEntity<Prototype>,
) -> PopupMenuItem {
    PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| {
        let _ = view.update(cx, |this, cx| {
            this.request_git_write(request.clone(), window, cx)
        });
    })
}
