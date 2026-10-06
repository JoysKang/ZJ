//! Design tokens: the only place with literal sizes and colors.
//!
//! Sizes sit on a 2/4 px grid modelled on VS Code's workbench. Colors are semantic; the dark
//! palette is Solarized Dark as VS Code's built-in theme maps it, the light palette is Nord
//! Light. Both are also written into GPUI Kit's theme (and its syntax theme) so Kit components
//! such as the Editor match.

use gpui_kit::{
    App, Hsla, Pixels, Rems, Window, WindowAppearance,
    component::{Theme, ThemeColor, ThemeMode},
    px, rems, rgb,
};
use serde_json::{Value, json};

// Spacing (4 px grid). Kit's Tailwind-style helpers (`gap_1` = 4, `px_2` = 8, `px_3` = 12,
// `p_4` = 16) are already on the grid; constants exist only where arithmetic is needed.

// Heights: every list row shares one height; every bar shares another. The owner asked for
// text one step larger than VS Code's 13 px, so rows grow from 22 to 24 on the same grid.
pub const ROW_HEIGHT: Pixels = px(24.);
/// How far beyond the viewport the Source Control list lays out rows (smooth scrolling).
pub const SCM_LIST_OVERDRAW: Pixels = px(200.);
pub const TAB_HEIGHT: Pixels = px(36.);
pub const TAB_CLOSE: Pixels = px(20.);
pub const DIRTY_DOT: Pixels = px(8.);
pub const KEYCAP: Pixels = px(20.);
pub const WELCOME_WIDTH: Pixels = px(340.);
/// Welcome page logo (`assets/logo/zj-sprig*.svg`, 640 × 600 artwork at a quarter scale) and
/// its cursor, placed where the artwork's `#zj-cursor` rect was (x 678, y 612, 26 × 190, from
/// the viewBox origin 190, 250).
pub const LOGO_WIDTH: Pixels = px(160.);
pub const LOGO_HEIGHT: Pixels = px(150.);
pub const LOGO_CURSOR_LEFT: Pixels = px(122.);
pub const LOGO_CURSOR_TOP: Pixels = px(90.5);
pub const LOGO_CURSOR_WIDTH: Pixels = px(6.5);
pub const LOGO_CURSOR_HEIGHT: Pixels = px(47.5);
pub const LOGO_CURSOR_RADIUS: Pixels = px(3.25);
pub const TITLE_HEIGHT: Pixels = px(38.);
pub const ACTIVITY_HEIGHT: Pixels = px(36.);
pub const ACTIVITY_ITEM: Pixels = px(28.);
pub const SIDEBAR_TITLE_HEIGHT: Pixels = px(36.);
pub const SECTION_HEIGHT: Pixels = px(24.);
/// Explorer tree geometry, measured from the owner's VS Code screenshot.
pub const TREE_BASE: Pixels = px(12.);
pub const TREE_STEP: Pixels = px(8.);
pub const ROW_INSET: Pixels = px(4.);
/// The Search view's replace chevron sits in the left margin, outside the inputs' edge.
pub const SEARCH_CHEVRON_OUTDENT: Pixels = px(-10.);
pub const GUIDE_WIDTH: Pixels = px(1.);
pub const DECORATION_DOT: Pixels = px(6.);
/// A folder's decoration dot is the file color, lighter.
pub const DECORATION_DOT_ALPHA: f32 = 0.7;
/// Items that cannot be used right now (waiting for a Git write, a one-sided review action).
pub const DIMMED_OPACITY: f32 = 0.5;
/// SGR 2 (faint) terminal text.
pub const TERMINAL_DIM_ALPHA: f32 = 0.66;
pub const DECORATION_WIDTH: Pixels = px(16.);
pub const BADGE_SIZE: Pixels = px(16.);
pub const BADGE_OFFSET: Pixels = px(-3.);
pub const COMMAND_CENTER_HEIGHT: Pixels = px(24.);
pub const COMMAND_CENTER_WIDTH: Pixels = px(600.);
pub const QUICK_OPEN_WIDTH: Pixels = px(600.);
pub const QUICK_OPEN_TOP: Pixels = px(8.);
pub const QUICK_OPEN_ROWS: usize = 12;
pub const RADIUS: Pixels = px(4.);
pub const RADIUS_LARGE: Pixels = px(6.);
/// The editor's floating find / replace widget (VS Code: 419 px wide, inputs about 220 px).
pub const FIND_WIDTH: Pixels = px(440.);
pub const FIND_INPUT_WIDTH: Pixels = px(236.);
/// Clear of the editor's vertical scrollbar.
pub const FIND_RIGHT: Pixels = px(16.);
pub const FIND_COUNT_WIDTH: Pixels = px(112.);
/// Buttons are 22 px with 16 px icons; the toggles inside the inputs 20 px.
pub const FIND_BUTTON: Pixels = px(22.);
pub const FIND_TOGGLE: Pixels = px(20.);
pub const FIND_SHADOW_BLUR: Pixels = px(8.);
/// Strikethrough on replaced text in the Search view's previews.
pub const STRIKE: Pixels = px(1.);
pub const FIND_SHADOW_Y: Pixels = px(2.);
pub const STATUS_HEIGHT: Pixels = px(24.);
/// Active tab indicator thickness (VS Code's tab.activeBorderTop).
pub const INDICATOR: Pixels = px(1.);

