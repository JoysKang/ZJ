//! Shared popup-menu bounds.

use crate::theme;
use gpui_kit::component::menu::PopupMenu;

pub(super) fn compact_menu(menu: PopupMenu) -> PopupMenu {
    menu.min_w(theme::MENU_MIN_WIDTH)
        .max_w(theme::MENU_MAX_WIDTH)
}
