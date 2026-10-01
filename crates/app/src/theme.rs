//! Design tokens: the only place with literal sizes and colors.
//!
//! Sizes sit on a 2/4 px grid modelled on VS Code's workbench. Colors are semantic; the dark
//! palette is Solarized Dark as VS Code's built-in theme maps it, the light palette is Nord
//! Light. Both are also written into GPUI Kit's theme (and its syntax theme) so Kit components
//! such as the Editor match.

use gpui_kit::{
    App, Hsla, Pixels, Window, WindowAppearance,
    component::{Theme, ThemeColor, ThemeMode},
    px, rgb,
};
use serde_json::{Value, json};

// Spacing (4 px grid). Kit's Tailwind-style helpers (`gap_1` = 4, `px_2` = 8, `px_3` = 12,
// `p_4` = 16) are already on the grid; constants exist only where arithmetic is needed.

// Heights: every list row shares one height; every bar shares another.
pub const ROW_HEIGHT: Pixels = px(22.);
pub const TAB_HEIGHT: Pixels = px(35.);
pub const BREADCRUMB_HEIGHT: Pixels = px(22.);
pub const TAB_CLOSE: Pixels = px(20.);
pub const DIRTY_DOT: Pixels = px(8.);
pub const KEYCAP: Pixels = px(20.);
pub const WELCOME_WIDTH: Pixels = px(340.);
pub const LOGO_TEXT: Pixels = px(180.);
pub const TITLE_HEIGHT: Pixels = px(38.);
pub const ACTIVITY_HEIGHT: Pixels = px(35.);
pub const ACTIVITY_ITEM: Pixels = px(28.);
pub const SIDEBAR_TITLE_HEIGHT: Pixels = px(35.);
pub const SECTION_HEIGHT: Pixels = px(22.);
/// Explorer tree geometry, measured from the owner's VS Code screenshot (rows are 22).
pub const TREE_BASE: Pixels = px(12.);
pub const TREE_STEP: Pixels = px(8.);
pub const ROW_INSET: Pixels = px(2.);
pub const GUIDE_WIDTH: Pixels = px(1.);
pub const DECORATION_DOT: Pixels = px(6.);
pub const DECORATION_WIDTH: Pixels = px(16.);
pub const BADGE_SIZE: Pixels = px(15.);
pub const BADGE_OFFSET: Pixels = px(-3.);
pub const COMMAND_CENTER_HEIGHT: Pixels = px(22.);
pub const COMMAND_CENTER_WIDTH: Pixels = px(600.);
pub const QUICK_OPEN_WIDTH: Pixels = px(600.);
pub const QUICK_OPEN_TOP: Pixels = px(6.);
pub const QUICK_OPEN_ROWS: usize = 12;
pub const RADIUS: Pixels = px(4.);
pub const RADIUS_LARGE: Pixels = px(6.);
pub const STATUS_HEIGHT: Pixels = px(24.);
/// Active tab indicator thickness (VS Code's tab.activeBorderTop).
pub const INDICATOR: Pixels = px(1.);

// Widths.
pub const TWISTY_WIDTH: Pixels = px(16.);
pub const SIDEBAR_WIDTH: Pixels = px(280.);
pub const SIDEBAR_MIN: Pixels = px(200.);
pub const SIDEBAR_MAX: Pixels = px(520.);
pub const EDITOR_MIN: Pixels = px(320.);
pub const EDITOR_MAX: Pixels = px(4000.);

// Icons and text.
pub const ICON_SIZE: Pixels = px(14.);
pub const SMALL_ICON_SIZE: Pixels = px(12.);
pub const FILE_ICON_SIZE: Pixels = px(16.);
pub const TEXT_BODY: Pixels = px(13.);
pub const TEXT_CAPTION: Pixels = px(12.);
pub const TEXT_SECTION: Pixels = px(11.);
pub const TEXT_BADGE: Pixels = px(9.);