// Widths.
pub const TWISTY_WIDTH: Pixels = px(16.);
pub const SIDEBAR_WIDTH: Pixels = px(300.);
pub const SIDEBAR_MIN: Pixels = px(200.);
pub const SIDEBAR_MAX: Pixels = px(520.);
pub const EDITOR_MIN: Pixels = px(320.);
pub const EDITOR_MAX: Pixels = px(4000.);

// Icons and text.
pub const ICON_SIZE: Pixels = px(16.);
pub const SMALL_ICON_SIZE: Pixels = px(14.);
pub const FILE_ICON_SIZE: Pixels = px(16.);
pub const TEXT_BODY: Pixels = px(14.);
pub const TEXT_CAPTION: Pixels = px(13.);
pub const TEXT_SECTION: Pixels = px(12.);
pub const TEXT_BADGE: Pixels = px(10.);
/// The editor's "file changed on disk" banner.
pub const BANNER_HEIGHT: Pixels = px(30.);

// Diff editor (VS Code: 5-digit line numbers, +/- indicators). Text and rows follow the
// editor font size, see [`diff_metrics`].
pub const DIFF_GUTTER: Pixels = px(48.);
pub const DIFF_INDICATOR: Pixels = px(20.);
pub const DIFF_TEXT_END: Pixels = px(32.);
/// Filler stripes: (line width, gap) in pixels; their sum divides every diff row height so the
/// pattern continues across rows.
pub const DIFF_HATCH: (f32, f32) = (1., 4.);
/// Monospace advance as a fraction of the font size (Menlo, SF Mono, DejaVu Sans Mono).
const MONO_ADVANCE: f32 = 0.6;

/// Diff editor text size, row height and character width for an editor font size. Rows are
/// about 1.5× the font, rounded to the hatch period so filler stripes line up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiffMetrics {
    pub text: Pixels,
    pub row: Pixels,
    pub column: Pixels,
}

pub fn diff_metrics(font: Pixels) -> DiffMetrics {
    let font = f32::from(font);
    let period = DIFF_HATCH.0 + DIFF_HATCH.1;
    DiffMetrics {
        text: px(font),
        row: px(((font * 1.5 / period).round() * period).max(3. * period)),
        column: px(font * MONO_ADVANCE),
    }
}
/// The terminal panel: its starting / smallest height, the grid's inset, the header buttons'
/// gap.
pub const TERMINAL_HEIGHT: Pixels = px(280.);
pub const TERMINAL_MIN: Pixels = px(80.);
pub const TERMINAL_MAX: Pixels = px(4000.);
pub const TERMINAL_PAD_X: Pixels = px(12.);
pub const TERMINAL_PAD_Y: Pixels = px(4.);
/// The cursor's width as a bar and its height as an underline.
pub const TERMINAL_CURSOR_BAR: Pixels = px(2.);

