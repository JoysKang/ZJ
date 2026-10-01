//! Source Control view, VS Code style: a commit box for the selected repository, then every
//! repository of the workspace with its "暂存的更改" / "更改" groups. Row actions appear on hover;
//! the status letter stays at the right edge.

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
        menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use workspace_editor_git::{ChangeKind, DiffSide, WriteOperation, WriteRequest};

fn count_badge(count: usize, colors: theme::Colors) -> impl IntoElement {
    div()
        .h(theme::BADGE_SIZE + theme::ROW_INSET)
        .min_w(theme::BADGE_SIZE + theme::ROW_INSET)
        .px_1()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(colors.keycap)
        .text_color(colors.foreground)
        .text_size(theme::TEXT_SECTION)
        .child(count.to_string())
}

/// A small icon button for row actions.
fn action(id: impl Into<ElementId>, icon: IconName, label: &'static str) -> Button {
    Button::new(id)
        .xsmall()
        .ghost()
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
}

/// Actions that only show while the row (CSS-like group) is hovered, as in VS Code.
fn hover_actions(group: SharedString) -> Div {
    h_flex()
        .flex_shrink_0()
        .gap_0p5()
        .opacity(0.)
        .group_hover(group, |actions| actions.opacity(1.))
}

impl Prototype {
    fn selected_group(&self) -> Option<usize> {
        self.scm_repo
            .as_ref()
            .and_then(|id| self.groups.iter().position(|g| &g.repo.id == id))
            .or_else(|| (!self.groups.is_empty()).then_some(0))
    }

    /// The "···" menu of a repository: push and whole-repository operations.
    fn scm_menu(&self, g: usize, menu: PopupMenu, view: WeakEntity<Self>) -> PopupMenu {
        let stage = self.scm_paths(g, None, DiffSide::Worktree);
        let unstage = self.scm_paths(g, None, DiffSide::Staged);
        let discard = self.scm_discard(g, None);
        let group = &self.groups[g];
        let push = group
            .status
            .as_ref()
            .and_then(|status| status.as_ref().ok())
            .filter(|status| status.upstream.is_some() && !group.write_pending)
            .map(|status| WriteRequest {
                repo: group.repo.clone(),
                generation: 0,
                expected: status.clone(),
                operation: WriteOperation::Push,
            });
        let refresh = view.clone();
        menu.item(PopupMenuItem::new("刷新").on_click(move |_, window, cx| {
            let _ = refresh.update(cx, |this, cx| this.refresh(window, cx));
        }))
        .separator()
        .when_some(push, |menu, request| {
            menu.item(git_menu_item("推送", request, view.clone()))
        })
        .when_some(stage, |menu, request| {
            menu.item(git_menu_item("暂存所有更改", request, view.clone()))
        })
        .when_some(unstage, |menu, request| {
            menu.item(git_menu_item("取消暂存所有更改", request, view.clone()))
        })
        .when_some(discard, |menu, request| {
            menu.item(git_menu_item("放弃所有更改…", request, view.clone()))
        })
    }

    pub(super) fn scm_more(&self, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.weak_entity();
        let Some(g) = self.selected_group() else {
            return div().into_any_element();
        };
        action("scm-more-button", IconName::Ellipsis, "更多操作")
            .dropdown_menu(move |menu, _, cx| {
                let view = weak.clone();
                match weak.upgrade() {
                    Some(this) => this.read(cx).scm_menu(g, menu, view),
                    None => menu,
                }
            })
            .into_any_element()
    }

