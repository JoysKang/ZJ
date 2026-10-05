//! The agent panel on the right of the workbench (design 01-A / 05-C): header, the live
//! strip of active sessions (direction B, only with two or more), the thread, the changed
//! files card and the composer. Narrower than `AGENT_COMPACT_WIDTH` it switches to the
//! compact density (direction C). Everything here only reads state prepared elsewhere.
//! This file has the shared pieces, the header, the strip and the switcher; `agent_panel/`
//! has the thread rows, the cards (tool, plan, login, permission), the changed-files card,
//! the composer and the settings view.

use super::agent::{self, AgentView, LiveSession, PermissionChoice, SPIN_FRAMES, agent_name};
use super::*;
use crate::agent_model::{self, Attachment, RowStatus};
use crate::markdown::{Block, Inline, Span};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Icon, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Textarea,
        menu::{DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
};
mod cards;
mod changes;
mod composer;
mod rows;
mod settings;

use workspace_editor_agent::{
    Glyph, PermissionKind, PlanStatus, ToolKind, ToolStatus,
    thread::{Item, LoginCard, LoginState, PermissionCard, PermissionState, ToolCard},
};

/// The agent's monochrome Lucide glyph on its tinted tile.
pub(super) fn glyph_tile(glyph: Glyph, size: Pixels, colors: theme::Colors) -> Div {
    let (fg, bg) = colors.glyphs[agent_model::glyph_index(glyph)];
    let icon = match glyph {
        Glyph::Claude => IconName::Asterisk,
        Glyph::Codex => IconName::SquareTerminal,
        Glyph::Gemini => IconName::Sparkle,
        Glyph::DeepSeek => IconName::Fish,
        Glyph::Generic => IconName::Bot,
    };
    div()
        .size(size)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::AGENT_GLYPH_RADIUS)
        .bg(bg)
        .child(Icon::new(icon).size(theme::AGENT_GLYPH_ICON).text_color(fg))
}

/// The loader icon at one of `SPIN_FRAMES` angles (the frame advances on a timer only while
/// the panel is visible).
pub(super) fn spinner(frame: usize, colors: theme::Colors) -> Icon {
    Icon::new(IconName::LoaderCircle)
        .size(theme::SMALL_ICON_SIZE)
        .text_color(colors.running)
        .rotate(radians(
            std::f32::consts::TAU * frame as f32 / SPIN_FRAMES as f32,
        ))
}

/// One status mark: spinner, yellow dot with halo, blue dot, red dot, or nothing.
pub(super) fn status_mark(status: RowStatus, frame: usize, colors: theme::Colors) -> AnyElement {
    let dot = |color: Hsla| {
        div()
            .size(theme::STATUS_DOT)
            .rounded_full()
            .flex_shrink_0()
            .bg(color)
    };
    match status {
        RowStatus::Running => spinner(frame, colors).into_any_element(),
        RowStatus::Awaiting => div()
            .size(theme::STATUS_HALO)
            .flex_shrink_0()
            .rounded_full()
            .bg(colors.attention_bg)
            .flex()
            .items_center()
            .justify_center()
            .child(dot(colors.attention))
            .into_any_element(),
        RowStatus::Unread => dot(colors.unread).into_any_element(),
        RowStatus::Error => dot(colors.deleted).into_any_element(),
        RowStatus::None => div().into_any_element(),
    }
}

