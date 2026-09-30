//! Design tokens: the only place with literal sizes and colors.
//!
//! Sizes sit on a 4 px grid. Colors are semantic and come in a light and a dark palette; both are
//! also written into GPUI Kit's theme so Kit components (Editor, inputs, scrollbars) match.

use gpui_kit::{
    App, Hsla, Pixels, Window, WindowAppearance,
    component::{Theme, ThemeColor, ThemeMode},
    px, rgb,
};

// Spacing (4 px grid). Kit's Tailwind-style helpers (`gap_1` = 4, `px_2` = 8, `px_3` = 12,
// `p_4` = 16) are already on the grid; constants exist only where arithmetic is needed.
pub const SPACE_2: Pixels = px(8.);

// Heights: every list row shares one height; every bar shares another.
pub const ROW_HEIGHT: Pixels = px(24.);
pub const BAR_HEIGHT: Pixels = px(32.);
pub const STATUS_HEIGHT: Pixels = px(24.);
/// Active tab indicator thickness.
pub const INDICATOR: Pixels = px(2.);

// Widths.
pub const RAIL_WIDTH: Pixels = px(40.);
pub const TREE_INDENT: Pixels = px(12.);
pub const TWISTY_WIDTH: Pixels = px(16.);
pub const STATUS_GLYPH_WIDTH: Pixels = px(16.);
pub const SIDEBAR_WIDTH: Pixels = px(280.);
pub const SIDEBAR_MIN: Pixels = px(200.);
pub const SIDEBAR_MAX: Pixels = px(520.);
pub const EDITOR_MIN: Pixels = px(320.);
pub const EDITOR_MAX: Pixels = px(4000.);

// Icons and text.
pub const ICON_SIZE: Pixels = px(14.);
pub const TEXT_BODY: Pixels = px(13.);
pub const TEXT_CAPTION: Pixels = px(12.);

// Window geometry.
pub const WINDOW_WIDTH: Pixels = px(1100.);
pub const WINDOW_HEIGHT: Pixels = px(720.);
pub const WINDOW_MIN_WIDTH: Pixels = px(760.);
pub const WINDOW_MIN_HEIGHT: Pixels = px(480.);
pub const WINDOW_ORIGIN: Pixels = px(60.);
pub const WINDOW_CASCADE: Pixels = px(28.);

/// Semantic colors as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Editor and active tab.
    pub editor: u32,
    /// Sidebar and activity rail.
    pub panel: u32,
    /// Top bar, tab bar, status bar, group headers.
    pub bar: u32,
    pub border: u32,
    pub hover: u32,
    /// Selected row; distinct from hover.
    pub selected: u32,
    /// Editor current-line highlight.
    pub active_line: u32,
    pub foreground: u32,
    /// Secondary text; ≥ 4.5:1 on every surface above.
    pub muted: u32,
    pub accent: u32,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflict: u32,
}

pub const LIGHT: Palette = Palette {
    editor: 0xffffff,
    panel: 0xf6f8fa,
    bar: 0xeef1f4,
    border: 0xd8dee4,
    hover: 0xe6eaef,
    selected: 0xdae8fc,
    active_line: 0xf6f8fa,
    foreground: 0x1f2328,
    muted: 0x59636e,
    accent: 0x0969da,
    added: 0x1a7f37,
    modified: 0x9a6700,
    deleted: 0xcf222e,
    untracked: 0x1b7c83,
    conflict: 0x8250df,
};

pub const DARK: Palette = Palette {
    editor: 0x1e1f22,
    panel: 0x191a1d,
    bar: 0x151618,
    border: 0x2e3035,
    hover: 0x2a2c30,
    selected: 0x1a3150,
    active_line: 0x25272b,
    foreground: 0xdfe1e5,
    muted: 0x9aa3ad,
    accent: 0x4c9aff,
    added: 0x3fb950,
    modified: 0xd29922,
    deleted: 0xf85149,
    untracked: 0x39c5cf,
    conflict: 0xbc8cff,
};

/// The palette converted for drawing.
#[derive(Clone, Copy)]
pub struct Colors {
    pub editor: Hsla,
    pub panel: Hsla,
    pub bar: Hsla,
    pub border: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub active_line: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub added: Hsla,
    pub modified: Hsla,
    pub deleted: Hsla,
    pub untracked: Hsla,
    pub conflict: Hsla,
}

