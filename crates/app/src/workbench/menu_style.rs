//! Compact dropdown triggers and shared popup-menu bounds.

use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{Icon, button::Button, h_flex, menu::PopupMenu},
    *,
};

pub(super) fn compact_dropdown(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    Button::new(id)
        .min_w_0()
        .max_w_full()
        .flex_shrink(1.)
        .accessibility_label(label.clone())
        .child(
            h_flex()
                .min_w_0()
                .max_w_full()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(theme::SMALL_ICON_SIZE)
                        .flex_shrink_0(),
                ),
        )
}

pub(super) fn compact_menu(menu: PopupMenu) -> PopupMenu {
    menu.min_w(theme::MENU_MIN_WIDTH)
        .max_w(theme::MENU_MAX_WIDTH)
}
