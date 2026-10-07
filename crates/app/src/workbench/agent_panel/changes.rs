//! The changed-files card above the composer.

use super::*;

impl Workbench {
    // ----- changed files ------------------------------------------------------------------

    pub(super) fn render_agent_changes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        let collapsed = session.changes_collapsed;
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
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(session) = this.agent.session_mut(key) {
                                session.changes_collapsed = !session.changes_collapsed;
                            }
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
                        .tooltip("保留已写入的修改（不再列在这里）")
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
}