pub const DIFF_COLUMN_WIDTH: Pixels = px(7.8);
pub const DIFF_GUTTER: Pixels = px(52.);
pub const DIFF_TEXT: Pixels = px(13.);
pub const SCM_NOTICE_MAX: Pixels = px(100.);
pub const COMMIT_HEIGHT: Pixels = px(60.);

// Window geometry.
pub const WINDOW_WIDTH: Pixels = px(1280.);
pub const WINDOW_HEIGHT: Pixels = px(800.);
pub const WINDOW_MIN_WIDTH: Pixels = px(760.);
pub const WINDOW_MIN_HEIGHT: Pixels = px(480.);
pub const WINDOW_ORIGIN: Pixels = px(60.);
pub const WINDOW_CASCADE: Pixels = px(28.);
pub const TRAFFIC_LIGHT_X: Pixels = px(12.);
pub const TRAFFIC_LIGHT_Y: Pixels = px(12.);

/// Semantic colors as 0xRRGGBB.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Editor, active tab, welcome page.
    pub editor: u32,
    /// Sidebar and status bar.
    pub panel: u32,
    /// Title bar.
    pub title: u32,
    /// Tab strip and inactive tabs.
    pub tabs: u32,
    pub border: u32,
    pub hover: u32,
    /// Selected row; distinct from hover, drawn with `selected_fg`.
    pub selected: u32,
    pub selected_fg: u32,
    /// Editor current-line highlight.
    pub active_line: u32,
    /// Editor text selection.
    pub selection: u32,
    pub caret: u32,
    pub foreground: u32,
    /// Secondary UI text; ≥ 4.5:1 on every surface except `selected`.
    pub muted: u32,
    pub accent: u32,
    pub badge: u32,
    pub badge_fg: u32,
    /// Keyboard shortcut key caps.
    pub keycap: u32,
    /// The faint welcome wordmark.
    pub logo: u32,
    pub indent_guide: u32,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflict: u32,
}

/// Solarized Dark, mapped like VS Code's built-in theme (sidebar #00212B, editor #002B36,
/// title #002C39, list selection #005A6F, git decorations from VS Code's defaults). Confirmed
/// against the owner's screenshot allowing for its Display P3 shift. `deleted` is lightened
/// from #C74E39 and inactive tabs use #003847 instead of #004052 so text keeps 4.5:1.
pub const DARK: Palette = Palette {
    editor: 0x002b36,
    panel: 0x00212b,
    title: 0x002c39,
    tabs: 0x003847,
    border: 0x073642,
    hover: 0x003846,
    selected: 0x005a6f,
    selected_fg: 0xffffff,
    active_line: 0x073642,
    selection: 0x274642,
    caret: 0xd30102,
    foreground: 0xcccccc,
    muted: 0x93a1a1,
    accent: 0x2aa198,
    badge: 0x047aa6,
    badge_fg: 0xffffff,
    keycap: 0x103a44,
    logo: 0x00222b,
    indent_guide: 0x0e4250,
    added: 0x81b88b,
    modified: 0xe2c08d,
    deleted: 0xe8735f,
    untracked: 0x73c991,
    conflict: 0xe4676b,
};

/// Nord Light: snow storm backgrounds, polar night text, frost accents. Aurora colors are
/// darkened for git decorations so they keep 4.5:1 on the light surfaces.
pub const LIGHT: Palette = Palette {
    editor: 0xeceff4,
    panel: 0xe5e9f0,
    title: 0xe5e9f0,
    tabs: 0xe5e9f0,
    border: 0xd8dee9,
    hover: 0xdce2eb,
    selected: 0xcbd8e8,
    selected_fg: 0x2e3440,
    active_line: 0xe5e9f0,
    selection: 0xd8dee9,
    caret: 0x5e81ac,
    foreground: 0x2e3440,
    muted: 0x4c566a,
    accent: 0x5e81ac,
    badge: 0x4c6a94,
    badge_fg: 0xffffff,
    keycap: 0xd8dee9,
    logo: 0xdfe4ec,
    indent_guide: 0xd0d6e0,
    added: 0x3f6b2d,
    modified: 0x8a5226,
    deleted: 0x99353f,
    untracked: 0x3f6b2d,
    conflict: 0x7a4e73,
};

