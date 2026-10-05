//! Go to definition (⌘-click, F12), go to symbol in file (⌘⇧O), find references (⇧F12) and
//! back / forward (⌃- / ⌃⇧-), without language servers: tree-sitter queries in
//! [`crate::symbols`] plus the lazily built [`SymbolIndex`]. All parsing runs off the UI thread.

use super::quick_open::{Pick, PickIcon, PickItem};
use super::{Pane, Prototype};
use crate::{
    symbol_index::{self, Location, SymbolIndex},
    symbols::{self, Kind, Local},
    theme,
};
use gpui_kit::{
    assets::IconName,
    component::input::{DefinitionProvider, EditorState, Rope, RopeExt, ShowDocumentHandler},
    *,
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_TARGETS: usize = 50;
const MAX_REFERENCES: usize = 500;
const MAX_HISTORY: usize = 50;
const URI_PREFIX: &str = "zj-nav:";

gpui_kit::actions!(
    workspace,
    [
        GoToSymbol,
        GoToLine,
        FindReferences,
        NavigateBack,
        NavigateForward
    ]
);

/// `:12` or `:12:5` (also `:12,5`) in the command center: 1-based line and optional column.
/// `None` for anything else, including 0.
pub(super) fn parse_line_query(query: &str) -> Option<(u32, Option<u32>)> {
    let query = query.trim();
    let (line, column) = match query.split_once([':', ',']) {
        Some((line, column)) => (line.trim(), Some(column.trim())),
        None => (query, None),
    };
    let number = |text: &str| text.parse::<u32>().ok().filter(|n| *n > 0);
    let line = number(line)?;
    match column {
        None => Some((line, None)),
        Some(column) => Some((line, Some(number(column)?))),
    }
}

/// A place to jump to; `column` and `len` are bytes in the line.
#[derive(Clone, Debug)]
pub struct Target {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub len: u32,
    pub name: String,
    pub kind: Kind,
    pub preview: String,
}

/// A cursor position in the back / forward history.
#[derive(Clone, Debug, PartialEq)]
pub struct NavPoint {
    path: PathBuf,
    offset: usize,
}

/// Kind icon and color in pickers, after VS Code's symbol icons.
pub(super) fn kind_icon(kind: Kind, colors: theme::Colors) -> (IconName, Hsla) {
    match kind {
        Kind::Function | Kind::Method => (IconName::SquareFunction, colors.accent),
        Kind::Class => (IconName::Box, colors.modified),
        Kind::Interface => (IconName::Braces, colors.added),
        Kind::Module => (IconName::Package, colors.muted),
        Kind::Type => (IconName::Type, colors.added),
        Kind::Macro => (IconName::Hash, colors.conflict),
        Kind::Constant | Kind::Field | Kind::Variable => (IconName::Variable, colors.untracked),
    }
}

fn line_text(text: &str, line: u32) -> String {
    text.lines()
        .nth(line as usize)
        .unwrap_or_default()
        .trim()
        .chars()
        .take(160)
        .collect()
}

fn target(path: &Path, text: &str, symbol: &symbols::Symbol) -> Target {
    Target {
        path: path.to_path_buf(),
        line: symbol.line,
        column: symbol.column,
        len: symbol.range.len() as u32,
        name: symbol.name.clone(),
        kind: symbol.kind,
        preview: line_text(text, symbol.line),
    }
}

/// Resolves the identifier at `offset` in `path` (whose current text is `text`).
fn resolve(
    path: &Path,
    language: &str,
    text: &str,
    offset: usize,
    index: Option<&SymbolIndex>,
) -> Vec<Target> {
    let Some((name, local)) = symbols::resolve_local(language, text, offset) else {
        return Vec::new();
    };
    let same_file = match local {
        Local::Binding(symbol) => return vec![target(path, text, &symbol)],
        // A unique definition in this file is what the name means here.
        Local::Unknown { definitions } if definitions.len() == 1 => {
            return vec![target(path, text, &definitions[0])];
        }
        Local::Unknown { definitions } => definitions,
        Local::OnDefinition => Vec::new(),
    };
    let here = symbols::word_at(text, offset).unwrap_or(offset..offset);
    let here_line = text[..here.start].matches('\n').count() as u32;
    // The index has this file's saved version; its in-memory definitions replace them.
    let mut others: Vec<Location> = index
        .map(|index| index.definitions(&name))
        .unwrap_or_default()
        .into_iter()
        .filter(|location| location.path != path)
        .collect();
    // A Rust call does not mean a TypeScript function of the same name.
    if others
        .iter()
        .any(|l| symbol_index::language(&l.path) == Some(language))
    {
        others.retain(|l| symbol_index::language(&l.path) == Some(language));
    }
    symbol_index::rank(path, text, &mut others);
    // `db::connect` / `net.connect`: a qualifier naming one candidate's module decides it.
    if let Some(qualifier) = qualifier(text, here.start) {
        let module = |location: &Location| {
            let stem = location.path.file_stem().and_then(|s| s.to_str());
            let dir = location
                .path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str());
            stem == Some(qualifier) || (stem == Some("mod") && dir == Some(qualifier))
        };
        let matching: Vec<&Location> = others.iter().filter(|l| module(l)).collect();
        if matching.len() == 1 && same_file.is_empty() {
            others = vec![matching[0].clone()];
        }
    }
    let mut targets: Vec<Target> = same_file
        .iter()
        .filter(|symbol| symbol.line != here_line)
        .map(|symbol| target(path, text, symbol))
        .collect();
    let mut cache: Option<(PathBuf, String)> = None;
    for location in others.into_iter().take(MAX_TARGETS) {
        if cache.as_ref().is_none_or(|(p, _)| *p != location.path) {
            cache = symbol_index::read(&location.path).map(|t| (location.path.clone(), t));
        }
        let preview = cache
            .as_ref()
            .map(|(_, text)| line_text(text, location.line))
            .unwrap_or_default();
        targets.push(Target {
            path: location.path,
            line: location.line,
            column: location.column,
            len: location.len,
            name: name.clone(),
            kind: location.kind,
            preview,
        });
    }
    targets.truncate(MAX_TARGETS);
    targets
}