/// The Markdown preview: VS Code's preview column (`max-width: 882px`; its 26 px side padding
/// is 24 here, on the 4 px grid),
/// the space around each block, the clickable room after the last block, and how far the
/// list lays out beyond the viewport.
pub const MD_MAX_WIDTH: Pixels = px(882.);
pub const MD_PAD_X: Pixels = px(24.);
pub const MD_BLOCK_GAP: Pixels = px(8.);
pub const MD_EDIT_PAD: Pixels = px(4.);
pub const MD_TAIL: Pixels = px(160.);
pub const MD_OVERDRAW: Pixels = px(800.);
/// Rows the block editor grows to before it scrolls.
pub const MD_EDIT_MAX_ROWS: usize = 400;

/// Terminal row height for an editor font size (VS Code's `lineHeight` 1 is about 1.2× the
/// font; a little more reads better at 14 px).
pub fn terminal_line_height(font: Pixels) -> Pixels {
    px((f32::from(font) * 1.3).round())
}

/// Overview ruler at the right edge of the diff editor, and its smallest marker.
pub const DIFF_RULER: Pixels = px(14.);
pub const DIFF_RULER_MIN: Pixels = px(3.);
pub const SCM_NOTICE_MAX: Pixels = px(100.);
/// Source Control repository rows: the branch line under the name, and wrapped lines.
pub const SCM_DETAIL_LINE: Pixels = px(20.);
/// Centers a `SCM_DETAIL_LINE` line inside a `ROW_HEIGHT` row: derived from the two, not a
/// spacing choice (the one value off the 4 px grid).
pub const SCM_LINE_PAD: Pixels = px((24. - 20.) / 2.);

// Git Graph (editor-area commit graph).
/// Horizontal space per branch lane.
pub const GRAPH_LANE: Pixels = px(14.);
/// Diameter of a commit node; HEAD adds a ring two strokes out.
pub const GRAPH_NODE: Pixels = px(8.);
/// Lane line thickness.
pub const GRAPH_STROKE: Pixels = px(1.6);
/// Author column; hashes and dates take what remains.
pub const GRAPH_AUTHOR: Pixels = px(140.);
pub const GRAPH_TIME: Pixels = px(84.);
/// The details view under the graph never takes more than this much height.
pub const GRAPH_DETAILS_MAX: Pixels = px(300.);