/// Syntax colors (tree-sitter capture → color, style) in the same Kit JSON shape as its theme
/// files. Solarized keeps its canonical comment color (#586E75, below 4.5:1 by design).
const SOLARIZED_SYNTAX: &[(&str, u32, Option<&str>)] = &[
    ("attribute", 0x93a1a1, None),
    ("boolean", 0xb58900, None),
    ("comment", 0x586e75, Some("italic")),
    ("comment.doc", 0x586e75, Some("italic")),
    ("constant", 0xb58900, None),
    ("constructor", 0xcb4b16, None),
    ("embedded", 0x839496, None),
    ("enum", 0xb58900, None),
    ("function", 0x268bd2, None),
    // Diff additions / deletions (see diff_syntax.rs).
    ("hint", 0xdc322f, None),
    ("predictive", 0x859900, None),
    ("keyword", 0x859900, None),
    ("label", 0x6c71c4, None),
    ("link_text", 0x268bd2, None),
    ("link_uri", 0x2aa198, None),
    ("number", 0xd33682, None),
    ("operator", 0x859900, None),
    ("preproc", 0xcb4b16, None),
    ("primary", 0x839496, None),
    ("property", 0x839496, None),
    ("punctuation", 0x839496, None),
    ("punctuation.bracket", 0x839496, None),
    ("punctuation.delimiter", 0x839496, None),
    ("punctuation.list_marker", 0xcb4b16, None),
    ("punctuation.special", 0xdc322f, None),
    ("string", 0x2aa198, None),
    ("string.escape", 0xdc322f, None),
    ("string.regex", 0xdc322f, None),
    ("string.special", 0x2aa198, None),
    ("string.special.symbol", 0x2aa198, None),
    ("tag", 0x268bd2, None),
    ("text.literal", 0x2aa198, None),
    ("title", 0x268bd2, Some("bold")),
    ("type", 0xcb4b16, None),
    ("variable", 0x839496, None),
    ("variable.special", 0x268bd2, None),
    ("variant", 0xb58900, None),
];

const NORD_LIGHT_SYNTAX: &[(&str, u32, Option<&str>)] = &[
    ("attribute", 0x96593a, None),
    ("boolean", 0x81587a, None),
    ("comment", 0x5f6b82, Some("italic")),
    ("comment.doc", 0x5f6b82, Some("italic")),
    ("constant", 0x81587a, None),
    ("constructor", 0x2f6f6d, None),
    ("embedded", 0x2e3440, None),
    ("enum", 0x2f6f6d, None),
    ("function", 0x2f6b8f, None),
    ("hint", 0x99353f, None),
    ("predictive", 0x4f7433, None),
    ("keyword", 0x4c6a94, Some("bold")),
    ("label", 0x81587a, None),
    ("link_text", 0x4c6a94, None),
    ("link_uri", 0x2f6b8f, None),
    ("number", 0x81587a, None),
    ("operator", 0x4c6a94, None),
    ("preproc", 0x4c6a94, None),
    ("primary", 0x2e3440, None),
    ("property", 0x2f6b8f, None),
    ("punctuation", 0x4c566a, None),
    ("punctuation.bracket", 0x4c566a, None),
    ("punctuation.delimiter", 0x4c566a, None),
    ("punctuation.list_marker", 0x4c6a94, None),
    ("punctuation.special", 0x96593a, None),
    ("string", 0x4f7433, None),
    ("string.escape", 0x96593a, None),
    ("string.regex", 0x96593a, None),
    ("string.special", 0x4f7433, None),
    ("string.special.symbol", 0x4f7433, None),
    ("tag", 0x4c6a94, None),
    ("text.literal", 0x4f7433, None),
    ("title", 0x4c6a94, Some("bold")),
    ("type", 0x2f6f6d, None),
    ("variable", 0x2e3440, None),
    ("variable.special", 0x4c6a94, None),
    ("variant", 0x2f6f6d, None),
];