/// The identifier right before `::` or `.` in front of `start`, as in `db::connect`.
fn qualifier(text: &str, start: usize) -> Option<&str> {
    let before = &text[..start];
    let before = before
        .strip_suffix("::")
        .or_else(|| before.strip_suffix('.'))?;
    let word = symbols::word_at(before, before.len())?;
    (word.end == before.len()).then(|| &before[word])
}

/// Every occurrence of the name at `offset` known to the tags queries, workspace-wide.
fn references(
    path: &Path,
    language: &str,
    text: &str,
    offset: usize,
    files: Vec<PathBuf>,
) -> Vec<Target> {
    let Some(word) = symbols::word_at(text, offset) else {
        return Vec::new();
    };
    let name = &text[word];
    let mut found = Vec::new();
    let collect = |file: &Path, language: &str, text: &str, found: &mut Vec<Target>| {
        if !text.contains(name) {
            return;
        }
        if let Some(symbols) = symbols::scan(language, text) {
            for symbol in symbols.definitions.iter().chain(&symbols.references) {
                if symbol.name == name && found.len() < MAX_REFERENCES {
                    found.push(target(file, text, symbol));
                }
            }
        }
    };
    collect(path, language, text, &mut found);
    for file in files {
        if file == path || found.len() >= MAX_REFERENCES {
            continue;
        }
        let (Some(language), Some(text)) =
            (symbol_index::language(&file), symbol_index::read(&file))
        else {
            continue;
        };
        collect(&file, language, &text, &mut found);
    }
    found
}

/// Kit's editor asks this for definitions on ⌘-hover, ⌘-click and F12.
struct Definitions {
    view: WeakEntity<Prototype>,
    path: PathBuf,
    language: &'static str,
}

