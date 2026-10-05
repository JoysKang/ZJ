//! The session list in the agent panel (design 02-A): the search box (opens ⌘J), filter
//! chips, then 已钉住 (drag to reorder) / 今天 / 昨天 / 本周 / 更早 with hover actions.

use super::agent::{HistoryOp, agent_name, glyph_for, workspace_label};
use super::agent_panel::{glyph_tile, status_mark};
use super::*;
use crate::agent_model::{self, Bucket, ListRow, RowStatus};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        menu::{DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
};
use workspace_editor_agent_history::{SessionId, SessionSummary};

/// A pinned row being dragged.
#[derive(Clone)]
pub(super) struct DraggedSession {
    pub id: SessionId,
    pub title: String,
}

pub(super) struct DragGhost {
    title: String,
}

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        div()
            .px_2()
            .py_1()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.strong_border)
            .bg(colors.selected)
            .text_color(colors.selected_fg)
            .text_size(theme::TEXT_CAPTION)
            .child(self.title.clone())
    }
}

pub(super) fn filter_chip(
    id: &'static str,
    label: String,
    on: bool,
    colors: theme::Colors,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(theme::AGENT_FILTER_CHIP)
        .px_2()
        .gap_1()
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .text_size(theme::TEXT_SECTION)
        .cursor_pointer()
        .whitespace_nowrap()
        .map(|chip| {
            if on {
                chip.bg(colors.chip_on)
                    .border_color(colors.chip_on_border)
                    .text_color(colors.foreground)
            } else {
                chip.border_color(colors.card_border)
                    .text_color(colors.muted)
                    .hover(|chip| chip.bg(colors.hover))
            }
        })
        .child(label)
}

