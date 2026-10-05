//! The composer: attachments, the input, the toolbar and the @ file picker.

use super::*;

impl Workbench {
    // ----- composer -----------------------------------------------------------------------

    pub(super) fn render_agent_composer(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .disabled(empty)
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

    /// What the `@` picker says when it has no files to show.
    pub(in crate::workbench) fn agent_mention_hint(&self) -> &'static str {
        let query = self.agent.mention.as_ref().map_or("", |m| m.query.as_str());
        if self.root.is_none() {
            // Without a folder there is no index; only open files can be referenced.
            if self.documents.is_empty() {
                "没有打开文件夹，也没有打开的文件可以引用"
            } else {
                "已打开的文件里没有匹配的（没有打开文件夹）"
            }
        } else if self.index.is_none() {
            "正在建立文件索引…"
        } else if query.is_empty() {
            "输入文件名的一部分"
        } else {
            "没有匹配的文件"
        }
    }

    pub(super) fn render_mention_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
                    .child(self.agent_mention_hint())
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
}