/// Text runs for inline markup: code in the mono font on a key-cap tint, bold, italic, links.
fn inline_runs(
    inline: &Inline,
    base: &Font,
    mono: &Font,
    color: Hsla,
    colors: theme::Colors,
) -> Vec<TextRun> {
    let run = |len: usize, font: Font, color: Hsla, background: Option<Hsla>| TextRun {
        len,
        font,
        color,
        background_color: background,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    let mut at = 0;
    for (range, span) in &inline.spans {
        if range.start > at {
            runs.push(run(range.start - at, base.clone(), color, None));
        }
        let len = range.end - range.start;
        runs.push(match span {
            Span::Code => run(len, mono.clone(), colors.foreground, Some(colors.keycap)),
            Span::Bold => run(
                len,
                Font {
                    weight: FontWeight::SEMIBOLD,
                    ..base.clone()
                },
                color,
                None,
            ),
            Span::Italic => run(
                len,
                Font {
                    style: FontStyle::Italic,
                    ..base.clone()
                },
                color,
                None,
            ),
            Span::Link => TextRun {
                underline: Some(UnderlineStyle {
                    thickness: theme::INDICATOR,
                    color: Some(colors.accent),
                    wavy: false,
                }),
                ..run(len, base.clone(), colors.accent, None)
            },
        });
        at = range.end;
    }
    if inline.text.len() > at {
        runs.push(run(inline.text.len() - at, base.clone(), color, None));
    }
    runs
}

fn styled(
    inline: &Inline,
    base: &Font,
    mono: &Font,
    color: Hsla,
    colors: theme::Colors,
) -> StyledText {
    StyledText::new(SharedString::from(inline.text.clone()))
        .with_runs(inline_runs(inline, base, mono, color, colors))
}

struct Fonts {
    base: Font,
    mono: Font,
}

impl Workbench {
    fn agent_fonts(&self, cx: &App) -> Fonts {
        let theme = gpui_kit::component::Theme::global(cx);
        Fonts {
            base: font(theme.font_family.clone()),
            mono: font(theme.mono_font_family.clone()),
        }
    }

    // ----- the panel ----------------------------------------------------------------------

    pub(super) fn render_agent_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let body = match self.agent.view {
            AgentView::Thread => self.render_agent_thread_view(cx),
            AgentView::History => self.render_agent_history(cx),
            AgentView::Settings => self.render_agent_settings(cx),
        };
        v_flex()
            .id("agent-panel")
            .key_context("AgentPanel")
            .size_full()
            .min_w_0()
            .relative()
            .bg(colors.panel)
            .border_l_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_BODY)
            .text_color(colors.foreground)
            .on_action(cx.listener(|this, _: &agent::NewAgentSession, window, cx| {
                this.agent_new_session(None, window, cx)
            }))
            .child(self.render_agent_header(cx))
            .child(body)
            .when(self.agent.switcher_open, |panel| {
                panel.child(self.render_agent_switcher(cx))
            })
            .into_any_element()
    }

    fn render_agent_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let compact = self.agent.compact();
        let icon_button = |id: &'static str, icon: IconName, tooltip: &'static str| {
            Button::new(id).ghost().xsmall().icon(icon).tooltip(tooltip)
        };
        let more = {
            let weak = cx.weak_entity();
            Button::new("agent-more")
                .ghost()
                .xsmall()
                .icon(IconName::Ellipsis)
                .tooltip("更多")
                .dropdown_menu(move |menu, _, _| {
                    let settings = weak.clone();
                    let close = weak.clone();
                    let search = weak.clone();
                    menu.item(
                        PopupMenuItem::new("搜索会话 ⌘J").on_click(move |_, window, cx| {
                            let _ =
                                search.update(cx, |this, cx| this.agent_open_search(window, cx));
                        }),
                    )
                    .item(
                        PopupMenuItem::new("Agent 设置").on_click(move |_, window, cx| {
                            let _ = settings.update(cx, |this, cx| {
                                this.agent_show(AgentView::Settings, window, cx)
                            });
                        }),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("关闭面板 ⌥⌘B").on_click(move |_, window, cx| {
                            let _ = close
                                .update(cx, |this, cx| this.set_agent_panel(false, window, cx));
                        }),
                    )
                })
        };
        let new_session = icon_button("agent-new", IconName::SquarePen, "新会话（⌘N）")
            .on_click(cx.listener(|this, _, window, cx| this.agent_new_session(None, window, cx)));
        let bar = h_flex()
            .h(theme::TAB_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .pl_3()
            .pr_2()
            .gap_2()
            .border_b_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION);
        let bar = match self.agent.view {
            AgentView::Thread => {
                let session = self.agent.current();
                let glyph = session.map_or(Glyph::Generic, |s| s.preset.glyph);
                let title = session.map_or_else(|| "新会话".to_string(), LiveSession::title);
                let status = session.map_or(RowStatus::None, LiveSession::row_status);
                bar.child(glyph_tile(glyph, theme::AGENT_GLYPH, colors))
                    .child(
                        h_flex()
                            .id("agent-title")
                            .min_w_0()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .when(compact, |title| {
                                title
                                    .cursor_pointer()
                                    .child(
                                        Icon::new(if self.agent.switcher_open {
                                            IconName::ChevronUp
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .size(theme::SMALL_ICON_SIZE)
                                        .text_color(colors.muted),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.agent.switcher_open = !this.agent.switcher_open;
                                        if this.agent.switcher_open {
                                            this.agent_reload_history(window, cx);
                                        }
                                        cx.notify();
                                    }))
                            }),
                    )
                    .when(!compact, |bar| {
                        bar.when(status != RowStatus::None, |bar| {
                            bar.child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap_1()
                                    .child(status_mark(status, self.agent.spin, colors))
                                    .when_some(status.label(), |mark, label| {
                                        mark.child(
                                            div()
                                                .text_color(match status {
                                                    RowStatus::Awaiting => colors.attention,
                                                    RowStatus::Error => colors.deleted,
                                                    RowStatus::Unread => colors.unread,
                                                    _ => colors.muted,
                                                })
                                                .child(label),
                                        )
                                    }),
                            )
                        })
                        .child(
                            icon_button("agent-history", IconName::RotateCcwClock, "会话列表")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.agent_view_history(window, cx)
                                })),
                        )
                    })
                    .child(new_session)
                    .child(more)
            }
            AgentView::History => {
                let (here, all, _) = self.agent.history.counts;
                bar.child(
                    icon_button("agent-back", IconName::ArrowLeft, "回到会话").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.agent_show(AgentView::Thread, window, cx)
                        }),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("会话"),
                )
                .when(!compact, |bar| {
                    bar.child(
                        div()
                            .text_size(theme::TEXT_SECTION)
                            .text_color(colors.muted)
                            .child(format!(
                                "{} {here} · 全部 {all}",
                                self.agent_scope_label(false, cx)
                            )),
                    )
                })
                .child(new_session)
                .child(more)
            }
            AgentView::Settings => bar
                .child(
                    icon_button("agent-back", IconName::ArrowLeft, "回到会话").on_click(
                        cx.listener(|this, _, window, cx| {
                            this.agent_show(AgentView::Thread, window, cx)
                        }),
                    ),
                )
                .child(
                    div()
                        .flex_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Agent 设置"),
                ),
        };
        bar.into_any_element()
    }

    // ----- thread view --------------------------------------------------------------------

    fn render_agent_thread_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let compact = self.agent.compact();
        let active: Vec<&LiveSession> = self.agent.active_sessions().collect();
        let strip = agent_model::show_live_strip(active.len()) && !compact;
        let session = self.agent.current();
        let info = session.map(|s| {
            let now = workspace_editor_agent_history::now_ms();
            let offset = agent_model::local_offset(now);
            let mut parts = vec![s.preset.display_name.clone()];
            if let Some(branch) = s.branch.clone().or_else(|| self.active_branch()) {
                parts.push(branch);
            }
            parts.push(format!(
                "{} 开始",
                agent_model::relative_time(s.started_at, now, offset)
            ));
            parts.join(" · ")
        });
        let empty = session.is_none_or(|s| s.thread.items.is_empty());
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .when(strip, |view| {
                view.child(self.render_agent_strip(&active, cx))
            })
            .when(!compact, |view| {
                view.when_some(info, |view, info| {
                    view.child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .pt_2()
                            .text_center()
                            .text_size(theme::TEXT_SECTION)
                            .text_color(colors.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(info),
                    )
                })
            })
            .child(if empty {
                self.render_agent_empty(cx)
            } else {
                list(
                    self.agent.thread_list.clone(),
                    cx.processor(|this, index: usize, window, cx| {
                        this.render_agent_row(index, window, cx)
                    }),
                )
                .flex_1()
                .w_full()
                .into_any_element()
            })
            .children(self.render_agent_changes(cx))
            .child(self.render_agent_composer(cx))
            .into_any_element()
    }

    fn render_agent_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let (glyph, name) = self
            .agent
            .current()
            .map_or((Glyph::Generic, "Agent".to_string()), |s| {
                (s.preset.glyph, s.preset.display_name.clone())
            });
        let hint = |keys: &'static str, text: &'static str| {
            h_flex()
                .gap_2()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(
                    div()
                        .px_1()
                        .rounded(theme::RADIUS)
                        .bg(colors.keycap)
                        .text_color(colors.foreground)
                        .text_size(theme::TEXT_SECTION)
                        .child(keys),
                )
                .child(text)
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap_3()
            .px_4()
            .child(glyph_tile(glyph, theme::AGENT_GLYPH * 2., colors))
            .child(
                div()
                    .text_color(colors.foreground)
                    .child(format!("向 {name} 提问")),
            )
            .when(self.root.is_none(), |view| {
                view.child(
                    div()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child("没有打开文件夹：对话在默认工作区里进行"),
                )
            })
            .child(
                v_flex()
                    .gap_2()
                    .child(hint("⌘L", "把编辑器里的选区加入对话"))
                    .child(hint("@", "引用工作区里的文件"))
                    .child(hint("⌘J", "搜索全部会话"))
                    .child(hint("⇧⏎", "换行；⏎ 发送")),
            )
            .into_any_element()
    }

    /// Direction B: active sessions stacked above the thread.
    fn render_agent_strip(&self, active: &[&LiveSession], cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let collapsed = self.agent.strip_collapsed;
        let waiting = active
            .iter()
            .filter(|s| s.row_status() == RowStatus::Awaiting)
            .count();
        let header = h_flex()
            .id("agent-strip-toggle")
            .h(theme::AGENT_GROUP_HEADER)
            .px_3()
            .gap_1()
            .cursor_pointer()
            .text_size(theme::TEXT_SECTION)
            .text_color(colors.muted)
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(theme::SMALL_ICON_SIZE),
            )
            .child(format!(
                "{} 个活动会话{}",
                active.len(),
                if waiting > 0 {
                    format!(" · {waiting} 待批准")
                } else {
                    String::new()
                }
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.agent.strip_collapsed = !this.agent.strip_collapsed;
                cx.notify();
            }));
        let rows = active.iter().map(|s| {
            let key = s.key;
            let selected = self.agent.current == Some(key);
            let status = s.row_status();
            h_flex()
                .id(("agent-strip", key))
                .h(theme::AGENT_STRIP_ROW)
                .mx_1()
                .px_2()
                .gap_2()
                .rounded(theme::RADIUS_LARGE)
                .cursor_pointer()
                .text_size(theme::TEXT_CAPTION)
                .map(|row| {
                    if selected {
                        row.bg(colors.selected).text_color(colors.selected_fg)
                    } else {
                        row.hover(|row| row.bg(colors.hover))
                    }
                })
                .child(glyph_tile(s.preset.glyph, theme::AGENT_GLYPH_SMALL, colors))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(s.thread.unread, |t| t.font_weight(FontWeight::SEMIBOLD))
                        .child(s.title()),
                )
                .when_some(s.branch.clone(), |row, branch| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .max_w(theme::AGENT_PLAN_BAR_WIDTH)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(theme::TEXT_SECTION)
                            .text_color(if selected {
                                colors.selected_fg
                            } else {
                                colors.muted
                            })
                            .child(branch),
                    )
                })
                .child(match status {
                    RowStatus::Awaiting => h_flex()
                        .h(theme::AGENT_CHIP)
                        .px_1()
                        .gap_1()
                        .rounded(theme::RADIUS)
                        .border_1()
                        .border_color(colors.attention_border)
                        .bg(colors.attention_bg)
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.attention)
                        .child(Icon::new(IconName::ShieldAlert).size(theme::SMALL_ICON_SIZE))
                        .child("待批准")
                        .into_any_element(),
                    other => status_mark(other, self.agent.spin, colors),
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.agent_select(key, window, cx)),
                )
        });
        v_flex()
            .w_full()
            .flex_shrink_0()
            .pb_1()
            .border_b_1()
            .border_color(colors.border)
            .child(header)
            .when(!collapsed, |strip| strip.children(rows))
            .into_any_element()
    }

    /// Compact density: the session switcher under the title.
    fn render_agent_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let now = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now);
        let live: Vec<&LiveSession> = self.agent.sessions.iter().rev().take(6).collect();
        let live_ids: HashSet<_> = live.iter().filter_map(|s| s.db).collect();
        let stored: Vec<_> = self
            .agent
            .history
            .rows
            .iter()
            .filter(|s| !live_ids.contains(&s.id))
            .take(8)
            .cloned()
            .collect();
        let row =
            |id: ElementId, glyph: Glyph, title: String, right: AnyElement, selected: bool| {
                h_flex()
                    .id(id)
                    .h(theme::AGENT_STRIP_ROW)
                    .mx_1()
                    .px_2()
                    .gap_2()
                    .rounded(theme::RADIUS)
                    .cursor_pointer()
                    .text_size(theme::TEXT_CAPTION)
                    .map(|row| {
                        if selected {
                            row.bg(colors.selected).text_color(colors.selected_fg)
                        } else {
                            row.hover(|row| row.bg(colors.hover))
                        }
                    })
                    .child(glyph_tile(glyph, theme::AGENT_GLYPH_SMALL, colors))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(title),
                    )
                    .child(right)
            };
        let section = |label: &'static str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_size(theme::TEXT_SECTION)
                .text_color(colors.muted)
                .child(label)
        };
        let total = self.agent.history.counts.1;
        v_flex()
            .id("agent-switcher")
            .absolute()
            .top(theme::TAB_HEIGHT)
            .left_1()
            .right_1()
            .py_1()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.strong_border)
            .bg(colors.panel)
            .shadow_lg()
            .occlude()
            .child(
                h_flex()
                    .id("agent-switcher-search")
                    .mx_2()
                    .my_1()
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
                    .child(div().flex_1().child("搜索会话…"))
                    .child("⌘J")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.agent.switcher_open = false;
                        this.agent_open_search(window, cx)
                    })),
            )
            .child(section("当前窗口"))
            .children(live.into_iter().map(|s| {
                let key = s.key;
                row(
                    ("agent-switch-live", key).into(),
                    s.preset.glyph,
                    s.title(),
                    status_mark(s.row_status(), self.agent.spin, colors),
                    self.agent.current == Some(key),
                )
                .on_click(
                    cx.listener(move |this, _, window, cx| this.agent_select(key, window, cx)),
                )
            }))
            .when(!stored.is_empty(), |list| list.child(section("最近")))
            .children(stored.into_iter().map(|s| {
                let id = s.id;
                row(
                    ("agent-switch-stored", id.0 as u64).into(),
                    agent::glyph_for(&self.agent.presets, &s.agent_id),
                    s.title.clone(),
                    div()
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child(agent_model::row_time(s.updated_at, now, offset))
                        .into_any_element(),
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.agent.switcher_open = false;
                    this.agent_open_stored(id, window, cx)
                }))
            }))
            .child(
                h_flex()
                    .mt_1()
                    .px_3()
                    .h(theme::AGENT_STRIP_ROW)
                    .gap_2()
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(
                        h_flex()
                            .id("agent-switch-all")
                            .gap_1()
                            .flex_1()
                            .cursor_pointer()
                            .child(Icon::new(IconName::RotateCcwClock).size(theme::SMALL_ICON_SIZE))
                            .child(format!("全部会话 {total}"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_show(AgentView::History, window, cx)
                            })),
                    )
                    .child(
                        h_flex()
                            .id("agent-switch-new")
                            .gap_1()
                            .cursor_pointer()
                            .child(Icon::new(IconName::SquarePen).size(theme::SMALL_ICON_SIZE))
                            .child("新会话 ⌘N")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_new_session(None, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn chip(label: &str, icon: Option<IconName>, colors: theme::Colors) -> Div {
    let file_icon = icon
        .is_none()
        .then(|| file_icons::icon(file_icons::for_file(label)));
    h_flex()
        .h(theme::AGENT_CHIP)
        .px_1()
        .gap_1()
        .rounded(theme::RADIUS)
        .bg(colors.keycap)
        .text_size(theme::TEXT_SECTION)
        .text_color(colors.foreground)
        .whitespace_nowrap()
        .children(file_icon)
        .children(icon.map(|i| {
            Icon::new(i)
                .size(theme::SMALL_ICON_SIZE)
                .text_color(colors.muted)
        }))
        .child(label.to_string())
}

/// The context-usage ring: the track and the used arc, drawn with a path.
fn usage_ring(fraction: f32, colors: theme::Colors) -> impl IntoElement {
    let track = colors.strong_border;
    let fill = if fraction > 0.85 {
        colors.attention
    } else {
        colors.running
    };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let stroke = theme::AGENT_RING_STROKE;
            let radius = (bounds.size.width.min(bounds.size.height) - stroke) / 2.;
            let center = bounds.center();
            let point = |angle: f32| {
                gpui_kit::point(
                    center.x + radius * angle.sin(),
                    center.y - radius * angle.cos(),
                )
            };
            let radii = gpui_kit::point(radius, radius);
            let mut circle = PathBuilder::stroke(stroke);
            circle.move_to(point(0.));
            circle.arc_to(
                radii,
                Pixels::ZERO,
                false,
                true,
                point(std::f32::consts::PI),
            );
            circle.arc_to(radii, Pixels::ZERO, false, true, point(0.));
            if let Ok(path) = circle.build() {
                window.paint_path(path, track);
            }
            if fraction > 0. {
                let end = std::f32::consts::TAU * fraction.min(0.999);
                let mut arc = PathBuilder::stroke(stroke);
                arc.move_to(point(0.));
                arc.arc_to(
                    radii,
                    Pixels::ZERO,
                    end > std::f32::consts::PI,
                    true,
                    point(end),
                );
                if let Ok(path) = arc.build() {
                    window.paint_path(path, fill);
                }
            }
        },
    )
    .size(theme::AGENT_RING)
    .flex_shrink_0()
}
