//! The panel's settings view.

use super::*;

impl Workbench {
    // ----- settings -----------------------------------------------------------------------

    pub(super) fn render_agent_settings(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .agent_workspace(cx)
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