impl DefinitionProvider for Definitions {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::LocationLink>>> {
        let Some(view) = self.view.upgrade() else {
            return Task::ready(Ok(Vec::new()));
        };
        let index = view.update(cx, |this, cx| this.symbol_index(cx));
        let rope = text.clone();
        let source = text.to_string();
        let (path, language) = (self.path.clone(), self.language);
        let job = cx.background_spawn(async move {
            resolve(&path, language, &source, offset, index.as_deref())
        });
        let weak = self.view.clone();
        cx.spawn(async move |cx| {
            let targets = job.await;
            if targets.is_empty() {
                return Ok(Vec::new());
            }
            let generation = weak.update(cx, |this, _| {
                this.nav.generation += 1;
                this.nav.targets = (this.nav.generation, targets);
                this.nav.generation
            })?;
            // Underline exactly the word under the pointer.
            let source = rope.to_string();
            let word = symbols::word_at(&source, offset).unwrap_or(offset..offset);
            let range = lsp_types::Range {
                start: rope.offset_to_position(word.start),
                end: rope.offset_to_position(word.end),
            };
            let uri: lsp_types::Uri = format!("{URI_PREFIX}{generation}").parse()?;
            Ok(vec![lsp_types::LocationLink {
                origin_selection_range: Some(range),
                target_uri: uri,
                target_range: range,
                target_selection_range: range,
            }])
        })
    }
}

/// Adds go to definition to a newly created editor for a supported language.
pub(super) fn attach(state: &mut EditorState, path: &Path, view: WeakEntity<Prototype>) {
    {
        let Some(language) = symbol_index::language(path) else {
            return;
        };
        let handler: ShowDocumentHandler = Rc::new({
            let view = view.clone();
            move |params, window, cx| {
                let Some(generation) = params
                    .uri
                    .as_str()
                    .strip_prefix(URI_PREFIX)
                    .and_then(|g| g.parse::<u64>().ok())
                else {
                    return false;
                };
                // The editor is mid-update here; jump once it is released.
                let view = view.clone();
                window.defer(cx, move |window, cx| {
                    let _ = view.update(cx, |this, cx| this.follow_targets(generation, window, cx));
                });
                true
            }
        });
        let lsp = state.lsp_mut();
        lsp.definition_provider = Some(Rc::new(Definitions {
            view,
            path: path.to_path_buf(),
            language,
        }));
        lsp.show_document = Some(handler);
    }
}

impl Prototype {
    /// The workspace symbol index; requesting it starts the background build once.
    pub(super) fn symbol_index(&mut self, cx: &mut Context<Self>) -> Option<Arc<SymbolIndex>> {
        if self.nav.symbols.is_none() && self.nav.symbols_task.is_none() {
            self.nav.symbols_requested = true;
            self.build_symbol_index(cx);
        }
        self.nav.symbols.clone()
    }

    pub(super) fn build_symbol_index(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.index.as_ref().map(|index| index.paths()) else {
            // build_index calls back here when the file list is ready.
            return;
        };
        self.nav.symbols_cancel.store(true, Ordering::Relaxed);
        self.nav.symbols_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.nav.symbols_cancel.clone();
        let job = cx.background_spawn(async move {
            let index = SymbolIndex::build(paths, &cancel);
            symbol_index::release_free_memory();
            eprintln!(
                "event=symbol_index_built files={} definitions={} bytes={} seconds={:.2} heap_kb={}",
                index.stats.files,
                index.stats.definitions,
                index.stats.bytes,
                index.stats.seconds,
                index.heap_bytes() / 1024
            );
            (index, cancel)
        });
        self.nav.symbols_task = Some(cx.spawn(async move |this, cx| {
            let (index, cancel) = job.await;
            let _ = this.update(cx, |this, _| {
                this.nav.symbols_task = None;
                if !cancel.load(Ordering::Relaxed) {
                    this.nav.symbols = Some(Arc::new(index));
                }
            });
        }));
    }

    /// Re-parses changed files into the symbol index (file watching).
    pub(super) fn update_symbol_index(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(index) = self.nav.symbols.clone() else {
            return;
        };
        let files: Vec<PathBuf> = files
            .into_iter()
            .filter(|path| symbol_index::language(path).is_some())
            .collect();
        if files.is_empty() || self.nav.symbols_task.is_some() {
            return;
        }
        let job = cx.background_spawn(async move { index.with_changes(&files) });
        self.nav.symbols_task = Some(cx.spawn(async move |this, cx| {
            let next = job.await;
            let _ = this.update(cx, |this, _| {
                this.nav.symbols_task = None;
                this.nav.symbols = Some(Arc::new(next));
            });
        }));
    }