// Agent panel (docs/adr/0004, design in ZJ-agent-ui-design.md).
/// Below this width the panel switches to the compact density (direction C).
pub const AGENT_COMPACT_WIDTH: Pixels = px(360.);
pub const AGENT_PANEL_MIN: Pixels = px(crate::settings::AGENT_PANEL_WIDTH_MIN);
pub const AGENT_PANEL_MAX: Pixels = px(crate::settings::AGENT_PANEL_WIDTH_MAX);
pub const AGENT_GLYPH: Pixels = px(18.);
pub const AGENT_GLYPH_SMALL: Pixels = px(16.);
pub const AGENT_GLYPH_ICON: Pixels = px(12.);
pub const AGENT_GLYPH_RADIUS: Pixels = px(5.);
pub const STATUS_DOT: Pixels = px(8.);
/// The pale halo around the waiting-for-approval dot.
pub const STATUS_HALO: Pixels = px(14.);
pub const AGENT_TOOL_ROW: Pixels = px(30.);
pub const AGENT_CARD_HEAD: Pixels = px(32.);
pub const AGENT_FILE_ROW: Pixels = px(26.);
pub const AGENT_FILE_INDENT: Pixels = px(24.);
pub const AGENT_CHIP: Pixels = px(20.);
pub const AGENT_COMPOSER_BAR: Pixels = px(34.);
pub const AGENT_COMPOSER_MIN: Pixels = px(44.);
pub const AGENT_RING: Pixels = px(16.);
pub const AGENT_RING_STROKE: Pixels = px(2.5);
pub const AGENT_PLAN_BAR: Pixels = px(3.);
pub const AGENT_PLAN_BAR_WIDTH: Pixels = px(160.);
pub const AGENT_LINE: Pixels = px(22.);
/// Between the paragraphs, lists and code blocks of a reply (8 px at the default rem).
pub const AGENT_PARAGRAPH_GAP: Rems = rems(0.5);
pub const AGENT_FILTER_CHIP: Pixels = px(22.);
pub const AGENT_SEARCH_INPUT: Pixels = px(28.);
/// 接受 / 拒绝 at the top right of a change block: kept clear of the overlay scrollbar.
pub const AGENT_HUNK_ACTIONS_INSET: Pixels = px(16.);
/// Two-line session rows in the history list.
pub const AGENT_HISTORY_ROW: Pixels = px(52.);
pub const AGENT_HISTORY_ROW_COMPACT: Pixels = px(32.);
pub const AGENT_STRIP_ROW: Pixels = px(32.);
pub const AGENT_GROUP_HEADER: Pixels = px(28.);
pub const AGENT_MENTION_ROWS: usize = 8;
/// Thread bubbles and cards never get wider than this, whatever the panel width.
pub const AGENT_THREAD_MAX: Pixels = px(760.);
/// ⌘J search overlay.
pub const AGENT_OVERLAY_WIDTH: Pixels = px(680.);
pub const AGENT_OVERLAY_TOP: Pixels = px(8.);
/// The ⌘J overlay's footer.
pub const AGENT_OVERLAY_FOOTER: Pixels = px(32.);
pub const AGENT_OVERLAY_INPUT: Pixels = px(44.);
pub const AGENT_OVERLAY_ROW: Pixels = px(80.);
pub const AGENT_OVERLAY_ROWS: usize = 7;
pub const RADIUS_OVERLAY: Pixels = px(8.);
pub const TEXT_OVERLAY_INPUT: Pixels = px(15.);
/// The diff editor's agent toolbar ("Claude Code 建议的修改 … 接受此文件").
pub const AGENT_REVIEW_BAR: Pixels = px(32.);

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
    /// Source Control's 提交 button (green, as in VS Code's multi-repository view) and its
    /// text; `commit_hover` is the hovered / pressed shade.
    pub commit: u32,
    pub commit_fg: u32,
    pub commit_hover: u32,
    /// Keyboard shortcut key caps.
    pub keycap: u32,
    /// The welcome logo's cursor (Nord aurora red in both themes, as in the app icon).
    pub logo_cursor: u32,
    /// The current find match (VS Code `editor.findMatchBackground`), drawn at
    /// `find_current_alpha`; the other matches use the accent.
    pub find_current: u32,
    pub find_current_alpha: f32,
    /// Floating widgets' drop shadow (VS Code `widget.shadow`), black at this opacity.
    pub shadow_alpha: f32,
    pub indent_guide: u32,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    pub untracked: u32,
    pub conflict: u32,
    /// Default code color (Solarized base0 / Nord polar night).
    pub code: u32,
    /// Diff hues: Solarized green/red, Nord aurora green/red.
    pub diff_green: u32,
    pub diff_red: u32,
    pub diff_line_alpha: f32,
    pub diff_text_alpha: f32,
    /// Agent panel: waiting for approval (yellow), done but unread (blue), running spinner.
    pub attention: u32,
    pub attention_halo: u32,
    pub unread: u32,
    pub running: u32,
    /// Cards inside the agent panel and their borders; `strong_border` frames inputs.
    pub card: u32,
    pub card_border: u32,
    pub strong_border: u32,
    /// Search hit highlight in the session search (background color and alpha, text).
    pub mark: u32,
    pub mark_alpha: f32,
    pub mark_fg: u32,
    /// Monochrome agent glyphs: Claude, Codex, DeepSeek, Gemini, user-defined.
    pub glyph_claude: u32,
    pub glyph_codex: u32,
    pub glyph_deepseek: u32,
    pub glyph_gemini: u32,
    pub glyph_generic: u32,
    /// The terminal's 16 ANSI colors (black … white, then the bright ones).
    pub terminal: [u32; 16],
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
    // A leaf green darkened until white text reaches 4.5:1 (Solarized's #859900 reads as
    // olive at this size).
    commit: 0x4e7d1f,
    commit_fg: 0xffffff,
    commit_hover: 0x47731b,
    keycap: 0x103a44,
    logo_cursor: 0xbf616a,
    // Solarized yellow.
    find_current: 0xb58900,
    find_current_alpha: 0.5,
    shadow_alpha: 0.36,
    indent_guide: 0x0e4250,
    added: 0x81b88b,
    modified: 0xe2c08d,
    deleted: 0xe8735f,
    untracked: 0x73c991,
    conflict: 0xe4676b,
    code: 0x839496,
    diff_green: 0x859900,
    diff_red: 0xdc322f,
    diff_line_alpha: 0.16,
    diff_text_alpha: 0.38,
    attention: 0xb58900,
    attention_halo: 0xb58900,
    unread: 0x268bd2,
    running: 0x2aa198,
    card: 0x002b36,
    card_border: 0x0a3c49,
    strong_border: 0x0e4250,
    mark: 0xb58900,
    mark_alpha: 0.30,
    mark_fg: 0xf0d78c,
    glyph_claude: 0xcb4b16,
    glyph_codex: 0x268bd2,
    glyph_deepseek: 0x6c71c4,
    glyph_gemini: 0xd33682,
    glyph_generic: 0x93a1a1,
    // VS Code's Solarized Dark `terminal.ansi*`: the bright colors are Solarized's base tones.
    terminal: [
        0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x586e75,
        0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
    ],
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
    // Nord aurora green with polar night text (white on it would be 2:1).
    commit: 0xa3be8c,
    commit_fg: 0x2e3440,
    commit_hover: 0x97b47e,
    keycap: 0xd8dee9,
    logo_cursor: 0xbf616a,
    // Nord aurora yellow.
    find_current: 0xebcb8b,
    find_current_alpha: 0.8,
    shadow_alpha: 0.16,
    indent_guide: 0xd0d6e0,
    added: 0x3f6b2d,
    modified: 0x8a5226,
    deleted: 0x99353f,
    untracked: 0x3f6b2d,
    conflict: 0x7a4e73,
    code: 0x2e3440,
    diff_green: 0xa3be8c,
    diff_red: 0xbf616a,
    diff_line_alpha: 0.26,
    diff_text_alpha: 0.5,
    attention: 0x8a5226,
    attention_halo: 0xd08770,
    unread: 0x4c6a94,
    running: 0x4c6a94,
    card: 0xeceff4,
    card_border: 0xd3d9e4,
    strong_border: 0xc9d1de,
    mark: 0xebcb8b,
    mark_alpha: 0.55,
    mark_fg: 0x2e3440,
    glyph_claude: 0x96593a,
    glyph_codex: 0x4c6a94,
    glyph_deepseek: 0x81587a,
    glyph_gemini: 0x2f6f6d,
    glyph_generic: 0x4c566a,
    // Nord with the aurora colors darkened (as for the git decorations) so text in them stays
    // readable on snow storm; white is a light gray, as on any light terminal theme.
    terminal: [
        0x3b4252, 0x99353f, 0x3f6b2d, 0x8a5226, 0x4c6a94, 0x81587a, 0x2f6f6d, 0xd8dee9, 0x4c566a,
        0xbf616a, 0x4f7433, 0x96593a, 0x5e81ac, 0xb48ead, 0x2f6b8f, 0xe5e9f0,
    ],
};

