//! Thread rows: the row dispatcher, user messages, replies (Markdown) and thoughts.

use super::*;

impl Workbench {
    // ----- thread rows --------------------------------------------------------------------

    pub(super) fn render_agent_row(
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
            Item::Agent { .. } => match session.md.get(&(session.thread.dropped + i)).cloned() {
                Some(blocks) => self.render_markdown(blocks.iter(), &fonts, cx),
                None => self.render_markdown(std::iter::empty(), &fonts, cx),
            },
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

    pub(super) fn render_user_message(
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

    pub(super) fn render_markdown<'a>(
        &self,
        blocks: impl Iterator<Item = &'a Block>,
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
            .children(blocks.enumerate().map(|(i, block)| {
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
                                .child(match marker.as_str() {
                                    crate::markdown::TASK_OPEN | crate::markdown::TASK_DONE => {
                                        Icon::new(if marker == crate::markdown::TASK_DONE {
                                            IconName::SquareCheck
                                        } else {
                                            IconName::Square
                                        })
                                        .size(theme::SMALL_ICON_SIZE)
                                        .into_any_element()
                                    }
                                    _ => marker.clone().into_any_element(),
                                }),
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

    pub(super) fn render_thought(
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
}