impl Workbench {
    pub(super) fn render_agent_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let history = &self.agent.history;
        let filter = &history.filter;
        let running = self
            .agent
            .sessions
            .iter()
            .filter(|s| s.row_status() == RowStatus::Running)
            .count();
        let pending = self
            .agent
            .sessions
            .iter()
            .filter(|s| matches!(s.row_status(), RowStatus::Awaiting | RowStatus::Unread))
            .count();
        let search = h_flex()
            .id("agent-history-search")
            .h(theme::AGENT_SEARCH_INPUT)
            .px_2()
            .gap_2()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.strong_border)
            .bg(colors.card)
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted)
            .cursor_pointer()
            .child(Icon::new(IconName::Search).size(theme::SMALL_ICON_SIZE))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child("搜索标题、消息、文件、分支…"),
            )
            .child(
                div()
                    .px_1()
                    .rounded(theme::RADIUS)
                    .bg(colors.keycap)
                    .text_size(theme::TEXT_BADGE)
                    .child("⌘J"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.agent_open_search(window, cx)));
        let agent_menu = {
            let weak = cx.weak_entity();
            let presets: Vec<(String, String)> = self
                .agent
                .presets
                .iter()
                .map(|p| (p.id.clone(), p.display_name.clone()))
                .collect();
            let current = filter.agent.clone();
            let label = match &current {
                Some(id) => format!("Agent：{}", agent_name(&self.agent.presets, id)),
                None => "Agent：全部".into(),
            };
            Button::new("agent-filter-agent")
                .ghost()
                .xsmall()
                .label(label)
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let all = weak.clone();
                    let mut menu = menu.item(
                        PopupMenuItem::new("全部 Agent")
                            .checked(current.is_none())
                            .on_click(move |_, window, cx| {
                                let _ = all.update(cx, |this, cx| {
                                    this.agent_set_filter(|f| f.agent = None, window, cx)
                                });
                            }),
                    );
                    for (id, name) in &presets {
                        let weak = weak.clone();
                        let id = id.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(current.as_deref() == Some(id.as_str()))
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        this.agent_set_filter(|f| f.agent = Some(id), window, cx)
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let chips = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                filter_chip(
                    "agent-filter-scope",
                    self.agent_scope_label(filter.all_workspaces, cx),
                    true,
                    colors,
                )
                .child(Icon::new(IconName::ChevronDown).size(theme::SMALL_ICON_SIZE))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.agent_set_filter(|f| f.all_workspaces = !f.all_workspaces, window, cx)
                })),
            )
            .child(agent_menu)
            .child(
                filter_chip(
                    "agent-filter-running",
                    format!("进行中 {running}"),
                    filter.running,
                    colors,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.agent_set_filter(|f| f.running = !f.running, window, cx)
                })),
            )
            .child(
                filter_chip(
                    "agent-filter-pending",
                    format!("待处理 {pending}"),
                    filter.pending,
                    colors,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.agent_set_filter(|f| f.pending = !f.pending, window, cx)
                })),
            );
        let body: AnyElement = if let Some(error) = &history.error {
            div()
                .p_3()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.deleted)
                .child(error.clone())
                .into_any_element()
        } else if history.loaded && history.grouped.is_empty() {
            div()
                .p_4()
                .text_center()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(if filter.archived {
                    "没有已归档的会话"
                } else {
                    "还没有会话。发出第一条消息后，它会出现在这里。"
                })
                .into_any_element()
        } else {
            list(
                history.list.clone(),
                cx.processor(|this, index: usize, _, cx| this.render_history_row(index, cx)),
            )
            .flex_1()
            .w_full()
            .into_any_element()
        };
        let archived = history.counts.2;
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(v_flex().px_3().pt_2().gap_2().child(search).child(chips))
            .child(v_flex().flex_1().min_h_0().pt_1().child(body))
            .child(
                h_flex()
                    .h(theme::STATUS_HEIGHT)
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(
                        h_flex()
                            .id("agent-archived")
                            .gap_1()
                            .cursor_pointer()
                            .hover(|b| b.text_color(colors.foreground))
                            .child(Icon::new(IconName::Archive).size(theme::SMALL_ICON_SIZE))
                            .child(if filter.archived {
                                "返回会话列表".to_string()
                            } else {
                                format!("已归档 {archived}")
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_set_filter(|f| f.archived = !f.archived, window, cx)
                            })),
                    )
                    .when(filter.archived && archived > 0, |bar| {
                        bar.child(
                            h_flex()
                                .id("agent-delete-archived")
                                .gap_1()
                                .cursor_pointer()
                                .hover(|b| b.text_color(colors.deleted))
                                .child(Icon::new(IconName::Trash).size(theme::SMALL_ICON_SIZE))
                                .child("全部删除")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.agent_delete_archived(window, cx)
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Icon::new(IconName::Database).size(theme::SMALL_ICON_SIZE))
                            .child("本地索引"),
                    ),
            )
            .into_any_element()
    }

    fn render_history_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let history = &self.agent.history;
        let Some(row) = history.grouped.get(index) else {
            return div().into_any_element();
        };
        let compact = self.agent.compact();
        match row {
            ListRow::Header { bucket, count } => h_flex()
                .w_full()
                .h(theme::AGENT_GROUP_HEADER)
                .px_3()
                .gap_1()
                .items_end()
                .pb_1()
                .text_size(theme::TEXT_SECTION)
                .text_color(colors.muted)
                .when(*bucket == Bucket::Pinned, |h| {
                    h.child(Icon::new(IconName::Pin).size(theme::SMALL_ICON_SIZE))
                })
                .child(div().flex_1().child(bucket.label()))
                .child(if *bucket == Bucket::Pinned {
                    "拖动排序".to_string()
                } else {
                    count.to_string()
                })
                .into_any_element(),
            ListRow::Session(i) => match history.rows.get(*i) {
                Some(session) => self.render_session_row(session, compact, cx),
                None => div().into_any_element(),
            },
        }
    }

    fn render_session_row(
        &self,
        s: &SessionSummary,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let now = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now);
        let live = self.agent.sessions.iter().find(|l| l.db == Some(s.id));
        let status = live.map_or(RowStatus::of_stored(s.status), |l| l.row_status());
        let selected = live.is_some_and(|l| self.agent.current == Some(l.key));
        let unread = status == RowStatus::Unread;
        let id = s.id;
        let pinned = s.pinned();
        let other_workspace = self
            .agent_workspace(cx)
            .is_some_and(|root| root != s.workspace_root);
        let group: SharedString = format!("agent-session-{}", id.0).into();
        let fg = if selected {
            colors.selected_fg
        } else {
            colors.foreground
        };
        let muted = if selected {
            colors.selected_fg
        } else {
            colors.muted
        };
        let title = s.title.clone();
        let renaming = history_renaming(self, id);
        let glyph = glyph_for(&self.agent.presets, &s.agent_id);
        let right: AnyElement = match status {
            RowStatus::Awaiting => div()
                .text_size(theme::TEXT_SECTION)
                .text_color(if selected { fg } else { colors.attention })
                .child("待批准")
                .into_any_element(),
            RowStatus::Running => status_mark(status, self.agent.spin, colors),
            _ => div()
                .text_size(theme::TEXT_SECTION)
                .text_color(muted)
                .child(if pinned {
                    agent_model::relative_time(s.updated_at, now, offset)
                } else {
                    agent_model::row_time(s.updated_at, now, offset)
                })
                .into_any_element(),
        };
        let action = |name: &'static str, icon: IconName, tooltip: &'static str| {
            Button::new(SharedString::from(format!("agent-row-{name}-{}", id.0)))
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(tooltip)
        };
        let hover_actions = h_flex()
            .absolute()
            .right_2()
            .top_1()
            .gap_1()
            .px_1()
            .rounded(theme::RADIUS)
            .bg(if selected {
                colors.selected
            } else {
                colors.hover
            })
            .opacity(0.)
            .group_hover(group.clone(), |a| a.opacity(1.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                action(
                    "pin",
                    if pinned {
                        IconName::PinOff
                    } else {
                        IconName::Pin
                    },
                    if pinned { "取消钉住" } else { "钉住" },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.agent_history_op(
                        if pinned {
                            HistoryOp::Unpin(id)
                        } else {
                            HistoryOp::Pin(id)
                        },
                        window,
                        cx,
                    )
                })),
            )
            .child(
                action("rename", IconName::Pencil, "重命名").on_click(cx.listener({
                    let title = title.clone();
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.agent_start_rename(id, title.clone(), window, cx)
                    }
                })),
            )
            .child(
                action(
                    "archive",
                    if s.archived {
                        IconName::ArchiveRestore
                    } else {
                        IconName::Archive
                    },
                    if s.archived { "取消归档" } else { "归档" },
                )
                .on_click(cx.listener({
                    let archived = s.archived;
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.agent_history_op(HistoryOp::Archive(id, !archived), window, cx)
                    }
                })),
            )
            .child(
                action("delete", IconName::Trash, "永久删除").on_click(cx.listener({
                    let title = title.clone();
                    move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.agent_history_op(HistoryOp::Delete(id, title.clone()), window, cx)
                    }
                })),
            );
        let glyph_with_pip = div()
            .relative()
            .flex_shrink_0()
            .child(glyph_tile(glyph, theme::AGENT_GLYPH, colors))
            .when(
                matches!(
                    status,
                    RowStatus::Awaiting | RowStatus::Unread | RowStatus::Error
                ),
                |g| {
                    g.child(
                        div()
                            .absolute()
                            .right(theme::BADGE_OFFSET)
                            .bottom(theme::BADGE_OFFSET)
                            .child(status_mark(status, self.agent.spin, colors)),
                    )
                },
            );
        let title_el: AnyElement = match renaming {
            Some(input) => div()
                .flex_1()
                .min_w_0()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(Input::new(&input).xsmall())
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(theme::TEXT_CAPTION)
                .text_color(fg)
                .when(unread, |t| t.font_weight(FontWeight::SEMIBOLD))
                .child(title.clone())
                .into_any_element(),
        };
        let meta = h_flex()
            .h(theme::SCM_DETAIL_LINE)
            .gap_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(theme::TEXT_SECTION)
            .text_color(muted)
            .when(other_workspace, |m| {
                m.child(
                    div()
                        .px_1()
                        .rounded(theme::RADIUS)
                        .bg(colors.keycap)
                        .text_color(if selected { fg } else { colors.foreground })
                        .child(workspace_label(&s.workspace_root, cx)),
                )
            })
            .when_some(s.branch.clone(), |m, branch| {
                m.child(Icon::new(IconName::GitBranch).size(theme::SMALL_ICON_SIZE))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(branch),
                    )
            })
            .when(s.lines_added > 0, |m| {
                m.child(
                    div()
                        .text_color(if selected { fg } else { colors.added })
                        .child(format!("+{}", s.lines_added)),
                )
            })
            .when(s.lines_removed > 0, |m| {
                m.child(
                    div()
                        .text_color(if selected { fg } else { colors.deleted })
                        .child(format!("−{}", s.lines_removed)),
                )
            })
            .when(unread, |m| m.child("· 完成，未读"))
            .when(status == RowStatus::Error, |m| m.child("· 出错"));
        // Rows without anything for the second line stay one line tall.
        let two_line = !compact
            && (other_workspace
                || s.branch.is_some()
                || s.lines_added > 0
                || s.lines_removed > 0
                || unread
                || status == RowStatus::Error);
        let row = h_flex()
            .id(("agent-session", id.0 as u64))
            .group(group.clone())
            .relative()
            .mx_1()
            .px_2()
            .gap_2()
            .rounded(theme::RADIUS_LARGE)
            .cursor_pointer()
            .map(|row| {
                if two_line {
                    row.h(theme::AGENT_HISTORY_ROW).items_start().pt_2()
                } else {
                    row.h(theme::AGENT_HISTORY_ROW_COMPACT)
                }
            })
            .map(|row| {
                if selected {
                    row.bg(colors.selected)
                } else {
                    row.hover(|row| row.bg(colors.hover))
                }
            })
            .child(glyph_with_pip)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(h_flex().gap_2().child(title_el).child(right))
                    .when(two_line, |c| c.child(meta)),
            )
            .child(hover_actions)
            .on_click(
                cx.listener(move |this, _, window, cx| this.agent_open_stored(id, window, cx)),
            );
        // Pinned rows reorder by drag; dropping any row on a pinned row pins it there.
        let pinned_ids: Vec<i64> = self
            .agent
            .history
            .rows
            .iter()
            .filter(|r| r.pinned())
            .map(|r| r.id.0)
            .collect();
        let row = row
            .on_drag(
                DraggedSession {
                    id,
                    title: title.clone(),
                },
                |dragged, _, _, cx| {
                    cx.new(|_| DragGhost {
                        title: dragged.title.clone(),
                    })
                },
            )
            .when(pinned, |row| {
                row.drag_over::<DraggedSession>(move |style, _, _, _| {
                    style.border_t_2().border_color(colors.accent)
                })
                .on_drop(cx.listener(
                    move |this, dragged: &DraggedSession, window, cx| {
                        if let Some(before) = agent_model::pin_drop(&pinned_ids, dragged.id.0, id.0)
                        {
                            this.agent_history_op(
                                HistoryOp::MovePin(dragged.id, before.map(SessionId)),
                                window,
                                cx,
                            );
                        }
                    },
                ))
            });
        div().w_full().pb_1().child(row).into_any_element()
    }
}

fn history_renaming(this: &Workbench, id: SessionId) -> Option<Entity<InputState>> {
    this.agent
        .history
        .renaming
        .as_ref()
        .filter(|(renaming, _, _)| *renaming == id)
        .map(|(_, input, _)| input.clone())
}