/// Syntax colors (tree-sitter capture → color, style) in the same Kit JSON shape as its theme
/// files, following VS Code's Solarized Dark token colors (keyword/operator green, function and
/// property blue, type/constructor orange, constants yellow, numbers magenta, strings cyan).
/// Solarized keeps its canonical comment color (#586E75, below 4.5:1 by design). Captures a
/// table does not list fall back to their prefix (`function.method` → `function`).
const SOLARIZED_SYNTAX: &[(&str, u32, Option<&str>)] = &[
    ("attribute", 0x93a1a1, None),
    ("boolean", 0xb58900, None),
    ("comment", 0x586e75, Some("italic")),
    ("comment.doc", 0x586e75, Some("italic")),
    ("constant", 0xb58900, None),
    ("constructor", 0xcb4b16, None),
    ("embedded", 0x839496, None),
    ("emphasis", 0xd33682, Some("italic")),
    ("emphasis.strong", 0xd33682, Some("bold")),
    ("enum", 0xcb4b16, None),
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
    ("preproc", 0xb58900, None),
    ("primary", 0x839496, None),
    // VS Code maps variable.other.property / variable.other to blue.
    ("property", 0x268bd2, None),
    ("punctuation", 0x839496, None),
    ("punctuation.bracket", 0x839496, None),
    ("punctuation.delimiter", 0x839496, None),
    ("punctuation.list_marker", 0xcb4b16, None),
    ("punctuation.special", 0xdc322f, None),
    ("string", 0x2aa198, None),
    ("string.escape", 0xcb4b16, None),
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
    ("emphasis", 0x81587a, Some("italic")),
    ("emphasis.strong", 0x81587a, Some("bold")),
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
    pub commit: Hsla,
    pub commit_fg: Hsla,
    pub commit_hover: Hsla,
    pub keycap: Hsla,
    pub logo_cursor: Hsla,
    pub indent_guide: Hsla,
    pub added: Hsla,
    pub modified: Hsla,
    pub deleted: Hsla,
    pub untracked: Hsla,
    pub conflict: Hsla,
    /// VS Code's command center: foreground at 5 % / 20 % opacity.
    /// VS Code `diffEditor.insertedLineBackground` / `removedLineBackground`.
    pub diff_added: Hsla,
    pub diff_deleted: Hsla,
    /// VS Code `diffEditor.insertedTextBackground` / `removedTextBackground`.
    pub diff_added_text: Hsla,
    pub diff_deleted_text: Hsla,
    /// VS Code `diffEditor.diagonalFill` for filler rows.
    pub diff_filler: Hsla,
    /// The editor's default text color (syntax theme `editor.foreground`).
    pub code: Hsla,
    pub command_bg: Hsla,
    pub command_border: Hsla,
    /// Editor text selection (also selected diff lines).
    pub selection: Hsla,
    /// Search matches inside result previews (VS Code `editor.findMatchHighlightBackground`).
    pub find_match: Hsla,
    pub find_current: Hsla,
    pub shadow: Hsla,
    pub attention: Hsla,
    pub attention_bg: Hsla,
    pub attention_border: Hsla,
    pub unread: Hsla,
    pub running: Hsla,
    pub card: Hsla,
    pub card_border: Hsla,
    pub strong_border: Hsla,
    pub mark_bg: Hsla,
    pub mark_fg: Hsla,
    /// Small icon buttons inside list rows (VS Code `toolbar.hoverBackground` /
    /// `toolbar.activeBackground`): the text color, faint, so they show on a hovered row too.
    pub control_hover: Hsla,
    pub control_active: Hsla,
    /// Secondary text and tag chips on a selected (accent) row, and the focused composer's
    /// border.
    pub selected_muted: Hsla,
    pub selected_chip: Hsla,
    pub focus_border: Hsla,
    /// Selected filter chips: accent tint and border.
    pub chip_on: Hsla,
    pub chip_on_border: Hsla,
    /// Glyph color and its tinted tile, indexed like [`Glyph`](workspace_editor_agent::Glyph).
    pub glyphs: [(Hsla, Hsla); 5],
    /// The editor's "file changed on disk" banner: the git-modified hue, faint.
    pub banner: Hsla,
    /// Merge conflicts in the editor, as VS Code's merge.current / incoming backgrounds: the
    /// current side green, the incoming side blue, marker lines a faint gray on top.
    pub conflict_ours: Hsla,
    pub conflict_theirs: Hsla,
    pub conflict_marker: Hsla,
    /// Git Graph lane colors, cycled as lanes are created (lines and nodes, never text).
    pub graph_lanes: [Hsla; 8],
    /// The terminal draws on the editor background in the code color, as VS Code's does.
    pub terminal: crate::terminal::Palette,
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
            commit: hsla(self.commit),
            commit_fg: hsla(self.commit_fg),
            commit_hover: hsla(self.commit_hover),
            keycap: hsla(self.keycap),
            logo_cursor: hsla(self.logo_cursor),
            indent_guide: hsla(self.indent_guide),
            added: hsla(self.added),
            modified: hsla(self.modified),
            deleted: hsla(self.deleted),
            untracked: hsla(self.untracked),
            conflict: hsla(self.conflict),
            diff_added: hsla(self.diff_green).opacity(self.diff_line_alpha),
            diff_deleted: hsla(self.diff_red).opacity(self.diff_line_alpha),
            diff_added_text: hsla(self.diff_green).opacity(self.diff_text_alpha),
            diff_deleted_text: hsla(self.diff_red).opacity(self.diff_text_alpha),
            diff_filler: hsla(self.foreground).opacity(0.2),
            code: hsla(self.code),
            command_bg: hsla(self.foreground).opacity(0.05),
            command_border: hsla(self.foreground).opacity(0.2),
            selection: hsla(self.selection),
            find_match: hsla(self.accent).opacity(0.3),
            find_current: hsla(self.find_current).opacity(self.find_current_alpha),
            shadow: hsla(0x000000).opacity(self.shadow_alpha),
            attention: hsla(self.attention),
            attention_bg: hsla(self.attention_halo).opacity(0.14),
            attention_border: hsla(self.attention).opacity(0.45),
            unread: hsla(self.unread),
            running: hsla(self.running),
            card: hsla(self.card),
            card_border: hsla(self.card_border),
            strong_border: hsla(self.strong_border),
            mark_bg: hsla(self.mark).opacity(self.mark_alpha),
            mark_fg: hsla(self.mark_fg),
            chip_on: hsla(self.accent).opacity(0.16),
            selected_muted: hsla(self.selected_fg).opacity(0.82),
            selected_chip: hsla(self.selected_fg).opacity(0.14),
            focus_border: hsla(self.accent).opacity(0.7),
            control_hover: hsla(self.foreground).opacity(0.12),
            control_active: hsla(self.foreground).opacity(0.2),
            chip_on_border: hsla(self.accent).opacity(0.45),
            glyphs: [
                self.glyph_claude,
                self.glyph_codex,
                self.glyph_deepseek,
                self.glyph_gemini,
                self.glyph_generic,
            ]
            .map(|c| (hsla(c), hsla(c).opacity(0.18))),
            banner: hsla(self.modified).opacity(0.16),
            conflict_ours: hsla(self.diff_green).opacity(self.diff_line_alpha),
            conflict_theirs: hsla(self.unread).opacity(self.diff_line_alpha),
            conflict_marker: hsla(self.foreground).opacity(0.12),
            // Git Graph's palette works on both dark and light backgrounds.
            graph_lanes: [
                0x0085d9, 0xd9008f, 0x00d0a0, 0xd98500, 0xa000d9, 0x00b8d9, 0xd0a000, 0xd94545,
            ]
            .map(hsla),
            terminal: crate::terminal::Palette {
                ansi: self.terminal.map(hsla),
                foreground: hsla(self.code),
                background: hsla(self.editor),
                cursor: hsla(self.caret),
            },
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
    let editor_font = cx
        .try_global::<crate::settings::Settings>()
        .map_or(crate::settings::EDITOR_FONT_DEFAULT, |s| s.editor_font_size);
    Theme::update(cx, |theme| {
        theme.mono_font_size = px(editor_font);
        apply(&palette, &mut theme.colors);
        // The Editor paints from the syntax theme's own style block, not from ThemeColor.
        let style = &mut std::sync::Arc::make_mut(&mut theme.highlight_theme).style;
        style.editor_background = Some(hsla(palette.editor));
        style.editor_foreground = Some(hsla(palette.code));
        style.editor_active_line = Some(hsla(palette.active_line));
        style.editor_line_number = Some(hsla(palette.muted));
        style.editor_active_line_number = Some(hsla(palette.foreground));
        match syntax {
            Ok(syntax) => style.syntax = syntax,
            Err(error) => eprintln!("event=syntax_theme_invalid error={error}"),
        }
    });
}