    fn follow_targets(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.nav.targets.0 != generation {
            return;
        }
        let targets = self.nav.targets.1.clone();
        match targets.len() {
            0 => {}
            1 => self.jump_to(targets[0].clone(), true, window, cx),
            _ => self.show_locations(targets, "定义", window, cx),
        }
    }

    fn show_locations(
        &mut self,
        targets: Vec<Target>,
        what: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<PickItem> = targets
            .into_iter()
            .map(|target| {
                let file = target
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let relative = self
                    .root
                    .as_ref()
                    .and_then(|root| target.path.strip_prefix(root).ok())
                    .unwrap_or(&target.path)
                    .display()
                    .to_string();
                PickItem {
                    label: if target.preview.is_empty() {
                        target.name.clone()
                    } else {
                        target.preview.clone()
                    },
                    detail: format!("{relative}:{}", target.line + 1),
                    icon: PickIcon::File(crate::file_icons::for_file(&file)),
                    keys: Vec::new(),
                    pick: Pick::Jump(target),
                }
            })
            .collect();
        self.message = String::new();
        let count = items.len();
        let placeholder = format!("{count} 个{what}，输入以筛选");
        self.open_picker(items, placeholder, "没有找到", window, cx);
    }

    pub(super) fn active_document(&self) -> Option<(PathBuf, Entity<EditorState>)> {
        match self.active {
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id)
                .map(|doc| (doc.path.clone(), doc.editor.clone())),
            _ => None,
        }
    }

    fn here(&self, cx: &App) -> Option<NavPoint> {
        let (path, editor) = self.active_document()?;
        Some(NavPoint {
            path,
            offset: editor.read(cx).cursor(),
        })
    }

    pub(super) fn remember(&mut self, cx: &App) {
        if let Some(point) = self.here(cx) {
            if self.nav.back.last() != Some(&point) {
                self.nav.back.push(point);
            }
            if self.nav.back.len() > MAX_HISTORY {
                self.nav.back.remove(0);
            }
            self.nav.forward.clear();
        }
    }

    /// Moves to `target`, opening its file if needed; `record` adds the current spot to the
    /// back history.
    pub(super) fn jump_to(
        &mut self,
        target: Target,
        record: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if record {
            self.remember(cx);
        }
        let place = Placement::Point {
            line: target.line,
            column: target.column,
            len: target.len,
        };
        self.go(target.path, place, window, cx);
    }

    pub(super) fn go(
        &mut self,
        path: PathBuf,
        place: Placement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self.documents.iter().find(|doc| doc.path == path) {
            let (id, editor) = (doc.id, doc.editor.clone());
            let switched = self.markdown_show_source(id, window, cx);
            if self.active != Pane::Document(id) {
                self.select_pane(Pane::Document(id), window, cx);
            } else if switched {
                self.focus_active_editor(window, cx);
            }
            place.apply(&editor, window, cx);
            return;
        }
        self.nav.pending_place = Some((path.clone(), place));
        self.open_file(path, self.root.clone(), window, cx);
    }

    /// Applies a jump that was waiting for its file to open.
    pub(super) fn apply_pending_place(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, place)) = self.nav.pending_place.take() else {
            return;
        };
        if let Some(doc) = self.documents.iter().find(|doc| doc.path == path) {
            let (id, editor) = (doc.id, doc.editor.clone());
            if self.markdown_show_source(id, window, cx) {
                self.focus_active_editor(window, cx);
            }
            place.apply(&editor, window, cx);
        }
    }

    pub(super) fn navigate_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(point) = self.nav.back.pop() else {
            return;
        };
        if let Some(here) = self.here(cx) {
            self.nav.forward.push(here);
        }
        self.go(point.path, Placement::Offset(point.offset), window, cx);
    }

    pub(super) fn navigate_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(point) = self.nav.forward.pop() else {
            return;
        };
        if let Some(here) = self.here(cx) {
            self.nav.back.push(here);
        }
        self.go(point.path, Placement::Offset(point.offset), window, cx);
    }

    /// ⌃G / `:N:C`: puts the cursor at 0-based `line` and `column` (characters) of the
    /// active document, recorded in the back history.
    pub(super) fn go_to_line(
        &mut self,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(doc) = self.documents.iter().find(|doc| doc.id == id) else {
            return;
        };
        let (path, center) = (doc.path.clone(), !doc.soft_wrap);
        self.remember(cx);
        let place = Placement::Line {
            line,
            column,
            center,
        };
        self.go(path, place, window, cx);
    }

    /// ⌘⇧O: the outline of the active file in a filterable list.
    pub(super) fn go_to_symbol(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, editor)) = self.active_document() else {
            return;
        };
        let language = crate::languages::for_path(&path).0;
        let text = editor.read(cx).text().to_string();
        let job = cx.background_spawn(async move {
            let symbols = symbols::outline(language, &text);
            symbols
                .iter()
                .map(|symbol| target(&path, &text, symbol))
                .collect::<Vec<_>>()
        });
        self.nav.task = Some(cx.spawn_in(window, async move |this, cx| {
            let targets = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let items = targets
                    .into_iter()
                    .map(|target| PickItem {
                        label: target.name.clone(),
                        detail: format!("{} · 第 {} 行", target.kind.label(), target.line + 1),
                        icon: PickIcon::Symbol(target.kind),
                        keys: Vec::new(),
                        pick: Pick::Jump(target),
                    })
                    .collect();
                this.open_picker(
                    items,
                    "转到文件中的符号".to_string(),
                    "此文件没有可识别的符号",
                    window,
                    cx,
                );
            });
        }));
    }

    /// ⇧F12: occurrences of the name at the cursor across the workspace.
    pub(super) fn find_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, editor)) = self.active_document() else {
            return;
        };
        let Some(language) = symbol_index::language(&path) else {
            return;
        };
        let state = editor.read(cx);
        let text = state.text().to_string();
        let offset = state.cursor();
        let files = self
            .symbol_index(cx)
            .map(|index| index.files().to_vec())
            .unwrap_or_default();
        self.message = "正在查找引用…".into();
        let job =
            cx.background_spawn(async move { references(&path, language, &text, offset, files) });
        self.nav.task = Some(cx.spawn_in(window, async move |this, cx| {
            let targets = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.message.clear();
                this.show_locations(targets, "引用", window, cx);
            });
        }));
    }
}