fn syntax_json(entries: &[(&str, u32, Option<&str>)]) -> Value {
    let mut map = serde_json::Map::new();
    for (name, color, style) in entries {
        let mut entry = json!({ "color": format!("#{color:06x}") });
        match style {
            Some("bold") => entry["font_weight"] = json!(700),
            Some(style) => entry["font_style"] = json!(style),
            None => {}
        }
        map.insert((*name).to_owned(), entry);
    }
    Value::Object(map)
}

/// The palette converted for drawing.
#[derive(Clone, Copy)]
#[allow(dead_code)] // Badge, key cap, logo and guide colors are used by the upcoming workbench.
pub struct Colors {
    pub editor: Hsla,
    pub panel: Hsla,
    pub title: Hsla,
    pub tabs: Hsla,
    pub border: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub selected_fg: Hsla,
    pub foreground: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub badge: Hsla,
    pub badge_fg: Hsla,
    pub keycap: Hsla,
    pub logo: Hsla,
    pub indent_guide: Hsla,
    pub added: Hsla,
    pub modified: Hsla,
    pub deleted: Hsla,
    pub untracked: Hsla,
    pub conflict: Hsla,
    /// VS Code's command center: foreground at 5 % / 20 % opacity.
    pub diff_added: Hsla,
    pub diff_deleted: Hsla,
    pub command_bg: Hsla,
    pub command_border: Hsla,
}

fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Palette {
    fn colors(&self) -> Colors {
        Colors {
            editor: hsla(self.editor),
            panel: hsla(self.panel),
            title: hsla(self.title),
            tabs: hsla(self.tabs),
            border: hsla(self.border),
            hover: hsla(self.hover),
            selected: hsla(self.selected),
            selected_fg: hsla(self.selected_fg),
            foreground: hsla(self.foreground),
            muted: hsla(self.muted),
            accent: hsla(self.accent),
            badge: hsla(self.badge),
            badge_fg: hsla(self.badge_fg),
            keycap: hsla(self.keycap),
            logo: hsla(self.logo),
            indent_guide: hsla(self.indent_guide),
            added: hsla(self.added),
            modified: hsla(self.modified),
            deleted: hsla(self.deleted),
            untracked: hsla(self.untracked),
            conflict: hsla(self.conflict),
            diff_added: hsla(self.added).opacity(0.12),
            diff_deleted: hsla(self.deleted).opacity(0.12),
            command_bg: hsla(self.foreground).opacity(0.05),
            command_border: hsla(self.foreground).opacity(0.2),
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
    let (palette, syntax) = if mode.is_dark() {
        (DARK, SOLARIZED_SYNTAX)
    } else {
        (LIGHT, NORD_LIGHT_SYNTAX)
    };
    let syntax = serde_json::from_value(syntax_json(syntax));
    Theme::update(cx, |theme| {
        apply(&palette, &mut theme.colors);
        // The Editor paints from the syntax theme's own style block, not from ThemeColor.
        let style = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        style.editor_background = Some(hsla(palette.editor));
        style.editor_foreground = Some(hsla(if mode.is_dark() {
            0x839496
        } else {
            palette.foreground
        }));
        style.editor_active_line = Some(hsla(palette.active_line));
        style.editor_line_number = Some(hsla(palette.muted));
        style.editor_active_line_number = Some(hsla(palette.foreground));
        match syntax {
            Ok(syntax) => style.syntax = syntax,
            Err(error) => eprintln!("event=syntax_theme_invalid error={error}"),
        }
    });
}

fn apply(palette: &Palette, theme: &mut ThemeColor) {
    let c = palette.colors();
    theme.background = c.editor;
    theme.foreground = c.foreground;
    theme.border = c.border;
    theme.muted = c.panel;
    theme.muted_foreground = c.muted;
    theme.accent = c.hover;
    theme.accent_foreground = c.foreground;
    theme.secondary = c.hover;
    theme.secondary_hover = c.hover;
    theme.secondary_active = c.selected;
    theme.secondary_foreground = c.foreground;
    theme.sidebar = c.panel;
    theme.sidebar_border = c.border;
    theme.sidebar_foreground = c.foreground;
    theme.title_bar = c.title;
    theme.title_bar_border = c.border;
    theme.tab_bar = c.tabs;
    theme.tab = c.tabs;
    theme.tab_active = c.editor;
    theme.tab_foreground = c.muted;
    theme.tab_active_foreground = c.foreground;
    theme.status_bar = c.panel;
    theme.status_bar_border = c.border;
    theme.list = c.panel;
    theme.list_head = c.panel;
    theme.list_hover = c.hover;
    theme.list_active = c.selected;
    theme.list_active_border = c.accent;
    theme.popover = c.panel;
    theme.popover_foreground = c.foreground;
    theme.input = c.border;
    theme.primary = c.badge;
    theme.primary_hover = c.badge;
    theme.primary_active = c.badge;
    theme.primary_foreground = c.badge_fg;
    theme.button_primary = c.badge;
    theme.button_primary_hover = c.badge;
    theme.button_primary_active = c.badge;
    theme.button_primary_foreground = c.badge_fg;
    theme.ring = c.accent;
    theme.link = c.accent;
    theme.selection = hsla(palette.selection);
    theme.caret = hsla(palette.caret);
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
        for (name, p) in [("dark", DARK), ("light", LIGHT)] {
            for (surface, bg) in [
                ("editor", p.editor),
                ("panel", p.panel),
                ("title", p.title),
                ("tabs", p.tabs),
                ("hover", p.hover),
                ("active line", p.active_line),
                ("keycap", p.keycap),
            ] {
                let fg = contrast(p.foreground, bg);
                let muted = contrast(p.muted, bg);
                assert!(fg >= 7.0, "{name} foreground on {surface}: {fg:.2}");
                assert!(muted >= 4.5, "{name} muted on {surface}: {muted:.2}");
            }
            let selected = contrast(p.selected_fg, p.selected);
            assert!(selected >= 7.0, "{name} selected row: {selected:.2}");
            let badge = contrast(p.badge_fg, p.badge);
            assert!(badge >= 4.5, "{name} badge: {badge:.2}");
            for (status, color) in [
                ("added", p.added),
                ("modified", p.modified),
                ("deleted", p.deleted),
                ("untracked", p.untracked),
                ("conflict", p.conflict),
            ] {
                for (surface, bg) in [("panel", p.panel), ("editor", p.editor)] {
                    let ratio = contrast(color, bg);
                    assert!(ratio >= 4.5, "{name} {status} on {surface}: {ratio:.2}");
                }
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

    #[test]
    fn syntax_themes_parse() {
        for entries in [SOLARIZED_SYNTAX, NORD_LIGHT_SYNTAX] {
            let value = syntax_json(entries);
            let theme: Result<gpui_kit::component::highlighter::SyntaxColors, _> =
                serde_json::from_value(value);
            let theme = theme.unwrap();
            assert!(theme.style("keyword").is_some());
            assert!(theme.style("comment").is_some());
        }
    }

    #[test]
    fn light_syntax_is_readable() {
        for (name, color, _) in NORD_LIGHT_SYNTAX {
            let ratio = contrast(*color, LIGHT.editor);
            assert!(ratio >= 4.5, "nord light {name}: {ratio:.2}");
        }
    }
}
