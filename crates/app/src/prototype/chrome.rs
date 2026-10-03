//! Window chrome: the unified title bar with its command center.

use super::{Pane, Prototype};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable, TitleBar,
        button::{Button, ButtonVariants},
        h_flex,
    },
    prelude::FluentBuilder,
    *,
};

fn status_item(id: &'static str, colors: theme::Colors) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h_full()
        .px_2()
        .gap_1()
        .flex_shrink_0()
        .hover(|item| item.bg(colors.hover).text_color(colors.foreground))
}

impl Prototype {
    pub(super) fn workspace_name(&self) -> String {
        self.root
            .as_ref()
            .map(|root| {
                root.file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "ZJ".into())
    }

    pub(super) fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let command_center = h_flex()
            .id("command-center")
            .w(theme::COMMAND_CENTER_WIDTH)
            .min_w_0()
            .h(theme::COMMAND_CENTER_HEIGHT)
            .px_2()
            .gap_2()
            .justify_center()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.command_border)
            .bg(colors.command_bg)
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted)
            .cursor_pointer()
            .hover(|this| this.bg(colors.hover).text_color(colors.foreground))
            .role(Role::Button)
            .aria_label("转到文件（⌘P）")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.open_quick_open(window, cx)))
            .child(
                Icon::new(IconName::Search)
                    .size(theme::SMALL_ICON_SIZE)
                    .text_color(colors.muted),
            )
            .child(self.workspace_name());
        let actions = h_flex()
            .flex_1()
            .justify_end()
            .pr_2()
            .gap_1()
            .child(
                Button::new("new-window")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Files)
                    .tooltip("新建窗口（⌘⇧N）")
                    .on_click(cx.listener(|this, _, _, cx| this.new_window(cx))),
            )
            .child(
                Button::new("toggle-sidebar")
                    .xsmall()
                    .ghost()
                    .icon(if self.sidebar_visible {
                        IconName::PanelLeftClose
                    } else {
                        IconName::PanelLeftOpen
                    })
                    .tooltip("切换侧栏（⌘B）")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
            )
            .child(
                Button::new("toggle-agent")
                    .xsmall()
                    .ghost()
                    .icon(if self.agent.visible {
                        IconName::PanelRightClose
                    } else {
                        IconName::PanelRightOpen
                    })
                    .tooltip("切换 Agent 面板（⌥⌘B）")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_agent_panel(window, cx)),
                    ),
            );
        TitleBar::new()
            .h(theme::TITLE_HEIGHT)
            .bg(colors.title)
            .border_color(colors.border)
            .child(
                h_flex()
                    .size_full()
                    .items_center()
                    .child(div().flex_1())
                    .child(command_center)
                    .child(actions),
            )
            .into_any_element()
    }

    /// Branch of the repository that contains the active file, or of the workspace root.
    pub(super) fn active_branch(&self) -> Option<String> {
        let group = &self.groups[self.active_group()?];
        match &group.status {
            Some(Ok(status)) => status.branch.clone(),
            _ => None,
        }
    }

    /// The repository that contains the active file, or the workspace root.
    pub(super) fn active_group(&self) -> Option<usize> {
        let path = match self.active {
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| doc.path.clone()),
            Pane::Diff => self.preview_diff.as_ref().map(|diff| diff.path.clone()),
            Pane::Welcome => None,
        }
        .or_else(|| self.root.clone())?;
        (0..self.groups.len())
            .filter(|g| path.starts_with(&self.groups[*g].repo.worktree))
            .max_by_key(|g| self.groups[*g].repo.worktree.as_os_str().len())
    }

    /// (waiting for approval and the agent's name when there is one, running, done unread).
    fn agent_status_counts(&self) -> ((usize, Option<String>), usize, usize) {
        use crate::agent_model::RowStatus;
        let mut awaiting = (0, None);
        let (mut running, mut unread) = (0, 0);
        for session in &self.agent.sessions {
            match session.row_status() {
                RowStatus::Awaiting => {
                    awaiting.0 += 1;
                    awaiting.1 = Some(session.preset.display_name.clone());
                }
                RowStatus::Running => running += 1,
                RowStatus::Unread => unread += 1,
                _ => {}
            }
        }
        (awaiting, running, unread)
    }

    pub(super) fn render_status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let document = match self.active {
            Pane::Document(id) => self.documents.iter().find(|doc| doc.id == id),
            _ => None,
        };
        let language = match self.active {
            Pane::Diff => self
                .preview_diff
                .as_ref()
                .map(|diff| super::language_for(&diff.path).1),
            _ => document.map(|doc| doc.language),
        };
        let icon = |name: IconName| {
            Icon::new(name)
                .size(theme::SMALL_ICON_SIZE)
                .text_color(colors.muted)
        };
        // As in VS Code: the branch opens 签出到…; with an upstream, 同步更改 shows ↓M ↑N.
        let active = self.active_group();
        let sync = active.and_then(|g| {
            let status = self.groups[g].status.as_ref()?.as_ref().ok()?;
            let upstream = status.upstream.clone()?;
            Some((g, upstream, status.behind, status.ahead))
        });
        let left = h_flex()
            .h_full()
            .min_w_0()
            .when_some(active.zip(self.active_branch()), |bar, (g, branch)| {
                bar.child(
                    status_item("status-branch", colors)
                        .child(icon(IconName::GitBranch))
                        .child(branch.clone())
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(format!(
                                "{branch}，签出分支…"
                            ))
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !this.groups.get(g).is_some_and(|g| g.write_pending) {
                                this.open_branch_picker(g, window, cx);
                            }
                        })),
                )
            })
            .when_some(sync, |bar, (g, upstream, behind, ahead)| {
                let pending = self.groups[g].write_pending;
                let counts = (behind > 0 || ahead > 0).then(|| format!("{behind}↓ {ahead}↑"));
                let tooltip = match &counts {
                    Some(counts) => format!("同步更改 {counts}（{upstream}）"),
                    None => format!("同步更改（{upstream}）"),
                };
                bar.child(
                    status_item("status-sync", colors)
                        .child(icon(IconName::RefreshCw))
                        .when_some(counts, |item, counts| item.child(counts))
                        .when(pending, |item| item.opacity(0.5))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone())
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let sync = workspace_editor_git::WriteOperation::Sync;
                            if let Some(request) = this.scm_request(g, sync) {
                                this.request_git_write(request, window, cx);
                            }
                        })),
                )
            })
            .when(!self.issues.is_empty(), |bar| {
                let first = self.issues.first().cloned().unwrap_or_default();
                bar.child(
                    status_item("status-issues", colors)
                        .child(icon(IconName::TriangleAlert))
                        .child(format!("{} 项仓库错误", self.issues.len()))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(first.clone())
                                .build(window, cx)
                        }),
                )
            })
            .when_some(self.watch_error.clone(), |bar, error| {
                bar.child(
                    status_item("status-watch-error", colors)
                        .child(icon(IconName::TriangleAlert))
                        .child("文件监听异常")
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(error.clone())
                                .build(window, cx)
                        }),
                )
            })
            .when(!self.message.is_empty(), |bar| {
                bar.child(
                    div()
                        .px_2()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(self.message.clone()),
                )
            });
        let (awaiting, running, unread) = self.agent_status_counts();
        let right = h_flex()
            .h_full()
            .flex_shrink_0()
            .when(awaiting.0 > 0, |bar| {
                let label = match (&awaiting.1, awaiting.0) {
                    (Some(agent), 1) => format!("{agent} · 待批准"),
                    (_, n) => format!("{n} 待批准"),
                };
                bar.child(
                    status_item("status-agent-awaiting", colors)
                        .child(super::agent_panel::status_mark(
                            crate::agent_model::RowStatus::Awaiting,
                            0,
                            colors,
                        ))
                        .child(div().text_color(colors.foreground).child(label))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("跳到下一个待批准（⌘⇧A）")
                                .build(window, cx)
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.agent_next_approval(window, cx)),
                        ),
                )
            })
            .when(running > 0, |bar| {
                bar.child(
                    status_item("status-agent-running", colors)
                        .child(super::agent_panel::spinner(self.agent.spin, colors))
                        .child(format!("{running} 运行中"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if !this.agent.visible {
                                this.set_agent_panel(true, window, cx);
                            }
                        })),
                )
            })
            .when(unread > 0 && !self.agent.visible, |bar| {
                bar.child(
                    status_item("status-agent-unread", colors)
                        .child(super::agent_panel::status_mark(
                            crate::agent_model::RowStatus::Unread,
                            0,
                            colors,
                        ))
                        .child(format!("{unread} 完成未读"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_agent_panel(true, window, cx);
                        })),
                )
            })
            .when_some(self.cursor, |bar, (line, column)| {
                bar.child(status_item("status-cursor", colors).child(format!(
                    "行 {}，列 {}",
                    line + 1,
                    column + 1
                )))
            })
            .when_some(document, |bar, doc| {
                bar.child(status_item("status-encoding", colors).child(if doc.bom {
                    "UTF-8 BOM"
                } else {
                    "UTF-8"
                }))
                .child(
                    status_item("status-eol", colors)
                        .cursor_pointer()
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("切换行尾序列（LF / CRLF）")
                                .build(window, cx)
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_line_ending(window, cx)),
                        )
                        .child(if doc.crlf { "CRLF" } else { "LF" }),
                )
            })
            .when_some(language, |bar, language| {
                bar.child(status_item("status-language", colors).child(language))
            });
        h_flex()
            .h(theme::STATUS_HEIGHT)
            .flex_shrink_0()
            .justify_between()
            .overflow_hidden()
            .bg(colors.panel)
            .border_t_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted)
            .child(left)
            .child(right)
            .into_any_element()
    }
}
