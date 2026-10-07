//! Thread rows: the row dispatcher, user messages, replies (Kit's `TextView`) and thoughts.

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
        let Some(row) = self.agent.thread_rows.get(index - usize::from(older)) else {
            return div().into_any_element();
        };
        if !row.process {
            return self.render_agent_item(row.range.start, window, cx);
        }
        let key = session.key;
        let start = row.range.start + session.thread.dropped;
        let expanded = self.agent.expanded_processes.contains(&(key, start));
        let active = session.busy()
            && row.range.start >= session.thread.turn_starts.last().copied().unwrap_or(0);
        v_flex()
            .w_full()
            .px_3()
            .pb_3()
            .gap_1()
            .child(
                Button::new(("agent-process", start))
                    .ghost()
                    .xsmall()
                    .w_full()
                    .justify_start()
                    .h(theme::AGENT_TOOL_ROW)
                    .gap_1()
                    .cursor_pointer()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .icon(if expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .label(if active { "执行中" } else { "执行过程" })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.agent.expanded_processes.remove(&(key, start)) {
                            this.agent.expanded_processes.insert((key, start));
                        }
                        this.agent.thread_list.pause_following_tail();
                        this.agent_sync_list(false);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_agent_item(
        &self,
        i: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(session) = self.agent.current() else {
            return div().into_any_element();
        };
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
                self.render_user_message(session.key, i, text, attachments, compact, cx)
            }
            Item::Agent { streaming, .. } => {
                let index = session.thread.dropped + i;
                let text = session.md.get(&index).cloned().unwrap_or_default();
                let id = SharedString::from(format!("agent-md-{}-{index}", session.key));
                let view = self.agent_text(id, text, false, cx);
                // Highlighted once complete: a growing block would be highlighted every batch.
                let view = if *streaming {
                    view
                } else {
                    view.shared_code_block_highlighter(self.agent.code.highlighter.clone())
                };
                div()
                    .w_full()
                    .line_height(theme::AGENT_LINE)
                    .child(view)
                    .into_any_element()
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
        // A user message starts a turn: more room above it than between a turn's rows.
        let turn = matches!(item, Item::User { .. });
        let took = session
            .turn_times
            .get(&(session.thread.dropped + i))
            .map(|took| {
                div()
                    .pt_1()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(agent_model::turn_took(*took))
            });
        div()
            .w_full()
            .px_3()
            .when(i == 0, |row| row.pt_3())
            .when(i > 0 && turn, |row| row.pt_3())
            .when(gap, |row| row.pb_3())
            .child(content)
            .children(took)
            .into_any_element()
    }

    pub(super) fn render_user_message(
        &self,
        key: u64,
        index: usize,
        text: &str,
        attachments: &[String],
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        // A bubble on the right, like a chat: replies run full width on the left.
        let bubble = v_flex()
            .min_w_0()
            .max_w(relative(theme::AGENT_USER_WIDTH))
            .gap_1()
            .rounded(theme::RADIUS_LARGE)
            .bg(colors.user_bubble)
            .map(|bubble| {
                if compact {
                    bubble.px_2().py_1()
                } else {
                    bubble.px_3().py_2()
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
            .child(div().line_height(theme::AGENT_LINE).child(self.agent_text(
                SharedString::from(format!("agent-user-{key}-{index}")),
                agent_model::literal_markdown(text).into(),
                false,
                cx,
            )));
        h_flex()
            .w_full()
            .justify_end()
            .child(bubble)
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
                        .child(self.agent_text(
                            SharedString::from(format!("agent-thought-{key}-{index}")),
                            text.to_string().into(),
                            true,
                            cx,
                        )),
                )
            })
            .into_any_element()
    }
}
