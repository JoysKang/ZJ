//! The agent panel on the right of the workbench (design 01-A / 05-C): header, the live
//! strip of active sessions (direction B, only with two or more), the thread, the changed
//! files card and the composer. Narrower than `AGENT_COMPACT_WIDTH` it switches to the
//! compact density (direction C). Everything here only reads state prepared elsewhere.

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
use workspace_editor_agent::{
    Glyph, PermissionKind, PlanStatus, ToolKind, ToolStatus,
    thread::{
        ChangeOrigin, Item, LoginCard, LoginState, PermissionCard, PermissionState, ToolCard,
    },
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

impl Prototype {
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
                        bar.when_some(status.label(), |bar, label| {
                            bar.child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap_1()
                                    .child(status_mark(status, self.agent.spin, colors))
                                    .child(
                                        div()
                                            .text_color(match status {
                                                RowStatus::Awaiting => colors.attention,
                                                RowStatus::Error => colors.deleted,
                                                RowStatus::Unread => colors.unread,
                                                _ => colors.muted,
                                            })
                                            .child(label),
                                    ),
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
                            .child(format!("本工作区 {here} · 全部 {all}")),
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
                        .child("先打开一个文件夹：Agent 在工作区里读写文件"),
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

    // ----- thread rows --------------------------------------------------------------------

    fn render_agent_row(
        &self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(session) = self.agent.current() else {
            return div().into_any_element();
        };
        let older = self.agent.older_row();
        if older && index == 0 {
            return div()
                .w_full()
                .px_3()
                .pt_2()
                .flex()
                .justify_center()
                .child(
                    Button::new("agent-older")
                        .ghost()
                        .xsmall()
                        .label(if session.oldest_seq.is_some() {
                            "加载更早的消息"
                        } else {
                            "更早的消息已移出内存 · 从历史重新打开"
                        })
                        .on_click(
                            cx.listener(|this, _, window, cx| this.agent_load_older(window, cx)),
                        ),
                )
                .into_any_element();
        }
        let i = index - usize::from(older);
        let Some(item) = session.thread.items.get(i) else {
            return div().into_any_element();
        };
        let compact = self.agent.compact();
        let fonts = self.agent_fonts(cx);
        let _ = window;
        let is_tool = |j: Option<usize>| {
            j.and_then(|j| session.thread.items.get(j))
                .is_some_and(|item| matches!(item, Item::Tool(_)))
        };
        let prev_tool = is_tool(i.checked_sub(1));
        let next_tool = is_tool(Some(i + 1));
        let content = match item {
            Item::User { text, attachments } => {
                self.render_user_message(text, attachments, compact, cx)
            }
            Item::Agent { .. } => {
                let blocks = session.md.get(&(session.thread.dropped + i)).cloned();
                self.render_markdown(blocks.as_deref().map_or(&[][..], |b| b), &fonts, cx)
            }
            Item::Thought { text, streaming } => {
                self.render_thought(session.key, i, text, *streaming, cx)
            }
            Item::Tool(card) => {
                self.render_tool(session, card, prev_tool, next_tool, compact, &fonts, cx)
            }
            Item::Plan(entries) => self.render_plan(session.key, entries, compact, cx),
            Item::Permission(card) => self.render_permission(session, card, compact, &fonts, cx),
            Item::Login(card) => self.render_login(session, i, card, cx),
            Item::Notice { text, error } => h_flex()
                .gap_2()
                .items_start()
                .text_size(theme::TEXT_CAPTION)
                .text_color(if *error { colors.deleted } else { colors.muted })
                .child(
                    Icon::new(if *error {
                        IconName::TriangleAlert
                    } else {
                        IconName::Info
                    })
                    .size(theme::SMALL_ICON_SIZE),
                )
                .child(div().flex_1().min_w_0().child(text.clone()))
                .into_any_element(),
        };
        // Consecutive tool calls share one card: no gap between them.
        let gap = !(matches!(item, Item::Tool(_)) && next_tool);
        div()
            .w_full()
            .px_3()
            .when(index == 0, |row| row.pt_3())
            .when(gap, |row| row.pb_3())
            .child(content)
            .into_any_element()
    }

    fn render_user_message(
        &self,
        text: &str,
        attachments: &[String],
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        v_flex()
            .w_full()
            .gap_1()
            .map(|bubble| {
                if compact {
                    bubble.pl_3().border_l_2().border_color(colors.accent)
                } else {
                    bubble
                        .px_3()
                        .py_2()
                        .rounded(theme::RADIUS_LARGE)
                        .bg(colors.hover)
                }
            })
            .when(!attachments.is_empty(), |bubble| {
                bubble.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(attachments.iter().map(|label| chip(label, None, colors))),
                )
            })
            .child(div().line_height(theme::AGENT_LINE).child(text.to_string()))
            .into_any_element()
    }

    fn render_markdown(
        &self,
        blocks: &[Block],
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let mono_family = gpui_kit::component::Theme::global(cx)
            .mono_font_family
            .clone();
        v_flex()
            .w_full()
            .gap_2()
            .line_height(theme::AGENT_LINE)
            .children(blocks.iter().enumerate().map(|(i, block)| {
                match block {
                    Block::Paragraph(inline) => div()
                        .child(styled(
                            inline,
                            &fonts.base,
                            &fonts.mono,
                            colors.foreground,
                            colors,
                        ))
                        .into_any_element(),
                    Block::Heading(inline) => div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(styled(
                            inline,
                            &Font {
                                weight: FontWeight::SEMIBOLD,
                                ..fonts.base.clone()
                            },
                            &fonts.mono,
                            colors.foreground,
                            colors,
                        ))
                        .into_any_element(),
                    Block::Item {
                        depth,
                        marker,
                        text,
                    } => h_flex()
                        .items_start()
                        .pl(theme::TREE_STEP * 2. * *depth as f32)
                        .child(
                            div()
                                .w(theme::ICON_SIZE + theme::TREE_STEP)
                                .flex_shrink_0()
                                .text_color(colors.muted)
                                .child(marker.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(styled(
                            text,
                            &fonts.base,
                            &fonts.mono,
                            colors.foreground,
                            colors,
                        )))
                        .into_any_element(),
                    Block::Quote(inline) => div()
                        .pl_3()
                        .border_l_2()
                        .border_color(colors.strong_border)
                        .child(styled(
                            inline,
                            &fonts.base,
                            &fonts.mono,
                            colors.muted,
                            colors,
                        ))
                        .into_any_element(),
                    Block::Rule => div()
                        .h(theme::INDICATOR)
                        .w_full()
                        .bg(colors.border)
                        .into_any_element(),
                    Block::Code { text, runs, .. } => div()
                        .id(("agent-code", i))
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded(theme::RADIUS)
                        .bg(colors.editor)
                        .border_1()
                        .border_color(colors.card_border)
                        .overflow_x_scroll()
                        .font_family(mono_family.clone())
                        .text_size(theme::TEXT_SECTION)
                        .line_height(theme::SCM_DETAIL_LINE)
                        .text_color(colors.code)
                        .whitespace_nowrap()
                        .child(
                            StyledText::new(SharedString::from(text.clone()))
                                .with_highlights(runs.iter().cloned()),
                        )
                        .into_any_element(),
                }
            }))
            .into_any_element()
    }

    fn render_thought(
        &self,
        key: u64,
        index: usize,
        text: &str,
        streaming: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let expanded = self.agent.expanded_thoughts.contains(&(key, index));
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .id(("agent-thought", index))
                    .gap_1()
                    .cursor_pointer()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(Icon::new(IconName::Brain).size(theme::SMALL_ICON_SIZE))
                    .child(if streaming {
                        "正在思考…"
                    } else {
                        "思考过程"
                    })
                    .child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(theme::SMALL_ICON_SIZE),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.agent.expanded_thoughts.remove(&(key, index)) {
                            this.agent.expanded_thoughts.insert((key, index));
                        }
                        this.agent.thread_list.remeasure();
                        cx.notify();
                    })),
            )
            .when(expanded, |thought| {
                thought.child(
                    div()
                        .pl_3()
                        .border_l_2()
                        .border_color(colors.border)
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(text.to_string()),
                )
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_tool(
        &self,
        session: &LiveSession,
        card: &ToolCard,
        prev_tool: bool,
        next_tool: bool,
        compact: bool,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let call = &card.call;
        let key = session.key;
        let id = call.id.clone();
        let expanded = self.agent.expanded_tools.contains(&(key, id.clone()));
        let (verb, detail) = agent_model::tool_summary(call);
        let output = agent_model::tool_output(call);
        let has_output = !output.is_empty();
        let icon = match call.kind {
            ToolKind::Read => IconName::FileText,
            ToolKind::Edit => {
                if card.removed == 0 && card.added > 0 {
                    IconName::FilePlus
                } else {
                    IconName::FilePen
                }
            }
            ToolKind::Delete => IconName::Delete,
            ToolKind::Move => IconName::ArrowRight,
            ToolKind::Search => IconName::Search,
            ToolKind::Execute => IconName::SquareTerminal,
            ToolKind::Think => IconName::Brain,
            ToolKind::Fetch => IconName::Globe,
            ToolKind::SwitchMode => IconName::ShieldCheck,
            ToolKind::Other => IconName::Bot,
        };
        let status: AnyElement = match call.status {
            ToolStatus::Completed => Icon::new(IconName::Check)
                .size(theme::SMALL_ICON_SIZE)
                .text_color(colors.added)
                .into_any_element(),
            ToolStatus::Failed => Icon::new(IconName::CircleX)
                .size(theme::SMALL_ICON_SIZE)
                .text_color(colors.deleted)
                .into_any_element(),
            ToolStatus::Pending | ToolStatus::InProgress => {
                spinner(self.agent.spin, colors).into_any_element()
            }
        };
        let edit_path = (call.kind == ToolKind::Edit)
            .then(|| call.locations.first().map(|l| l.path.clone()))
            .flatten()
            .filter(|p| session.thread.changed_files.contains_key(p));
        let mono = matches!(call.kind, ToolKind::Execute | ToolKind::Search);
        let row = h_flex()
            .id(SharedString::from(format!("agent-tool-{key}-{id}")))
            .h(theme::AGENT_TOOL_ROW)
            .w_full()
            .gap_2()
            .text_size(theme::TEXT_CAPTION)
            .when(!compact, |row| row.px_3())
            .when(has_output || edit_path.is_some(), |row| {
                row.cursor_pointer().hover(|row| row.bg(colors.hover))
            })
            .child(
                Icon::new(icon)
                    .size(theme::SMALL_ICON_SIZE)
                    .text_color(colors.muted),
            )
            .when(!verb.is_empty(), |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_color(colors.foreground)
                        .child(verb),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(colors.muted)
                    .when(mono, |d| d.font_family(fonts.mono.family.clone()))
                    .child(detail),
            )
            .when(card.added + card.removed > 0, |row| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_color(colors.added)
                        .child(format!("+{}", card.added)),
                )
                .when(card.removed > 0, |row| {
                    row.child(
                        div()
                            .flex_shrink_0()
                            .text_color(colors.deleted)
                            .child(format!("−{}", card.removed)),
                    )
                })
            })
            .child(status)
            .when(has_output || edit_path.is_some(), |row| {
                row.child(
                    Icon::new(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .size(theme::SMALL_ICON_SIZE)
                    .text_color(colors.muted),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(path) = edit_path.clone() {
                    this.agent_open_review(key, path, window, cx);
                    return;
                }
                if !this.agent.expanded_tools.remove(&(key, id.clone())) {
                    this.agent.expanded_tools.insert((key, id.clone()));
                }
                this.agent.thread_list.remeasure();
                cx.notify();
            }));
        let body = v_flex()
            .w_full()
            .child(row)
            .when(expanded && has_output, |body| {
                let text: String = output.lines().take(40).collect::<Vec<_>>().join("\n");
                body.child(
                    div()
                        .w_full()
                        .pl(theme::AGENT_FILE_INDENT + theme::TREE_STEP)
                        .pr_3()
                        .py_2()
                        .border_t_1()
                        .border_color(colors.card_border)
                        .bg(colors.panel)
                        .font_family(fonts.mono.family.clone())
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child(text),
                )
            });
        if compact {
            return body.into_any_element();
        }
        body.overflow_hidden()
            .bg(colors.card)
            .border_l_1()
            .border_r_1()
            .border_color(colors.card_border)
            .map(|card| {
                if prev_tool {
                    card.border_t_1()
                } else {
                    card.border_t_1().rounded_t(theme::RADIUS_LARGE)
                }
            })
            .when(!next_tool, |card| {
                card.border_b_1().rounded_b(theme::RADIUS_LARGE)
            })
            .into_any_element()
    }

    fn render_plan(
        &self,
        key: u64,
        entries: &[workspace_editor_agent::PlanEntry],
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let done = entries
            .iter()
            .filter(|e| e.status == PlanStatus::Completed)
            .count();
        let total = entries.len().max(1);
        let collapsed = compact || self.agent.collapsed_plans.contains(&key);
        let current = entries
            .iter()
            .find(|e| e.status == PlanStatus::InProgress)
            .map(|e| e.content.clone());
        let bar = div()
            .h(theme::AGENT_PLAN_BAR)
            .flex_1()
            .max_w(theme::AGENT_PLAN_BAR_WIDTH)
            .rounded_full()
            .bg(colors.strong_border)
            .child(
                div()
                    .h_full()
                    .w(relative(done as f32 / total as f32))
                    .rounded_full()
                    .bg(colors.accent),
            );
        let head = h_flex()
            .id(("agent-plan", key))
            .h(theme::AGENT_CARD_HEAD)
            .w_full()
            .gap_2()
            .when(!compact, |h| h.px_3())
            .cursor_pointer()
            .text_size(theme::TEXT_CAPTION)
            .child(
                Icon::new(IconName::ListTodo)
                    .size(theme::SMALL_ICON_SIZE)
                    .text_color(colors.muted),
            )
            .child("计划")
            .child(
                div()
                    .text_color(colors.muted)
                    .child(format!("{done} / {}", entries.len())),
            )
            .child(bar)
            .when(collapsed, |h| {
                h.when_some(current.clone(), |h, current| {
                    h.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(colors.muted)
                            .child(current),
                    )
                })
            })
            .when(!collapsed, |h| h.child(div().flex_1()))
            .child(
                Icon::new(if collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .size(theme::SMALL_ICON_SIZE)
                .text_color(colors.muted),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.agent.collapsed_plans.remove(&key) {
                    this.agent.collapsed_plans.insert(key);
                }
                this.agent.thread_list.remeasure();
                cx.notify();
            }));
        let spin = self.agent.spin;
        let items = entries.iter().map(|entry| {
            let (icon, color, text): (AnyElement, Hsla, Hsla) = match entry.status {
                PlanStatus::Completed => (
                    Icon::new(IconName::CircleCheck)
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(colors.added)
                        .into_any_element(),
                    colors.added,
                    colors.muted,
                ),
                PlanStatus::InProgress => (
                    spinner(spin, colors).into_any_element(),
                    colors.running,
                    colors.foreground,
                ),
                PlanStatus::Pending => (
                    Icon::new(IconName::Circle)
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(colors.muted)
                        .into_any_element(),
                    colors.muted,
                    colors.muted,
                ),
            };
            let _ = color;
            h_flex()
                .items_start()
                .gap_2()
                .py_1()
                .text_size(theme::TEXT_CAPTION)
                .child(div().pt(theme::ROW_INSET).child(icon))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(text)
                        .when(entry.status == PlanStatus::InProgress, |t| {
                            t.font_weight(FontWeight::MEDIUM)
                        })
                        .child(entry.content.clone()),
                )
        });
        v_flex()
            .w_full()
            .when(!compact, |card| {
                card.rounded(theme::RADIUS_LARGE)
                    .border_1()
                    .border_color(colors.card_border)
                    .bg(colors.card)
            })
            .child(head)
            .when(!collapsed, |card| {
                card.child(v_flex().px_3().pb_2().children(items))
            })
            .into_any_element()
    }

    fn render_login(
        &self,
        session: &LiveSession,
        index: usize,
        card: &LoginCard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let agent = session.preset.display_name.clone();
        if card.state != LoginState::Pending {
            let (icon, color, text) = match card.state {
                LoginState::Done => (
                    IconName::CircleCheck,
                    colors.added,
                    format!("已登录「{agent}」"),
                ),
                _ => (IconName::Ban, colors.muted, "登录已取消".to_string()),
            };
            return h_flex()
                .gap_2()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(
                    Icon::new(icon)
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(color),
                )
                .child(text)
                .into_any_element();
        }
        let key = session.key;
        let base = (index as u64) << 8;
        let buttons = card.methods.iter().enumerate().map(|(n, method)| {
            let id = method.id.clone();
            let button = Button::new(("agent-login", base | n as u64))
                .xsmall()
                .icon(if method.terminal {
                    IconName::SquareTerminal
                } else {
                    IconName::Globe
                })
                .label(method.name.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.agent_login(key, &id, cx)));
            let button = if n == 0 {
                button.primary()
            } else {
                button.outline()
            };
            match &method.description {
                Some(tip) => button.tooltip(tip.clone()),
                None => button,
            }
        });
        let has_terminal = card.methods.iter().any(|m| m.terminal);
        let hint = if has_terminal {
            "选择登录方式。终端方式会打开「终端」窗口，完成后点「已登录，重试」"
        } else {
            "选择登录方式，按提示在浏览器里完成；完成后会自动继续"
        };
        v_flex()
            .w_full()
            .gap_2()
            .p_3()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.attention_border)
            .bg(colors.card)
            .child(
                h_flex()
                    .gap_2()
                    .text_size(theme::TEXT_CAPTION)
                    .child(
                        Icon::new(IconName::ShieldAlert)
                            .size(theme::SMALL_ICON_SIZE)
                            .text_color(colors.attention),
                    )
                    .child(div().flex_1().child(if card.retried {
                        format!("{agent} 仍未登录")
                    } else {
                        format!("{agent} 需要登录")
                    })),
            )
            .child(
                div()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(hint),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(buttons)
                    .when(has_terminal, |row| {
                        row.child(
                            Button::new(("agent-login-retry", base))
                                .ghost()
                                .xsmall()
                                .icon(IconName::RefreshCw)
                                .label("已登录，重试")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.agent_retry_login(key, cx)
                                })),
                        )
                    }),
            )
            .into_any_element()
    }

    fn render_permission(
        &self,
        session: &LiveSession,
        card: &PermissionCard,
        compact: bool,
        fonts: &Fonts,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let request = &card.request;
        let kind = request.tool_call.kind;
        let command = workspace_editor_agent::thread::permission_command(request)
            .unwrap_or_else(|| "（Agent 没有说明要做什么）".into());
        let agent = session.preset.display_name.clone();
        match &card.state {
            PermissionState::Pending => {}
            state => {
                let (icon, color, text) = match state {
                    PermissionState::Answered(PermissionKind::RejectOnce, label)
                    | PermissionState::Answered(PermissionKind::RejectAlways, label) => {
                        (IconName::CircleX, colors.deleted, label.clone())
                    }
                    PermissionState::Answered(_, label) => {
                        (IconName::ShieldCheck, colors.added, label.clone())
                    }
                    PermissionState::Rule(rule) => (
                        IconName::ShieldCheck,
                        colors.added,
                        format!("按规则自动允许「{rule}」"),
                    ),
                    PermissionState::Cancelled => {
                        (IconName::Ban, colors.muted, "请求已取消".to_string())
                    }
                    PermissionState::Pending => unreachable!(),
                };
                return h_flex()
                    .gap_2()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(
                        Icon::new(icon)
                            .size(theme::SMALL_ICON_SIZE)
                            .text_color(color),
                    )
                    .child(div().flex_shrink_0().child(text))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .font_family(fonts.mono.family.clone())
                            .child(command),
                    )
                    .into_any_element();
            }
        }
        let key = session.key;
        let id = request.id;
        let prefix = (kind == Some(ToolKind::Execute))
            .then(|| workspace_editor_agent::thread::command_prefix(&command))
            .filter(|p| !p.is_empty());
        let heading = match kind {
            Some(ToolKind::Execute) => format!("{agent} 请求运行命令"),
            Some(ToolKind::Edit) | Some(ToolKind::Delete) | Some(ToolKind::Move) => {
                format!("{agent} 请求修改文件")
            }
            Some(ToolKind::Fetch) => format!("{agent} 请求访问网络"),
            _ => format!("{agent} 请求权限"),
        };
        let mode = session
            .thread
            .modes
            .as_ref()
            .and_then(|m| {
                m.available
                    .iter()
                    .find(|(id, _)| *id == m.current)
                    .map(|(_, name)| name.clone())
            })
            .unwrap_or_else(|| "询问模式".into());
        let keycap = |label: &'static str| {
            div()
                .px_1()
                .rounded(theme::RADIUS)
                .bg(colors.keycap)
                .text_size(theme::TEXT_BADGE)
                .text_color(colors.muted)
                .child(label)
        };
        let once = Button::new(("agent-allow-once", id))
            .primary()
            .xsmall()
            .label(if compact { "允许" } else { "允许一次" })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.agent_answer(key, id, PermissionChoice::Once, window, cx)
            }));
        let always = prefix.clone().map(|prefix| {
            let label = if compact {
                "始终".to_string()
            } else {
                format!("始终允许 {prefix}")
            };
            Button::new(("agent-allow-always", id))
                .outline()
                .xsmall()
                .label(label)
                .tooltip("只对这个工作区生效；保存在设置里，可在 Agent 设置中移除")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.agent_answer(
                        key,
                        id,
                        PermissionChoice::Always(prefix.clone()),
                        window,
                        cx,
                    )
                }))
        });
        let reject = Button::new(("agent-reject", id))
            .ghost()
            .xsmall()
            .label("拒绝")
            .on_click(cx.listener(move |this, _, window, cx| {
                this.agent_answer(key, id, PermissionChoice::Reject, window, cx)
            }));
        v_flex()
            .w_full()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.attention_border)
            .bg(colors.card)
            .overflow_hidden()
            .when(!compact, |card| {
                card.child(
                    h_flex()
                        .px_3()
                        .pt_2()
                        .gap_2()
                        .text_size(theme::TEXT_CAPTION)
                        .child(
                            Icon::new(IconName::ShieldAlert)
                                .size(theme::SMALL_ICON_SIZE)
                                .text_color(colors.attention),
                        )
                        .child(div().flex_1().child(heading))
                        .child(
                            div()
                                .text_size(theme::TEXT_SECTION)
                                .text_color(colors.muted)
                                .child(mode),
                        ),
                )
            })
            .child(
                div()
                    .mx_3()
                    .mt_2()
                    .px_3()
                    .py_2()
                    .rounded(theme::RADIUS)
                    .bg(colors.panel)
                    .font_family(fonts.mono.family.clone())
                    .text_size(theme::TEXT_SECTION)
                    .child(command),
            )
            .when(!compact, |card| {
                card.child(
                    h_flex()
                        .px_3()
                        .pt_1()
                        .gap_3()
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child(
                            h_flex()
                                .gap_1()
                                .child(Icon::new(IconName::Folder).size(theme::SMALL_ICON_SIZE))
                                .child(self.workspace_name()),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .child(Icon::new(IconName::Shield).size(theme::SMALL_ICON_SIZE))
                                .child("ZJ 不会自动批准命令和写入"),
                        ),
                )
            })
            .child(
                h_flex()
                    .p_2()
                    .px_3()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(once)
                            .when(!compact, |b| b.child(keycap("⏎"))),
                    )
                    .children(always)
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap_1()
                            .child(reject)
                            .when(!compact, |b| b.child(keycap("Esc"))),
                    ),
            )
            .into_any_element()
    }

    // ----- changed files ------------------------------------------------------------------

    fn render_agent_changes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = theme::colors(cx);
        let session = self.agent.current()?;
        let files = &session.thread.changed_files;
        if files.is_empty() {
            return None;
        }
        let compact = self.agent.compact();
        let (added, removed) = files
            .values()
            .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
        let proposed = files.values().any(|c| c.origin == ChangeOrigin::Proposed);
        let collapsed = compact || self.agent.changes_collapsed;
        let key = session.key;
        let head =
            h_flex()
                .h(theme::AGENT_CARD_HEAD)
                .w_full()
                .pl_2()
                .pr_1()
                .gap_1()
                .text_size(theme::TEXT_CAPTION)
                .child(
                    h_flex()
                        .id("agent-changes-toggle")
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .cursor_pointer()
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .size(theme::SMALL_ICON_SIZE)
                            .text_color(colors.muted),
                        )
                        .child(div().flex_shrink_0().child(if compact {
                            format!("{} 个文件", files.len())
                        } else {
                            format!("{} 个文件有修改", files.len())
                        }))
                        .child(div().text_color(colors.added).child(format!("+{added}")))
                        .child(
                            div()
                                .text_color(colors.deleted)
                                .child(format!("−{removed}")),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.agent.changes_collapsed = !this.agent.changes_collapsed;
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("agent-reject-all")
                        .ghost()
                        .xsmall()
                        .label(if compact { "拒绝" } else { "全部拒绝" })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_resolve_all(false, window, cx)
                        })),
                )
                .child(
                    Button::new("agent-accept-all")
                        .ghost()
                        .selected(true)
                        .xsmall()
                        .label(if compact { "接受" } else { "全部接受" })
                        .tooltip(if proposed {
                            "把建议写入磁盘"
                        } else {
                            "保留已写入的修改（不再列在这里）"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_resolve_all(true, window, cx)
                        })),
                )
                .child(
                    Button::new("agent-review")
                        .ghost()
                        .xsmall()
                        .icon(IconName::GitCompare)
                        .label("审阅")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.agent_review_first(window, cx)),
                        ),
                );
        let rows = files.iter().map(|(path, change)| {
            let name = agent_model::file_name(path);
            let dir = path
                .parent()
                .map(|p| self.relative(p).display().to_string())
                .unwrap_or_default();
            let path = path.clone();
            h_flex()
                .id(SharedString::from(format!(
                    "agent-change-{}",
                    path.display()
                )))
                .h(theme::AGENT_FILE_ROW)
                .pl(theme::AGENT_FILE_INDENT)
                .pr_3()
                .gap_2()
                .cursor_pointer()
                .hover(|row| row.bg(colors.hover))
                .text_size(theme::TEXT_CAPTION)
                .child(file_icons::icon(file_icons::for_file(&name)))
                .child(div().flex_shrink_0().child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child(dir),
                )
                .when(change.new_file, |row| {
                    row.child(div().text_color(colors.added).child("新文件"))
                })
                .child(
                    div()
                        .text_color(colors.added)
                        .child(format!("+{}", change.added)),
                )
                .when(!change.new_file, |row| {
                    row.child(
                        div()
                            .text_color(colors.deleted)
                            .child(format!("−{}", change.removed)),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.agent_open_review(key, path.clone(), window, cx)
                }))
        });
        Some(
            v_flex()
                .mx_3()
                .flex_shrink_0()
                .rounded_t(theme::RADIUS_LARGE)
                .border_1()
                .border_b_0()
                .border_color(colors.card_border)
                .bg(colors.card)
                .child(head)
                .when(!collapsed, |card| card.children(rows).pb_1())
                .into_any_element(),
        )
    }

    // ----- composer -----------------------------------------------------------------------

    fn render_agent_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let compact = self.agent.compact();
        let session = self.agent.current();
        let attached = session.is_some_and(|s| !s.thread.changed_files.is_empty());
        let busy = session.is_some_and(LiveSession::busy);
        let empty = self.agent.composer.read(cx).value().trim().is_empty();
        let chips = self
            .agent
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let icon = match a {
                    Attachment::File(_) => None,
                    Attachment::Selection { .. } => Some(IconName::SquareDashedText),
                };
                chip(&a.label(), icon, colors).child(
                    div()
                        .id(("agent-chip-remove", i))
                        .cursor_pointer()
                        .text_color(colors.muted)
                        .child(Icon::new(IconName::Close).size(theme::SMALL_ICON_SIZE))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.agent_remove_attachment(i, cx)),
                        ),
                )
            })
            .collect::<Vec<_>>();
        let agent_picker = {
            let weak = cx.weak_entity();
            let presets: Vec<(String, String)> = self
                .agent
                .presets
                .iter()
                .map(|p| (p.id.clone(), p.display_name.clone()))
                .collect();
            let current = session.map_or(self.agent.agent_id.clone(), |s| s.preset.id.clone());
            let label = agent_name(&self.agent.presets, &current);
            Button::new("agent-picker")
                .ghost()
                .xsmall()
                .label(label)
                .dropdown_caret(true)
                .tooltip("选择 Agent（换 Agent 会开新会话）")
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (id, name) in &presets {
                        let weak = weak.clone();
                        let id = id.clone();
                        let checked = id == current;
                        menu =
                            menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                                move |_, window, cx| {
                                    let id = id.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        this.agent_new_session(Some(id), window, cx)
                                    });
                                },
                            ));
                    }
                    menu
                })
        };
        let mode_picker = session.and_then(|s| s.thread.modes.clone()).map(|modes| {
            let weak = cx.weak_entity();
            let label = modes
                .available
                .iter()
                .find(|(id, _)| *id == modes.current)
                .map_or(modes.current.clone(), |(_, name)| name.clone());
            Button::new("agent-mode")
                .ghost()
                .xsmall()
                .icon(IconName::ShieldCheck)
                .label(label)
                .dropdown_caret(true)
                .tooltip("会话模式（跳过审批的模式不提供）")
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (id, name) in &modes.available {
                        let weak = weak.clone();
                        let id = id.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(id == modes.current)
                                .on_click(move |_, _, cx| {
                                    let id = id.clone();
                                    let _ = weak.update(cx, |this, cx| this.agent_set_mode(id, cx));
                                }),
                        );
                    }
                    menu
                })
        });
        let ring = session.and_then(|s| agent_model::usage_ring(s.thread.usage));
        let send = if busy {
            Button::new("agent-stop")
                .ghost()
                .small()
                .icon(IconName::Square)
                .tooltip("停止")
                .on_click(cx.listener(|this, _, _, cx| this.agent_cancel(cx)))
        } else {
            Button::new("agent-send")
                .primary()
                .small()
                .icon(IconName::ArrowUp)
                .tooltip("发送（⏎）")
                .disabled(empty || self.root.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.agent_submit(window, cx)))
        };
        let composer = v_flex()
            .id("agent-composer")
            .mx_3()
            .mb_3()
            .flex_shrink_0()
            .relative()
            .border_1()
            .border_color(if self.agent.composer_focused {
                colors.accent.opacity(0.7)
            } else {
                colors.strong_border
            })
            .bg(colors.card)
            .map(|c| {
                if attached {
                    c.rounded_b(theme::RADIUS_LARGE)
                } else {
                    c.rounded(theme::RADIUS_LARGE)
                }
            })
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let plain = !event.keystroke.modifiers.modified();
                if this.agent.mention.is_some() {
                    match key {
                        "up" => this.agent_move_mention(-1, cx),
                        "down" => this.agent_move_mention(1, cx),
                        "enter" | "tab" if plain => this.agent_pick_mention(None, window, cx),
                        "escape" => this.agent_close_mention(cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                // With an empty composer, ⏎ allows once and Esc rejects the pending request.
                let empty = this.agent.composer.read(cx).value().trim().is_empty();
                if !empty || !plain {
                    return;
                }
                let pending = this.agent.current().and_then(|s| {
                    s.thread
                        .pending_permissions()
                        .next()
                        .map(|card| (s.key, card.request.id))
                });
                if let Some((session, id)) = pending {
                    let choice = match key {
                        "enter" => PermissionChoice::Once,
                        "escape" => PermissionChoice::Reject,
                        _ => return,
                    };
                    this.agent_answer(session, id, choice, window, cx);
                    cx.stop_propagation();
                }
            }))
            .when(!chips.is_empty() || !compact, |c| {
                c.child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .px_2()
                        .pt_2()
                        .children(chips)
                        .when(!compact, |row| {
                            row.child(
                                h_flex()
                                    .id("agent-add-context")
                                    .h(theme::AGENT_CHIP)
                                    .px_1()
                                    .gap_1()
                                    .rounded(theme::RADIUS)
                                    .border_1()
                                    .border_dashed()
                                    .border_color(colors.strong_border)
                                    .text_size(theme::TEXT_SECTION)
                                    .text_color(colors.muted)
                                    .cursor_pointer()
                                    .child(Icon::new(IconName::Plus).size(theme::SMALL_ICON_SIZE))
                                    .child("上下文")
                                    .tooltip(|window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            "加入编辑器里的选区或当前文件（⌘L）",
                                        )
                                        .build(window, cx)
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.agent_add_selection(window, cx)
                                    })),
                            )
                        }),
                )
            })
            .child(
                div().px_1().min_h(theme::AGENT_COMPOSER_MIN).child(
                    Textarea::new(&self.agent.composer)
                        .appearance(false)
                        .bordered(false)
                        .aria_label("给 Agent 的消息"),
                ),
            )
            .child(
                h_flex()
                    .h(theme::AGENT_COMPOSER_BAR)
                    .px_1()
                    .gap_1()
                    .child(
                        Button::new("agent-attach")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Paperclip)
                            .tooltip("附加当前文件或选区（⌘L）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.agent_add_selection(window, cx)
                            })),
                    )
                    .child(
                        Button::new("agent-mention")
                            .ghost()
                            .xsmall()
                            .icon(IconName::AtSign)
                            .tooltip("引用文件（@）")
                            .on_click(cx.listener(|this, _, window, cx| {
                                let text = this.agent.composer.read(cx).value().to_string();
                                let next = if text.is_empty() || text.ends_with(' ') {
                                    format!("{text}@")
                                } else {
                                    format!("{text} @")
                                };
                                this.agent
                                    .composer
                                    .update(cx, |c, cx| c.set_value(next, window, cx));
                                this.agent_focus_composer(window, cx);
                                this.agent_composer_changed(window, cx);
                            })),
                    )
                    .children(mode_picker)
                    .when(!compact, |bar| bar.child(agent_picker))
                    .child(div().flex_1())
                    .when_some(ring, |bar, (fraction, label)| {
                        bar.child(
                            h_flex()
                                .gap_1()
                                .px_1()
                                .text_size(theme::TEXT_BADGE)
                                .text_color(colors.muted)
                                .child(usage_ring(fraction, colors))
                                .child(label),
                        )
                    })
                    .child(send),
            )
            .children(self.render_mention_picker(cx));
        composer.into_any_element()
    }

    fn render_mention_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mention = self.agent.mention.as_ref()?;
        let colors = theme::colors(cx);
        let rows: Vec<AnyElement> = if mention.results.is_empty() {
            vec![
                div()
                    .px_3()
                    .h(theme::ROW_HEIGHT)
                    .flex()
                    .items_center()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(if self.index.is_none() {
                        "正在建立文件索引…"
                    } else {
                        "输入文件名的一部分"
                    })
                    .into_any_element(),
            ]
        } else {
            mention
                .results
                .iter()
                .take(theme::AGENT_MENTION_ROWS)
                .enumerate()
                .map(|(i, path)| {
                    let name = agent_model::file_name(path);
                    let dir = path
                        .parent()
                        .map(|p| self.relative(p).display().to_string())
                        .unwrap_or_default();
                    let selected = i == mention.selected;
                    h_flex()
                        .id(("agent-mention", i))
                        .h(theme::ROW_HEIGHT)
                        .px_2()
                        .gap_2()
                        .rounded(theme::RADIUS)
                        .text_size(theme::TEXT_CAPTION)
                        .cursor_pointer()
                        .map(|row| {
                            if selected {
                                row.bg(colors.selected).text_color(colors.selected_fg)
                            } else {
                                row.hover(|row| row.bg(colors.hover))
                            }
                        })
                        .child(file_icons::icon(file_icons::for_file(&name)))
                        .child(div().flex_shrink_0().child(name))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(theme::TEXT_SECTION)
                                .text_color(if selected {
                                    colors.selected_fg
                                } else {
                                    colors.muted
                                })
                                .child(dir),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.agent_pick_mention(Some(i), window, cx)
                        }))
                        .into_any_element()
                })
                .collect()
        };
        Some(
            v_flex()
                .absolute()
                .left_0()
                .right_0()
                .bottom_full()
                .mb_1()
                .p_1()
                .rounded(theme::RADIUS_LARGE)
                .border_1()
                .border_color(colors.strong_border)
                .bg(colors.panel)
                .shadow_lg()
                .occlude()
                .children(rows)
                .into_any_element(),
        )
    }

    // ----- settings -----------------------------------------------------------------------

    fn render_agent_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let section = |title: &'static str| {
            div()
                .pt_3()
                .pb_1()
                .text_size(theme::TEXT_SECTION)
                .text_color(colors.muted)
                .child(title)
        };
        let accept_first = settings.accept_first;
        let write_mode = h_flex()
            .gap_1()
            .child(
                Button::new("agent-write-direct")
                    .xsmall()
                    .outline()
                    .selected(!accept_first)
                    .label("直接写入")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.agent_set_write_mode(false, window, cx)
                    })),
            )
            .child(
                Button::new("agent-write-accept")
                    .xsmall()
                    .outline()
                    .selected(accept_first)
                    .label("先审阅再写入")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.agent_set_write_mode(true, window, cx)
                    })),
            );
        let idle = {
            let weak = cx.weak_entity();
            let current = settings.idle_minutes;
            Button::new("agent-idle")
                .xsmall()
                .outline()
                .label(format!("{current} 分钟"))
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for minutes in [5u32, 10, 20, 30, 60] {
                        let weak = weak.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{minutes} 分钟"))
                                .checked(minutes == current)
                                .on_click(move |_, window, cx| {
                                    let _ = weak.update(cx, |this, cx| {
                                        this.change_settings(window, cx, |s| {
                                            s.agent.idle_minutes = minutes
                                        });
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let default_agent = {
            let weak = cx.weak_entity();
            let presets: Vec<(String, String)> = self
                .agent
                .presets
                .iter()
                .map(|p| (p.id.clone(), p.display_name.clone()))
                .collect();
            let current = settings.default_agent.clone();
            Button::new("agent-default")
                .xsmall()
                .outline()
                .label(agent_name(&self.agent.presets, &current))
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (id, name) in &presets {
                        let weak = weak.clone();
                        let id = id.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(id == current)
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        this.agent.agent_id = id.clone();
                                        this.change_settings(window, cx, move |s| {
                                            s.agent.default_agent = id
                                        });
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let line = |label: &'static str, control: AnyElement| {
            h_flex()
                .min_h(theme::AGENT_TOOL_ROW)
                .gap_2()
                .text_size(theme::TEXT_CAPTION)
                .child(div().flex_1().child(label))
                .child(control)
        };
        let agents = self.agent.presets.iter().map(|preset| {
            let env = settings.env.get(&preset.id);
            let env_lines: Vec<String> = preset
                .env
                .iter()
                .map(|(name, value)| match value {
                    workspace_editor_agent::registry::EnvValue::FromEnv(var) => {
                        match env.and_then(|e| e.get(name)) {
                            Some(source) => format!("{name} ← {source}"),
                            None => format!("{name} ← ${var}"),
                        }
                    }
                    workspace_editor_agent::registry::EnvValue::Literal(v) => {
                        format!("{name} = {v}")
                    }
                })
                .chain(
                    env.into_iter()
                        .flatten()
                        .filter(|(name, _)| !preset.env.iter().any(|(n, _)| n == *name))
                        .map(|(name, source)| format!("{name} ← {source}")),
                )
                .collect();
            v_flex()
                .py_2()
                .gap_1()
                .border_b_1()
                .border_color(colors.border)
                .child(
                    h_flex()
                        .gap_2()
                        .text_size(theme::TEXT_CAPTION)
                        .child(glyph_tile(preset.glyph, theme::AGENT_GLYPH_SMALL, colors))
                        .child(div().flex_1().child(preset.display_name.clone()))
                        .child(
                            div()
                                .text_size(theme::TEXT_SECTION)
                                .text_color(colors.muted)
                                .child(preset.id.clone()),
                        ),
                )
                .children(env_lines.into_iter().map(|line| {
                    div()
                        .pl(theme::AGENT_FILE_INDENT)
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .font_family(
                            gpui_kit::component::Theme::global(cx)
                                .mono_font_family
                                .clone(),
                        )
                        .child(line)
                }))
        });
        let rules = self
            .root
            .as_ref()
            .and_then(|root| settings.allow.get(&root.to_string_lossy().into_owned()))
            .cloned()
            .unwrap_or_default();
        v_flex()
            .id("agent-settings")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_3()
            .pb_3()
            .child(section("写入方式"))
            .child(line("Agent 修改文件时", write_mode.into_any_element()))
            .child(
                div()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(if accept_first {
                        "修改先留在 ZJ 里，在 Diff 中逐处接受后才写入磁盘。Agent 自己运行的命令（如 cargo fmt）仍会直接改动文件。"
                    } else {
                        "修改直接写入磁盘；面板列出改动过的文件，可与 Agent 改动前的内容对比、逐处还原。"
                    }),
            )
            .child(section("会话"))
            .child(line("新会话默认使用", default_agent.into_any_element()))
            .child(line("空闲多久后停止 Agent 进程", idle.into_any_element()))
            .child(section("此工作区的始终允许"))
            .when(rules.is_empty(), |s| {
                s.child(
                    div()
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child("还没有规则。审批卡上的“始终允许”只对这个工作区的同一命令前缀生效；带 && ; | 等的命令每次都会询问。"),
                )
            })
            .children(rules.into_iter().enumerate().map(|(i, rule)| {
                let remove = rule.clone();
                h_flex()
                    .h(theme::AGENT_FILE_ROW)
                    .gap_2()
                    .text_size(theme::TEXT_CAPTION)
                    .child(
                        div()
                            .flex_1()
                            .font_family(gpui_kit::component::Theme::global(cx).mono_font_family.clone())
                            .child(rule),
                    )
                    .child(
                        Button::new(("agent-rule-remove", i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .tooltip("移除规则")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.agent_remove_rule(remove.clone(), window, cx)
                            })),
                    )
            }))
            .child(section("Agent 与环境变量"))
            .children(agents)
            .child(
                div()
                    .pt_2()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(format!(
                        "密钥不写进设置文件：在设置的 agent.env 里写 \"$变量名\"（取 ZJ 的环境变量）或 \"keychain:账户名\"（macOS 钥匙串，服务名 {}）。存入钥匙串：{}",
                        crate::secrets::KEYCHAIN_SERVICE,
                        crate::secrets::keychain_hint("deepseek")
                    )),
            )
            .child(section("会话历史"))
            .child(
                div()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(
                        cx.try_global::<agent::AgentStore>()
                            .and_then(|s| s.history.as_ref())
                            .map(|h| format!("保存在本机 {}（SQLite，只有你能读）。删除会话会同时删除消息和搜索索引并回收空间。", h.path().display()))
                            .unwrap_or_else(|| "没有可用的数据目录，会话历史不会保存".into()),
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
