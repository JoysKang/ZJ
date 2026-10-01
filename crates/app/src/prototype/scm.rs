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
        input::{Enter, Textarea},
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
                // ⌘Enter commits; capture it before the textarea inserts a line break.
                div()
                    .capture_action(cx.listener({
                        let id = id.clone();
                        move |this, action: &Enter, window, cx| {
                            if action.secondary {
                                cx.stop_propagation();
                                this.scm_commit(id.clone(), false, window, cx);
                            }
                        }
                    }))
                    .child(
                        Textarea::new(&group.commit_input)
                            .h(theme::COMMIT_HEIGHT)
                            .disabled(group.write_pending)
                            .aria_label(format!("{target} 的提交消息")),
                    ),
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
                list(
                    self.scm_list.clone(),
                    cx.processor(|this, index: usize, _, cx| this.scm_row(index, cx)),
                )
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }

    fn scm_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let hover: SharedString = format!("scm-row-{index}").into();
        // The list can ask for a row while `rows` is being rebuilt.
        let Some(&row) = self.rows.get(index) else {
            return div().into_any_element();
        };
        if let Row::Group(g) = row {
            return div()
                .w_full()
                .px(theme::ROW_INSET)
                .child(self.scm_repo_row(index, g, hover, cx))
                .into_any_element();
        }
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
        let row = match row {
            Row::Group(_) => unreachable!("repository rows return above"),
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
        index: usize,
        g: usize,
        hover: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let group = &self.groups[g];
        let worktree = &group.repo.worktree;
        // Line 1: the repository's own folder name, then (dim) where it sits in the workspace.
        let name = worktree
            .file_name()
            .unwrap_or(worktree.as_os_str())
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let parent = self
            .root
            .as_deref()
            .and_then(|root| worktree.parent()?.strip_prefix(root).ok())
            .map(|path| path.to_string_lossy().replace(SINGLE_LINE, "⏎"))
            .filter(|path| !path.is_empty());
        // The branch (with ↑N / ↓M when non-zero against its upstream), or the error.
        let (detail, error, count) = match &group.status {
            None => (None, false, None),
            Some(Err(error)) => (Some(format!("错误：{error}")), true, None),
            Some(Ok(status)) => {
                let mut branch = status.branch.as_deref().unwrap_or("未知分支").to_string();
                if status.upstream.is_some() {
                    if status.ahead > 0 {
                        branch.push_str(&format!("\u{a0}\u{a0}↑{}", status.ahead));
                    }
                    if status.behind > 0 {
                        branch.push_str(&format!("\u{a0}\u{a0}↓{}", status.behind));
                    }
                }
                (Some(branch), false, Some(status.changes.len()))
            }
        };
        let label = format!("仓库 {name} · {}", detail.as_deref().unwrap_or_default());
        let mut title = soft_breaks(&name);
        let name_len = title.len();
        if let Some(parent) = &parent {
            title.push_str("  ");
            title.push_str(&soft_breaks(parent));
        }
        let dim = HighlightStyle {
            color: Some(colors.muted),
            font_weight: Some(FontWeight::NORMAL),
            ..Default::default()
        };
        let title_text = StyledText::new(title.clone())
            .with_highlights((name_len < title.len()).then_some((name_len..title.len(), dim)));
        let tooltip = worktree.display().to_string();
        let selected = self.groups.len() > 1 && self.selected_group() == Some(g);
        let id = group.repo.id.clone();
        let weak = cx.weak_entity();
        // The actions cover the end of the first line without moving it; their background matches the
        // hovered (or selected) row and fades in from the left.
        let overlay_bg = if selected {
            colors.selected
        } else {
            colors.hover
        };
        v_flex()
            .id(("scm-row", index))
            .group(hover.clone())
            .relative()
            .w_full()
            .pl(theme::ROW_INSET)
            .pr_1()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .cursor_pointer()
            .hover(|row| row.bg(colors.hover))
            .when(selected, |row| row.bg(colors.selected))
            .role(Role::TreeItem)
            .aria_label(label)
            // One line (name, then the dim branch) when both fit; otherwise flex-wrap drops the
            // branch onto its own line under the name, and either one wraps further at `/` `-`
            // `_` if it is still too long. The list re-lays rows out when its width changes.
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_1()
                    .child(
                        div()
                            .h(theme::ROW_HEIGHT)
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .child(chevron(group.expanded, colors.muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_x_1p5()
                            .child(
                                div()
                                    .min_w_0()
                                    // 18 px lines inside a 22 px first line.
                                    .py(px(2.))
                                    .text_size(theme::TEXT_BODY)
                                    .line_height(theme::SCM_DETAIL_LINE)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(title_text),
                            )
                            .when_some(detail, |line, detail| {
                                line.child(
                                    h_flex()
                                        .min_w_0()
                                        .items_start()
                                        .gap_1()
                                        .text_size(theme::TEXT_SECTION)
                                        .line_height(theme::SCM_DETAIL_LINE)
                                        .text_color(if error {
                                            colors.deleted
                                        } else {
                                            colors.muted
                                        })
                                        .when(!error, |line| {
                                            line.child(
                                                div()
                                                    .h(theme::SCM_DETAIL_LINE)
                                                    .flex()
                                                    .items_center()
                                                    .flex_shrink_0()
                                                    .child(
                                                        Icon::new(IconName::GitBranch)
                                                            .size(theme::SMALL_ICON_SIZE)
                                                            .text_color(colors.muted),
                                                    ),
                                            )
                                        })
                                        .child(div().min_w_0().child(soft_breaks(&detail))),
                                )
                            }),
                    )
                    .when_some(count.filter(|count| *count > 0), |row, count| {
                        row.child(
                            div()
                                .h(theme::ROW_HEIGHT)
                                .flex()
                                .items_center()
                                .flex_shrink_0()
                                .child(count_badge(count, colors)),
                        )
                    }),
            )
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h(theme::ROW_HEIGHT)
                    .opacity(0.)
                    .group_hover(hover.clone(), |actions| actions.opacity(1.))
                    .child(div().w(theme::ACTION_FADE).h_full().bg(linear_gradient(
                        90.,
                        linear_color_stop(overlay_bg.opacity(0.), 0.),
                        linear_color_stop(overlay_bg, 1.),
                    )))
                    .child(
                        hover_actions(hover)
                            .h_full()
                            .pr_1()
                            .items_center()
                            .bg(overlay_bg)
                            .child(
                                action(("scm-repo-commit", g), IconName::Check, "提交")
                                    .disabled(group.write_pending)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.scm_commit(id.clone(), false, window, cx)
                                    })),
                            )
                            .child(
                                action(("scm-repo-refresh", g), IconName::RefreshCw, "刷新")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.refresh(window, cx)
                                    })),
                            )
                            .child(
                                action(("scm-repo-more", g), IconName::Ellipsis, "更多操作")
                                    .dropdown_menu(move |menu, _, cx| {
                                        let view = weak.clone();
                                        match weak.upgrade() {
                                            Some(this) => this.read(cx).scm_menu(g, menu, view),
                                            None => menu,
                                        }
                                    }),
                            ),
                    ),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
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
        let collapsed = match side {
            DiffSide::Staged => self.groups[g].staged_collapsed,
            DiffSide::Worktree => self.groups[g].changes_collapsed,
        };
        base.pl(theme::TREE_BASE)
            .text_color(colors.foreground)
            .role(Role::TreeItem)
            .aria_expanded(!collapsed)
            .child(chevron(!collapsed, colors.muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(match side {
                        DiffSide::Staged => "暂存的更改",
                        DiffSide::Worktree => "更改",
                    }),
            )
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
            .on_click(cx.listener(move |this, _, _, cx| {
                let group = &mut this.groups[g];
                match side {
                    DiffSide::Staged => group.staged_collapsed = !group.staged_collapsed,
                    DiffSide::Worktree => group.changes_collapsed = !group.changes_collapsed,
                }
                this.rebuild_rows();
                cx.notify();
            }))
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

/// Lets a long name wrap after `/`, `-` and `_` (gpui keeps `a-b` and `a_b` together and
/// otherwise breaks before `/`), so branch and repository names wrap instead of being cut.
fn soft_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        out.push(c);
        if matches!(c, '/' | '-' | '_') {
            out.push('\u{200B}');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::soft_breaks;

    #[test]
    fn long_names_wrap_after_separators() {
        assert_eq!(soft_breaks("main"), "main");
        assert_eq!(
            soft_breaks("feature/pay-q4_x"),
            "feature/\u{200B}pay-\u{200B}q4_\u{200B}x"
        );
        assert_eq!(soft_breaks("功能/分支"), "功能/\u{200B}分支");
    }
}