/// The code editor's font size (Kit's editor reads `mono_font_size`).
pub fn apply_editor_font(size: f32, cx: &mut App) {
    Theme::update(cx, |theme| theme.mono_font_size = px(size));
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

/// The app's syntax theme on top of Kit's defaults, for tests that run without a window.
#[cfg(test)]
pub fn highlight_theme_for_tests(dark: bool) -> gpui_kit::component::highlighter::HighlightTheme {
    use gpui_kit::component::highlighter::HighlightTheme;
    let mut theme = if dark {
        (*HighlightTheme::default_dark()).clone()
    } else {
        (*HighlightTheme::default_light()).clone()
    };
    let syntax = if dark {
        SOLARIZED_SYNTAX
    } else {
        NORD_LIGHT_SYNTAX
    };
    theme.style.syntax = serde_json::from_value(syntax_json(syntax)).unwrap();
    theme
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 间距用 4 px 网格 (CLAUDE.md): no half-step spacing utilities (gap_0p5 is 2 px, gap_1p5
    /// 6 px …) in the UI code.
    #[test]
    fn spacing_utilities_stay_on_the_4px_grid() {
        fn visit(dir: &std::path::Path, found: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    visit(&path, found);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (n, line) in text.lines().enumerate() {
                        let off_grid = line.match_indices("p5()").any(|(at, _)| {
                            line[..at].ends_with(|c: char| c.is_ascii_digit())
                                && line[..at]
                                    .trim_end_matches(|c: char| c.is_ascii_digit())
                                    .ends_with('_')
                        });
                        if off_grid {
                            found.push(format!("{}:{}", path.display(), n + 1));
                        }
                    }
                }
            }
        }
        let mut found = Vec::new();
        visit(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut found,
        );
        assert!(found.is_empty(), "off the 4 px grid: {found:?}");
    }

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
            for bg in [p.commit, p.commit_hover] {
                let commit = contrast(p.commit_fg, bg);
                assert!(commit >= 4.5, "{name} commit button: {commit:.2}");
            }
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
    fn agent_colors_are_readable() {
        for (name, p) in [("dark", DARK), ("light", LIGHT)] {
            for (what, color) in [
                ("attention", p.attention),
                ("unread", p.unread),
                ("running", p.running),
                ("claude", p.glyph_claude),
                ("codex", p.glyph_codex),
                ("deepseek", p.glyph_deepseek),
                ("gemini", p.glyph_gemini),
                ("generic", p.glyph_generic),
            ] {
                for (surface, bg) in [("panel", p.panel), ("card", p.card)] {
                    let ratio = contrast(color, bg);
                    // Glyphs and dots are non-text UI: WCAG 1.4.11 asks for 3:1.
                    assert!(ratio >= 3.0, "{name} {what} on {surface}: {ratio:.2}");
                }
            }
            let fg = contrast(p.foreground, p.card);
            let muted = contrast(p.muted, p.card);
            assert!(fg >= 7.0, "{name} foreground on card: {fg:.2}");
            assert!(muted >= 4.5, "{name} muted on card: {muted:.2}");
            let attention = contrast(p.attention, p.panel);
            assert!(attention >= 4.5, "{name} 待批准 text: {attention:.2}");
        }
    }

    #[test]
    fn diff_rows_follow_the_font_and_keep_the_hatch_period() {
        let period = DIFF_HATCH.0 + DIFF_HATCH.1;
        for size in 10..=24 {
            let m = diff_metrics(px(size as f32));
            let row = f32::from(m.row);
            assert_eq!(row % period, 0., "{size}");
            assert!(row >= size as f32 * 1.25, "{size}: {row}");
        }
        assert_eq!(diff_metrics(px(14.)).row, px(20.));
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
