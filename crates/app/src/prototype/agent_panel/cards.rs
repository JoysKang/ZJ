//! Cards in the thread: tool calls, plans, logins and permission requests.

use super::*;

impl Prototype {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_tool(
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

    pub(super) fn render_plan(
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

    pub(super) fn render_login(
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

    pub(super) fn render_permission(
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
}
