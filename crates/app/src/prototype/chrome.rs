//! Window chrome: the unified title bar with its command center.

use super::Prototype;
use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable, TitleBar,
        button::{Button, ButtonVariants},
        h_flex,
    },
    *,
};

impl Prototype {
    pub(super) fn workspace_name(&self) -> String {
        self.root
            .as_ref()
            .map(|root| {
                root.file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "ZJ".into())
    }

    pub(super) fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let command_center = h_flex()
            .id("command-center")
            .w(theme::COMMAND_CENTER_WIDTH)
            .min_w_0()
            .h(theme::COMMAND_CENTER_HEIGHT)
            .px_2()
            .gap_2()
            .justify_center()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.command_border)
            .bg(colors.command_bg)
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted)
            .cursor_pointer()
            .hover(|this| this.bg(colors.hover).text_color(colors.foreground))
            .role(Role::Button)
            .aria_label("转到文件（⌘P）")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.open_quick_open(window, cx)))
            .child(
                Icon::new(IconName::Search)
                    .size(theme::SMALL_ICON_SIZE)
                    .text_color(colors.muted),
            )
            .child(self.workspace_name());
        let actions = h_flex().flex_1().justify_end().pr_2().gap_1().child(
            Button::new("toggle-sidebar")
                .xsmall()
                .ghost()
                .icon(if self.sidebar_visible {
                    IconName::PanelLeftClose
                } else {
                    IconName::PanelLeftOpen
                })
                .tooltip("切换侧栏（⌘B）")
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sidebar(cx))),
        );
        TitleBar::new()
            .h(theme::TITLE_HEIGHT)
            .bg(colors.title)
            .border_color(colors.border)
            .child(
                h_flex()
                    .size_full()
                    .items_center()
                    .child(div().flex_1())
                    .child(command_center)
                    .child(actions),
            )
            .into_any_element()
    }
}