    /// Commit box: message input and the primary 提交 button for the selected repository.
    fn scm_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(group) = self.selected_group().map(|g| &self.groups[g]) else {
            return div().into_any_element();
        };
        let id = group.repo.id.clone();
        let status = group.status.as_ref().and_then(|s| s.as_ref().ok());
        let can_commit = status.is_some_and(|s| {
            s.changes.iter().any(|c| c.staged())
                && !s.changes.iter().any(|c| c.kind == ChangeKind::Conflict)
        });
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
        v_flex()
            .flex_shrink_0()
            .px_2()
            .pb_2()
            .gap_2()
            .child(
                Textarea::new(&group.commit_input)
                    .h(theme::COMMIT_HEIGHT)
                    .disabled(group.write_pending)
                    .aria_label(format!("{target} 的提交消息")),
            )
            .child(
                Button::new("scm-commit")
                    .small()
                    .primary()
                    .w_full()
                    .icon(IconName::Check)
                    .label("提交")
                    .tooltip(format!("提交到“{branch}”（{target}）"))
                    .disabled(group.write_pending || !can_commit)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.scm_commit(id.clone(), false, window, cx)
                    })),
            )
            .when(!group.write_message.is_empty(), |view| {
                view.child(
                    div()
                        .id("git-operation-result")
                        .max_h(theme::SCM_NOTICE_MAX)
                        .overflow_y_scroll()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
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
        let hover: SharedString = format!("scm-row-{index}").into();
        let base = h_flex()
            .id(("scm-row", index))
            .group(hover.clone())
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pr_1()
            .gap_1()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .cursor_pointer()
            .hover(|row| row.bg(colors.hover));
        let row = match self.rows[index] {
            Row::Group(g) => self.scm_repo_row(g, base, hover, cx),
            Row::Heading(g, side, count) => self.scm_heading_row(g, side, count, base, hover, cx),
            Row::File(g, i, side) => self.scm_file_row(index, g, i, side, base, hover, cx),
        };
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }

    fn scm_repo_row(
        &self,
        g: usize,
        base: Stateful<Div>,
        hover: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
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
        let selected = self.groups.len() > 1 && self.selected_group() == Some(g);
        let id = group.repo.id.clone();
        let weak = cx.weak_entity();
        base.pl(theme::ROW_INSET)
            .role(Role::TreeItem)
            .aria_label(format!("仓库 {name} · {detail}"))
            .when(selected, |row| row.bg(colors.selected))
            .child(chevron(group.expanded, colors.muted))
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .when(!detail.is_empty(), |row| {
                row.child(
                    Icon::new(IconName::GitBranch)
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(colors.muted),
                )
            })
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
            .child(
                hover_actions(hover)
                    .child(
                        action(("scm-repo-commit", g), IconName::Check, "提交")
                            .disabled(group.write_pending)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.scm_commit(id.clone(), false, window, cx)
                            })),
                    )
                    .child(
                        action(("scm-repo-refresh", g), IconName::RefreshCw, "刷新").on_click(
                            cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.refresh(window, cx)
                            }),
                        ),
                    )
                    .child(
                        action(("scm-repo-more", g), IconName::Ellipsis, "更多操作").dropdown_menu(
                            move |menu, _, cx| {
                                let view = weak.clone();
                                match weak.upgrade() {
                                    Some(this) => this.read(cx).scm_menu(g, menu, view),
                                    None => menu,
                                }
                            },
                        ),
                    ),
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
            .into_any_element()
    }

    fn scm_heading_row(
        &self,
        g: usize,
        side: DiffSide,
        count: usize,
        base: Stateful<Div>,
        hover: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let toggle = self.scm_paths(g, None, side);
        let discard = matches!(side, DiffSide::Worktree)
            .then(|| self.scm_discard(g, None))
            .flatten();
        base.pl(theme::TREE_BASE + theme::TREE_STEP)
            .text_color(colors.foreground)
            .child(div().flex_1().min_w_0().child(match side {
                DiffSide::Staged => "暂存的更改",
                DiffSide::Worktree => "更改",
            }))
            .child(
                hover_actions(hover)
                    .when_some(discard, |actions, request| {
                        actions.child(
                            action(("scm-discard-all", g), IconName::Undo2, "放弃所有更改")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_git_write(request.clone(), window, cx);
                                })),
                        )
                    })
                    .when_some(toggle, |actions, request| {
                        let (icon, label) = match side {
                            DiffSide::Staged => (IconName::Minus, "取消暂存所有更改"),
                            DiffSide::Worktree => (IconName::Plus, "暂存所有更改"),
                        };
                        actions.child(
                            action(("scm-all", g * 2 + side as usize), icon, label).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_git_write(request.clone(), window, cx);
                                }),
                            ),
                        )
                    }),
            )
            .child(count_badge(count, colors))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn scm_file_row(
        &self,
        index: usize,
        g: usize,
        i: usize,
        side: DiffSide,
        base: Stateful<Div>,
        hover: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(Ok(status)) = &self.groups[g].status else {
            return base.into_any_element();
        };
        let change = &status.changes[i];
        let file_path = self.groups[g].repo.worktree.join(&change.path);
        let discard = matches!(side, DiffSide::Worktree)
            .then(|| self.scm_discard(g, Some(i)))
            .flatten();
        let toggle = self.scm_paths(g, Some(i), side);
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
        let deleted = decoration.letter == 'D';
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
        // The row whose diff is open is selected, as in VS Code.
        let selected = self.preview_diff.as_ref().is_some_and(|diff| {
            diff.path == file_path
                && match &diff.request.operation {
                    workspace_editor_git::Operation::Diff { side: open, .. } => {
                        matches!(
                            (open, side),
                            (DiffSide::Staged, DiffSide::Staged)
                                | (DiffSide::Worktree, DiffSide::Worktree)
                        )
                    }
                    workspace_editor_git::Operation::UntrackedDiff { .. } => true,
                    _ => false,
                }
        });
        let untracked = change.kind == ChangeKind::Untracked;
        let (toggle_icon, toggle_label) = match side {
            DiffSide::Staged => (IconName::Minus, "取消暂存更改"),
            DiffSide::Worktree => (IconName::Plus, "暂存更改"),
        };
        let discard_label = if untracked {
            "删除文件"
        } else {
            "放弃更改"
        };
        let weak = cx.weak_entity();
        let context_file = file_path.clone();
        let context_toggle = toggle.clone();
        let context_discard = discard.clone();
        let open_file = file_path.clone();
        base.pl(theme::TREE_BASE + theme::TREE_STEP + theme::TWISTY_WIDTH)
            .role(Role::Button)
            .aria_label(format!(
                "{} · {} · {}",
                match side {
                    DiffSide::Staged => "暂存的更改",
                    DiffSide::Worktree => "更改",
                },
                self.groups[g].repo.worktree.display(),
                change.path.display()
            ))
            .when(selected, |row| row.bg(colors.selected))
            .child(file_icons::icon(file_icons::for_file(&name)))
            .child(
                div()
                    .flex_shrink_0()
                    .pl_1()
                    .text_color(color)
                    .when(deleted, |name| name.line_through())
                    .child(name),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .pl_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(directory),
            )
            .child(
                hover_actions(hover)
                    .child(
                        action(("scm-open", index), IconName::File, "打开文件").on_click(
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.scm_open_file(&open_file, window, cx);
                            }),
                        ),
                    )
                    .when_some(discard, |actions, request| {
                        actions.child(
                            action(("scm-discard", index), IconName::Undo2, discard_label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_git_write(request.clone(), window, cx);
                                })),
                        )
                    })
                    .when_some(toggle, |actions, request| {
                        actions.child(
                            action(("scm-stage", index), toggle_icon, toggle_label).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.request_git_write(request.clone(), window, cx);
                                }),
                            ),
                        )
                    }),
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
            .on_click(
                cx.listener(move |this, _, window, cx| this.open_diff(g, i, side, window, cx)),
            )
            .context_menu(move |menu, _, _| {
                let view = weak.clone();
                let path = context_file.clone();
                menu.item(
                    PopupMenuItem::new("打开文件").on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.scm_open_file(&path, window, cx));
                    }),
                )
                .when_some(context_toggle.clone(), |menu, request| {
                    menu.item(git_menu_item(toggle_label, request, weak.clone()))
                })
                .when_some(context_discard.clone(), |menu, request| {
                    menu.item(git_menu_item(
                        if untracked {
                            "删除文件…"
                        } else {
                            "放弃更改…"
                        },
                        request,
                        weak.clone(),
                    ))
                })
            })
            .into_any_element()
    }
}

fn git_menu_item(label: &str, request: WriteRequest, view: WeakEntity<Prototype>) -> PopupMenuItem {
    PopupMenuItem::new(label.to_string()).on_click(move |_, window, cx| {
        let _ = view.update(cx, |this, cx| {
            this.request_git_write(request.clone(), window, cx)
        });
    })
}
