//! The composer: attachments, the input, the toolbar and the @ file and / command pickers.

use super::*;

impl Workbench {
    // ----- composer -----------------------------------------------------------------------

    pub(super) fn render_agent_composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let compact = self.agent.compact();
        let session = self.agent.current();
        let attached = session.is_some_and(|s| !s.thread.changed_files.is_empty());
        let busy = session.is_some_and(LiveSession::busy);
        let empty = self.agent.composer.read(cx).value().trim().is_empty()
            && self.agent.attachments.is_empty();
        let chips = self
            .agent
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let icon = match a {
                    Attachment::File(_) => None,
                    Attachment::Image { .. } => Some(IconName::Image),
                    Attachment::Selection { .. } => Some(IconName::SquareDashedText),
                };
                let remove = Button::new(("agent-chip-remove", i))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("删除附件")
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.agent_remove_attachment(i, cx)),
                    );
                if let Attachment::Image { id, .. } = a {
                    v_flex()
                        .w(theme::AGENT_IMAGE_WIDTH)
                        .gap_1()
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .h(theme::AGENT_IMAGE_HEIGHT)
                                .rounded(theme::RADIUS)
                                .overflow_hidden()
                                .bg(colors.editor)
                                .children(self.agent.image_previews.get(id).map(|image| {
                                    img(image.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Contain)
                                }))
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .right_0()
                                        .bg(colors.card)
                                        .rounded(theme::RADIUS)
                                        .child(remove),
                                ),
                        )
                        .child(
                            div()
                                .text_size(theme::TEXT_BADGE)
                                .text_color(colors.muted)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(a.label()),
                        )
                        .into_any_element()
                } else {
                    chip(&a.label(), icon, colors)
                        .child(remove)
                        .into_any_element()
                }
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
        let config_picker = session
            .map(|s| s.thread.configs.clone())
            .filter(|configs| !configs.is_empty())
            .map(|configs| {
                let weak = cx.weak_entity();
                Button::new("agent-config")
                    .ghost()
                    .xsmall()
                    .label(agent_model::config_label(&configs))
                    .dropdown_caret(true)
                    .tooltip("模型与思考强度")
                    .dropdown_menu(move |menu, _, _| {
                        let mut menu = menu;
                        for (i, config) in configs.iter().enumerate() {
                            if i > 0 {
                                menu = menu.separator();
                            }
                            menu = menu.label(config.name.clone());
                            for (value, name) in &config.values {
                                let weak = weak.clone();
                                let id = config.id.clone();
                                let value = value.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(name.clone())
                                        .checked(value == config.current)
                                        .on_click(move |_, _, cx| {
                                            let (id, value) = (id.clone(), value.clone());
                                            let _ = weak.update(cx, |this, cx| {
                                                this.agent_set_config(id, value, cx)
                                            });
                                        }),
                                );
                            }
                        }
                        menu
                    })
            });
        let quota = self.agent_shows_quota().then(|| {
            let label = self
                .agent
                .quota
                .as_ref()
                .and_then(crate::quota::Quota::left_percent)
                .map_or_else(|| "额度".into(), |left| format!("额度 {left:.0}%"));
            gpui_kit::base::Popup::new(
                "agent-quota-popup",
                Button::new("agent-quota")
                    .ghost()
                    .xsmall()
                    .label(label)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agent.quota_open = true;
                        this.agent.quota_pinned = true;
                        this.agent_query_quota(cx);
                        cx.notify();
                    })),
            )
            .anchor(Anchor::BottomRight)
            .offset(theme::ROW_INSET)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.agent.quota_open = *hovered || this.agent.quota_pinned;
                if *hovered {
                    this.agent_query_quota(cx);
                }
                cx.notify();
            }))
            .when_some(self.render_quota_card(cx), |popup, card| {
                popup.content(card)
            })
        });
        let can_steer = session
            .and_then(|s| s.client.as_ref())
            .is_some_and(|c| c.supports_steering());
        let send = Button::new("agent-send")
            .primary()
            .small()
            .icon(IconName::ArrowUp)
            .tooltip(if busy && !can_steer {
                "当前 Agent 尚不能接收运行中的补充指令"
            } else if busy {
                "追加指令（⏎）"
            } else {
                "发送（⏎）"
            })
            .disabled(empty || self.agent.images_loading > 0 || (busy && !can_steer))
            .on_click(cx.listener(|this, _, window, cx| this.agent_submit(window, cx)));
        let composer = v_flex()
            .id("agent-composer")
            .mx_3()
            .mt_3()
            .mb_3()
            .flex_shrink_0()
            .relative()
            .border_1()
            .border_color(if self.agent.composer_focused {
                colors.focus_border
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
            .capture_action(
                cx.listener(|this, _: &gpui_kit::component::input::Paste, _, cx| {
                    if !this.agent_paste_images(cx) {
                        cx.propagate();
                    }
                }),
            )
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let plain = !event.keystroke.modifiers.modified();
                // ⏎ reaches `agent_submit` even when stopped here, so the pickers take it there
                // (picking here as well sent the message right after).
                if this.agent.mention.is_some() {
                    match key {
                        "up" => this.agent_move_mention(-1, cx),
                        "down" => this.agent_move_mention(1, cx),
                        "tab" if plain => this.agent_pick_mention(None, window, cx),
                        "escape" => this.agent_close_mention(cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                if this.agent.slash.is_some() {
                    let any = !this.agent_slash_matches(cx).is_empty();
                    match key {
                        "up" if any => this.agent_move_slash(-1, cx),
                        "down" if any => this.agent_move_slash(1, cx),
                        "tab" if plain && any => this.agent_pick_slash(None, window, cx),
                        "escape" => this.agent_close_slash(cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                    return;
                }
                // With an empty composer, ⏎ allows once and Esc rejects the pending request.
                let empty = this.agent.composer.read(cx).value().trim().is_empty()
                    && this.agent.attachments.is_empty();
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
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.agent_attach_images(paths.paths().to_vec(), cx)
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
                div().px_1().child(
                    Textarea::new(&self.agent.composer)
                        .min_h(theme::AGENT_COMPOSER_MIN)
                        .appearance(false)
                        .bordered(false)
                        .aria_label("给 Agent 的消息"),
                ),
            )
            .when(self.agent.images_loading > 0, |composer| {
                composer.child(
                    div()
                        .px_2()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child("正在载入图片…"),
                )
            })
            .child(
                h_flex()
                    .h(theme::AGENT_COMPOSER_BAR)
                    .px_1()
                    .gap_1()
                    .child(
                        Button::new("agent-add-image")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Image)
                            .tooltip("添加图片（也可粘贴或拖入）")
                            .on_click(cx.listener(|this, _, _, cx| this.agent_pick_images(cx))),
                    )
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
                    .children(config_picker)
                    .when(!compact, |bar| bar.child(agent_picker))
                    .child(div().flex_1())
                    .children(quota)
                    .child(send)
                    .when(busy, |bar| {
                        bar.child(
                            Button::new("agent-stop")
                                .ghost()
                                .small()
                                .icon(IconName::Square)
                                .tooltip("停止")
                                .on_click(cx.listener(|this, _, _, cx| this.agent_cancel(cx))),
                        )
                    }),
            )
            .children(self.render_mention_picker(cx))
            .children(self.render_slash_picker(cx));
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

    /// The agent's commands while the message starts with `/` (empty until the agent has
    /// started once).
    pub(super) fn render_slash_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let selected = self.agent.slash?;
        let colors = theme::colors(cx);
        let matches = self.agent_slash_matches(cx);
        let rows: Vec<AnyElement> = if matches.is_empty() {
            let session = self.agent.current();
            let hint = if !self.agent_commands(cx).is_empty() {
                "没有匹配的命令"
            } else if session.is_some_and(|s| s.thread.session_id.is_some()) {
                "这个 Agent 没有提供命令"
            } else {
                "正在启动 Agent，稍后列出命令（也可以直接输入完整命令发送）"
            };
            vec![
                div()
                    .px_3()
                    .h(theme::ROW_HEIGHT)
                    .flex()
                    .items_center()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(hint)
                    .into_any_element(),
            ]
        } else {
            matches
                .into_iter()
                .enumerate()
                .map(|(i, command)| {
                    let selected = i == selected;
                    let detail = match &command.input_hint {
                        Some(hint) if command.description.is_empty() => hint.clone(),
                        Some(hint) => format!("{} · {hint}", command.description),
                        None => command.description.clone(),
                    };
                    h_flex()
                        .id(("agent-slash", i))
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
                        .child(div().flex_shrink_0().child(format!("/{}", command.name)))
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
                                .child(detail),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.agent_pick_slash(Some(i), window, cx)
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

    /// The Codex quota's details: each limit's share left, when it resets, the credits.
    fn render_quota_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.agent.quota_open || !self.agent_shows_quota() {
            return None;
        }
        let quota = self.agent.quota.as_ref();
        let colors = theme::colors(cx);
        let now_ms = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now_ms);
        let line = |text: String| {
            div()
                .h(theme::ROW_HEIGHT)
                .flex()
                .items_center()
                .text_size(theme::TEXT_CAPTION)
                .child(text)
        };
        let title = match quota.and_then(|quota| quota.plan.as_ref()) {
            Some(plan) => format!("Codex 额度 · ChatGPT {}", agent_model::capitalized(plan)),
            None => "Codex 额度".to_string(),
        };
        let windows = quota
            .into_iter()
            .flat_map(|quota| &quota.windows)
            .map(|window| {
                let left = window.left_percent();
                let color = if left < 10.0 {
                    colors.deleted
                } else if left < 25.0 {
                    colors.attention
                } else {
                    colors.foreground
                };
                let reset = window.resets_at.map(|at| {
                    let (_, month, day, _, hour, minute) = agent_model::civil(at * 1000, offset);
                    format!(
                        "{} 后重置（{month}/{day} {hour:02}:{minute:02}）",
                        crate::quota::countdown(at - now_ms / 1000)
                    )
                });
                h_flex()
                    .h(theme::ROW_HEIGHT)
                    .gap_2()
                    .text_size(theme::TEXT_CAPTION)
                    .child(crate::quota::window_label(window.minutes))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(format!("剩余 {left:.0}%")),
                    )
                    .children(reset.map(|reset| div().text_color(colors.muted).child(reset)))
            });
        let updated = quota.map(|quota| {
            let (year, month, day, _, hour, minute) = agent_model::civil(quota.updated_ms, offset);
            format!(
                "{} {year}/{month:02}/{day:02} {hour:02}:{minute:02}",
                if quota.live {
                    "已刷新"
                } else {
                    "上次记录"
                }
            )
        });
        Some(
            v_flex()
                .id("agent-quota-card")
                .w(theme::AGENT_QUOTA_WIDTH)
                .px_3()
                .py_2()
                .rounded(theme::RADIUS_LARGE)
                .border_1()
                .border_color(colors.strong_border)
                .bg(colors.panel)
                .shadow_lg()
                .occlude()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.agent.quota_open = false;
                    this.agent.quota_pinned = false;
                    cx.notify();
                }))
                .child(line(title).font_weight(FontWeight::SEMIBOLD))
                .when(self.agent.quota_loading, |card| {
                    card.child(line("正在刷新…".into()).text_color(colors.muted))
                })
                .when_some(self.agent.quota_error.clone(), |card, error| {
                    card.child(
                        div()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(colors.deleted)
                            .child(error),
                    )
                })
                .when(!self.agent.quota_loading, |card| {
                    card.children(windows)
                        .children(
                            quota
                                .and_then(|q| q.credits.clone())
                                .map(|credits| line(format!("Credits 余额 {credits}"))),
                        )
                        .children(updated.map(|text| line(text).text_color(colors.muted)))
                })
                .map(|card| {
                    #[cfg(test)]
                    let card = {
                        use gpui_kit::test::TestSupportExt;
                        card.test_support()
                    };
                    card.into_any_element()
                }),
        )
    }
}