/// Where to put the cursor once a file is showing.
#[derive(Clone, Debug)]
pub enum Placement {
    Point {
        line: u32,
        column: u32,
        len: u32,
    },
    Offset(usize),
    /// `column` in characters; `center` scrolls the line to the middle of the editor.
    Line {
        line: u32,
        column: u32,
        center: bool,
    },
}

impl Placement {
    fn apply(&self, editor: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
        editor.update(cx, |state, cx| {
            if let Placement::Line {
                line,
                column,
                center,
            } = *self
            {
                state.set_cursor_position(lsp_types::Position::new(line, column), window, cx);
                // Kit only scrolls the cursor into view at the nearest edge. Buffer lines are
                // display rows only without soft wrap, so wrapped buffers keep Kit's scroll.
                if center
                    && let (Some(height), Some(bounds)) = (state.line_height(), state.text_bounds())
                {
                    let top = height * line as f32 - (bounds.size.height - height) / 2.;
                    let x = state.scroll_offset().x;
                    state.set_scroll_offset(point(x, -top.max(Pixels::ZERO)), cx);
                }
                return;
            }
            let rope = state.text().clone();
            let (start, end) = match *self {
                Placement::Line { .. } => return,
                Placement::Point { line, column, len } => {
                    let start = rope.point_to_offset(gpui_kit::component::input::Point::new(
                        line as usize,
                        column as usize,
                    ));
                    (start, (start + len as usize).min(rope.len()))
                }
                Placement::Offset(offset) => {
                    let offset = offset.min(rope.len());
                    (offset, offset)
                }
            };
            state.set_cursor_position(rope.offset_to_position(start), window, cx);
            if end > start {
                state.set_selected_range(start..end, cx);
            }
        });
    }
}