impl Palette {
    fn colors(&self) -> Colors {
        let c = |hex: u32| -> Hsla { rgb(hex).into() };
        Colors {
            editor: c(self.editor),
            panel: c(self.panel),
            bar: c(self.bar),
            border: c(self.border),
            hover: c(self.hover),
            selected: c(self.selected),
            active_line: c(self.active_line),
            foreground: c(self.foreground),
            muted: c(self.muted),
            accent: c(self.accent),
            added: c(self.added),
            modified: c(self.modified),
            deleted: c(self.deleted),
            untracked: c(self.untracked),
            conflict: c(self.conflict),
        }
    }
}

/// Colors for the current light / dark mode.
pub fn colors(cx: &App) -> Colors {
    if Theme::global(cx).is_dark() {
        DARK.colors()
    } else {
        LIGHT.colors()
    }
}

/// `ZJ_APPEARANCE=light|dark` pins the mode (screenshots, debugging); anything else follows
/// the system.
fn pinned_mode() -> Option<ThemeMode> {
    match std::env::var("ZJ_APPEARANCE").ok()?.as_str() {
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        _ => None,
    }
}

/// Applies the system (or pinned) appearance. Pass the window when there is one: on Linux only
/// the window reports the appearance reliably.
pub fn follow_appearance(window: Option<&mut Window>, cx: &mut App) {
    let mode = pinned_mode().unwrap_or_else(|| {
        let appearance: WindowAppearance = window
            .as_ref()
            .map(|window| window.appearance())
            .unwrap_or_else(|| cx.window_appearance());
        appearance.into()
    });
    // Changing the mode reloads Kit's theme file, so the palette is re-applied afterwards.
    Theme::change(mode, None, cx);
    let palette = if mode.is_dark() { DARK } else { LIGHT };
    Theme::update(cx, |theme| {
        apply(&palette, &mut theme.colors);
        // The Editor paints from the syntax theme's own style block, not from ThemeColor.
        let c = palette.colors();
        let style = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        style.editor_background = Some(c.editor);
        style.editor_foreground = Some(c.foreground);
        style.editor_active_line = Some(c.active_line);
        style.editor_line_number = Some(c.muted);
        style.editor_active_line_number = Some(c.foreground);
    });
}

fn apply(palette: &Palette, theme: &mut ThemeColor) {
    let c = palette.colors();
    theme.background = c.editor;
    theme.foreground = c.foreground;
    theme.border = c.border;
    theme.muted = c.panel;
    theme.muted_foreground = c.muted;
    theme.sidebar = c.panel;
    theme.sidebar_border = c.border;
    theme.sidebar_foreground = c.foreground;
    theme.title_bar = c.bar;
    theme.title_bar_border = c.border;
    theme.tab_bar = c.bar;
    theme.tab = c.bar;
    theme.tab_active = c.editor;
    theme.tab_foreground = c.muted;
    theme.tab_active_foreground = c.foreground;
    theme.status_bar = c.bar;
    theme.status_bar_border = c.border;
    theme.list = c.panel;
    theme.list_head = c.bar;
    theme.list_hover = c.hover;
    theme.list_active = c.selected;
    theme.list_active_border = c.accent;
    theme.primary = c.accent;
    theme.ring = c.accent;
    theme.link = c.accent;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(hex: u32) -> f64 {
        let channel = |shift: u32| {
            let v = f64::from((hex >> shift) & 0xff) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    /// WCAG 2.1 contrast ratio.
    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn text_meets_wcag_on_every_surface() {
        for (name, p) in [("light", LIGHT), ("dark", DARK)] {
            for (surface, bg) in [
                ("editor", p.editor),
                ("panel", p.panel),
                ("bar", p.bar),
                ("hover", p.hover),
                ("selected", p.selected),
                ("active line", p.active_line),
            ] {
                let fg = contrast(p.foreground, bg);
                let muted = contrast(p.muted, bg);
                assert!(fg >= 7.0, "{name} foreground on {surface}: {fg:.2}");
                assert!(muted >= 4.5, "{name} muted on {surface}: {muted:.2}");
            }
            for (status, color) in [
                ("added", p.added),
                ("modified", p.modified),
                ("deleted", p.deleted),
                ("untracked", p.untracked),
                ("conflict", p.conflict),
            ] {
                let ratio = contrast(color, p.panel);
                assert!(ratio >= 4.5, "{name} {status} on panel: {ratio:.2}");
            }
        }
    }

    #[test]
    fn hover_and_selection_are_distinct() {
        for p in [LIGHT, DARK] {
            assert_ne!(p.hover, p.selected);
            assert_ne!(p.hover, p.panel);
            assert_ne!(p.selected, p.panel);
        }
    }
}
