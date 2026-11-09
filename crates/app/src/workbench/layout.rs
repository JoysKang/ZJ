//! Panel size contracts: a panel asks for its preferred size, but the window's remaining
//! space wins. Each panel keeps its own minimum; when the window cannot satisfy both, the
//! sidebar and terminal shrink first so the editor stays usable.

use crate::theme;
use gpui_kit::Pixels;

/// The widest the sidebar may be without pushing the editor below its minimum.
pub(super) fn max_sidebar_width(viewport: Pixels) -> Pixels {
    (viewport - theme::EDITOR_MIN - theme::WORKBENCH_INSET * 2.)
        .max(theme::SIDEBAR_MIN)
        .min(theme::SIDEBAR_MAX)
}

/// The sidebar's width: the user's preference, clamped to the space this window has.
pub(super) fn sidebar_width(preferred: Pixels, viewport: Pixels) -> Pixels {
    preferred.clamp(theme::SIDEBAR_MIN, max_sidebar_width(viewport))
}

/// The tallest the terminal panel may be while the editor keeps its minimum height.
pub(super) fn max_terminal_height(viewport: Pixels) -> Pixels {
    (viewport
        - theme::TITLE_HEIGHT
        - theme::STATUS_HEIGHT
        - theme::CHROME_GAP
        - theme::EDITOR_MIN_HEIGHT)
        .max(theme::TERMINAL_MIN)
        .min(theme::TERMINAL_MAX)
}

/// The terminal's height: its preference, clamped to the window's remaining height.
pub(super) fn terminal_height(preferred: Pixels, viewport: Pixels) -> Pixels {
    preferred.clamp(theme::TERMINAL_MIN, max_terminal_height(viewport))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;

    #[test]
    fn a_wide_sidebar_gives_way_before_the_editor_does() {
        // 760 px is the window minimum: 760 - 320 editor - 4 panel insets.
        assert_eq!(max_sidebar_width(px(760.)), px(436.));
        assert_eq!(sidebar_width(px(520.), px(760.)), px(436.));
        assert_eq!(sidebar_width(px(400.), px(760.)), px(400.));
        assert_eq!(sidebar_width(px(520.), px(1400.)), px(520.));
        assert_eq!(sidebar_width(px(100.), px(1400.)), theme::SIDEBAR_MIN);
    }

    #[test]
    fn a_tall_terminal_gives_way_before_the_editor_does() {
        assert_eq!(terminal_height(theme::TERMINAL_HEIGHT, px(900.)), px(280.));
        assert_eq!(terminal_height(theme::TERMINAL_HEIGHT, px(480.)), px(219.));
        assert_eq!(terminal_height(px(4000.), px(480.)), px(219.));
        assert_eq!(terminal_height(px(10.), px(900.)), theme::TERMINAL_MIN);
    }

    #[test]
    fn a_window_too_small_for_both_keeps_each_panel_minimum() {
        assert_eq!(max_sidebar_width(px(300.)), theme::SIDEBAR_MIN);
        assert_eq!(max_terminal_height(px(300.)), theme::TERMINAL_MIN);
    }
}