/// Go to definition, references and back / forward: the symbol index and the navigation history.
pub(super) struct NavState {
    /// Workspace symbols for cross-file go to definition, built on first use.
    pub(super) symbols: Option<Arc<crate::symbol_index::SymbolIndex>>,
    pub(super) symbols_task: Option<Task<()>>,
    pub(super) symbols_cancel: Arc<AtomicBool>,
    pub(super) symbols_requested: bool,
    pub(super) generation: u64,
    pub(super) targets: (u64, Vec<Target>),
    pub(super) back: Vec<NavPoint>,
    pub(super) forward: Vec<NavPoint>,
    pub(super) task: Option<Task<()>>,
    pub(super) pending_place: Option<(PathBuf, Placement)>,
}

#[cfg(test)]
mod tests {
    use super::{AtomicBool, Path, SymbolIndex, parse_line_query, references, resolve};
    use std::fs;

    #[test]
    fn line_queries_parse_line_and_column() {
        assert_eq!(parse_line_query("3"), Some((3, None)));
        assert_eq!(parse_line_query(" 12 "), Some((12, None)));
        assert_eq!(parse_line_query("12:5"), Some((12, Some(5))));
        assert_eq!(parse_line_query("12,5"), Some((12, Some(5))));
        assert_eq!(parse_line_query("12 : 5"), Some((12, Some(5))));
        for invalid in ["", "0", "-1", "a", "3:", "3:0", "3:x", ":3", "3:4:5", "1.5"] {
            assert_eq!(parse_line_query(invalid), None, "{invalid:?}");
        }
    }

    /// Each language: the reference in the sample resolves to its definition line.
    #[test]
    fn resolve_targets_for_every_language() {
        for (language, source, line) in crate::symbols::tests::CASES {
            let (text, offset) = crate::symbols::tests::split(source);
            let targets = resolve(Path::new("/tmp/sample"), language, &text, offset, None);
            assert_eq!(
                targets.first().map(|t| t.line),
                Some(*line),
                "{language}: {targets:?}"
            );
        }
    }

    #[test]
    fn qualifiers_pick_the_named_module() {
        let root = std::env::temp_dir().join(format!("zj-nav-q-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        for module in ["db", "net"] {
            fs::write(
                root.join(format!("src/{module}.rs")),
                "pub fn connect() {}\n",
            )
            .unwrap();
        }
        let main = root.join("src/main.rs");
        let source = "mod db;\nmod net;\nfn main() { db::connect(); net::connect(); }\n";
        let index = SymbolIndex::build(
            vec![root.join("src/db.rs"), root.join("src/net.rs")],
            &AtomicBool::new(false),
        );
        let offset = source.rfind("connect").unwrap();
        let targets = resolve(&main, "rust", source, offset, Some(&index));
        assert_eq!(targets.len(), 1);
        assert!(targets[0].path.ends_with("net.rs"));
        // Unqualified: both, for the picker.
        let source = "use db::*;\nuse net::*;\nfn main() { connect(); }\n";
        let offset = source.rfind("connect").unwrap();
        assert_eq!(
            resolve(&main, "rust", source, offset, Some(&index)).len(),
            2
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cross_file_definition_and_references() {
        let root = std::env::temp_dir().join(format!("zj-nav-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("lib")).unwrap();
        let util = root.join("lib/util.py");
        fs::write(&util, "def slugify(text):\n    return text.lower()\n").unwrap();
        let main = root.join("main.py");
        let source = "from lib.util import slugify\n\nprint(slugify('A'))\n";
        fs::write(&main, source).unwrap();
        let index = SymbolIndex::build(vec![util.clone(), main.clone()], &AtomicBool::new(false));
        let offset = source.rfind("slugify").unwrap() + 2;
        let targets = resolve(&main, "python", source, offset, Some(&index));
        assert_eq!(targets.len(), 1);
        assert_eq!(
            (targets[0].path.clone(), targets[0].line),
            (util.clone(), 0)
        );
        assert_eq!(targets[0].preview, "def slugify(text):");
        let found = references(&main, "python", source, offset, index.files().to_vec());
        let places: Vec<_> = found.iter().map(|t| (t.path == util, t.line)).collect();
        assert!(
            places.contains(&(false, 2)) && places.contains(&(true, 0)),
            "{places:?}"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
