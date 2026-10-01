//! Code navigation without language servers: tree-sitter tags queries (definitions and
//! references, as GitHub's search-based navigation uses) and small locals queries for
//! scope-aware jumps inside one file. Everything here is pure and runs off the UI thread.

use gpui_kit::component::highlighter::LanguageRegistry;
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Mutex, OnceLock},
};
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};

/// Files above this are not parsed for navigation.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Function,
    Method,
    Class,
    Interface,
    Module,
    Type,
    Constant,
    Field,
    Macro,
    Variable,
}

impl Kind {
    fn from_capture(name: &str) -> Option<Kind> {
        let kind = name.strip_prefix("definition.")?;
        Some(match kind {
            "function" => Kind::Function,
            "method" => Kind::Method,
            "class" => Kind::Class,
            "interface" => Kind::Interface,
            "module" => Kind::Module,
            "type" => Kind::Type,
            "constant" => Kind::Constant,
            "field" => Kind::Field,
            "macro" => Kind::Macro,
            _ => Kind::Variable,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Function => "函数",
            Kind::Method => "方法",
            Kind::Class => "类型",
            Kind::Interface => "接口",
            Kind::Module => "模块",
            Kind::Type => "类型别名",
            Kind::Constant => "常量",
            Kind::Field => "字段",
            Kind::Macro => "宏",
            Kind::Variable => "变量",
        }
    }
}

/// A definition or reference: the name's byte range plus its 0-based line and byte column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: Kind,
    pub range: Range<usize>,
    pub line: u32,
    pub column: u32,
}

/// Compiled lazily and separately: the workspace index needs only `tags`, and a language's
/// `locals` is compiled the first time a file of it is navigated in.
struct Queries {
    name: String,
    language: Language,
    tag_sources: Vec<&'static str>,
    local_sources: Vec<&'static str>,
    tags: OnceLock<Option<Query>>,
    locals: OnceLock<Option<Query>>,
}

impl Queries {
    fn compile(&self, parts: &[&str]) -> Option<Query> {
        match Query::new(&self.language, &parts.concat()) {
            Ok(query) => Some(query),
            Err(error) => {
                eprintln!(
                    "event=nav_query_invalid language={} error={error}",
                    self.name
                );
                None
            }
        }
    }

    fn tags(&self) -> Option<&Query> {
        self.tags
            .get_or_init(|| self.compile(&self.tag_sources))
            .as_ref()
    }

    fn locals(&self) -> Option<&Query> {
        self.locals
            .get_or_init(|| self.compile(&self.local_sources))
            .as_ref()
    }
}

macro_rules! query {
    ($name:literal) => {
        include_str!(concat!("../queries/", $name))
    };
}

/// Tags and locals sources per highlighter language (TS/TSX extend the JavaScript queries).
fn sources(language: &str) -> Option<(Vec<&'static str>, Vec<&'static str>)> {
    Some(match language {
        "rust" => (
            vec![query!("rust-tags.scm"), query!("rust-extra-tags.scm")],
            vec![query!("rust-locals.scm")],
        ),
        "python" => (
            vec![query!("python-tags.scm"), query!("python-extra-tags.scm")],
            vec![query!("python-locals.scm")],
        ),
        "go" => (
            vec![query!("go-tags.scm"), query!("go-extra-tags.scm")],
            vec![query!("go-locals.scm")],
        ),
        "c" => (
            vec![query!("c-tags.scm"), query!("c-extra-tags.scm")],
            vec![query!("c-locals.scm")],
        ),
        "cpp" => (
            vec![query!("cpp-tags.scm"), query!("cpp-extra-tags.scm")],
            vec![query!("cpp-locals.scm")],
        ),
        "java" => (
            vec![query!("java-tags.scm"), query!("java-extra-tags.scm")],
            vec![query!("java-locals.scm")],
        ),
        "javascript" => (
            vec![query!("javascript-tags.scm")],
            vec![query!("javascript-locals.scm")],
        ),
        "typescript" | "tsx" => (
            vec![
                query!("javascript-tags.scm"),
                query!("typescript-tags.scm"),
                query!("typescript-extra-tags.scm"),
            ],
            vec![
                query!("javascript-locals.scm"),
                query!("typescript-locals.scm"),
            ],
        ),
        "bash" => (
            vec![query!("bash-tags.scm")],
            vec![query!("bash-locals.scm")],
        ),
        _ => return None,
    })
}

/// Every highlighter language that has navigation queries.
#[cfg(test)]
pub const LANGUAGES: &[&str] = &[
    "rust",
    "python",
    "go",
    "c",
    "cpp",
    "java",
    "javascript",
    "typescript",
    "tsx",
    "bash",
];

fn queries(language: &str) -> Option<&'static Queries> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<&'static Queries>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(entry) = cache.lock().unwrap().get(language) {
        return *entry;
    }
    let built = (|| {
        let (tags, locals) = sources(language)?;
        let grammar = LanguageRegistry::singleton().language(language)?.language?;
        // Leaked once per language for the process lifetime: compiled queries are immutable.
        Some(&*Box::leak(Box::new(Queries {
            name: language.to_string(),
            language: grammar,
            tag_sources: tags,
            local_sources: locals,
            tags: OnceLock::new(),
            locals: OnceLock::new(),
        })))
    })();
    cache.lock().unwrap().insert(language.to_string(), built);
    built
}

pub fn supported(language: &str) -> bool {
    sources(language).is_some()
}

fn parse(queries: &Queries, text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&queries.language).ok()?;
    parser.parse(text, None)
}

fn symbol(text: &str, node: Node, kind: Kind) -> Symbol {
    let range = node.byte_range();
    let point = node.start_position();
    Symbol {
        name: text[range.clone()].to_string(),
        kind,
        range,
        line: point.row as u32,
        column: point.column as u32,
    }
}

/// Definitions and references of one file, from its tags query.
#[derive(Default, Debug)]
pub struct FileSymbols {
    pub definitions: Vec<Symbol>,
    pub references: Vec<Symbol>,
}

fn tags_in(queries: &Queries, tree: &Tree, text: &str) -> FileSymbols {
    let mut result = FileSymbols::default();
    let Some(query) = queries.tags() else {
        return result;
    };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(256);
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(found) = matches.next() {
        let mut name_node = None;
        let mut kind = None;
        let mut reference = false;
        for capture in found.captures {
            let capture_name = names[capture.index as usize];
            if capture_name == "name" {
                name_node = Some(capture.node);
            } else if let Some(k) = Kind::from_capture(capture_name) {
                kind = Some(k);
            } else if capture_name.starts_with("reference.") {
                reference = true;
            }
        }
        let Some(node) = name_node else { continue };
        if node.byte_range().len() > 256 || node.byte_range().is_empty() {
            continue;
        }
        match (kind, reference) {
            (Some(kind), _) => result.definitions.push(symbol(text, node, kind)),
            (None, true) => result.references.push(symbol(text, node, Kind::Variable)),
            _ => {}
        }
    }
    // A method also matches the plain function pattern; keep the more specific kind.
    result
        .definitions
        .sort_by_key(|s| (s.range.start, s.kind == Kind::Function));
    result.definitions.dedup_by(|a, b| a.range == b.range);
    result.references.sort_by_key(|s| s.range.start);
    result.references.dedup_by(|a, b| a.range == b.range);
    result
}

/// Definitions and references of a whole file (the cross-file index and ⇧F12).
pub fn scan(language: &str, text: &str) -> Option<FileSymbols> {
    if text.len() > MAX_FILE_BYTES {
        return None;
    }
    let queries = queries(language)?;
    let tree = parse(queries, text)?;
    Some(tags_in(queries, &tree, text))
}

/// The document outline for ⌘⇧O, in source order.
pub fn outline(language: &str, text: &str) -> Vec<Symbol> {
    scan(language, text)
        .map(|symbols| symbols.definitions)
        .unwrap_or_default()
}

/// The identifier around `offset` (letters, digits, `_` and `$`).
pub fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    let word = &text[start..end];
    (start < end && !word.starts_with(|c: char| c.is_ascii_digit())).then_some(start..end)
}

/// What a go-to-definition request resolves to inside the file itself.
#[derive(Debug, PartialEq, Eq)]
pub enum Local {
    /// A local binding (parameter, `let`, assignment) visible at the offset.
    Binding(Symbol),
    /// The cursor is on a definition; look for other definitions of the name elsewhere.
    OnDefinition,
    /// Not a local; same-file tag definitions, then the workspace index.
    Unknown { definitions: Vec<Symbol> },
}

/// Scope-aware resolution of the identifier at `offset` within one file.
pub fn resolve_local(language: &str, text: &str, offset: usize) -> Option<(String, Local)> {
    let word = word_at(text, offset)?;
    let name = text[word.clone()].to_string();
    let queries = queries(language)?;
    let tree = parse(queries, text)?;
    let tags = tags_in(queries, &tree, text);
    if tags.definitions.iter().any(|d| d.range == word) {
        return Some((name, Local::OnDefinition));
    }
    if let Some(binding) = local_binding(queries, &tree, text, &name, word.clone()) {
        if binding.range == word {
            return Some((name, Local::OnDefinition));
        }
        return Some((name, Local::Binding(binding)));
    }
    let definitions = tags
        .definitions
        .into_iter()
        .filter(|d| d.name == name)
        .collect();
    Some((name, Local::Unknown { definitions }))
}

fn local_binding(
    queries: &Queries,
    tree: &Tree,
    text: &str,
    name: &str,
    at: Range<usize>,
) -> Option<Symbol> {
    let query = queries.locals()?;
    let names = query.capture_names();
    let mut scopes: Vec<Range<usize>> = std::iter::once(0..text.len()).collect();
    let mut definitions: Vec<Node> = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_match_limit(256);
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(found) = matches.next() {
        for capture in found.captures {
            match names[capture.index as usize] {
                "local.scope" => scopes.push(capture.node.byte_range()),
                "local.definition" => {
                    let node = capture.node;
                    if &text[node.byte_range()] == name {
                        definitions.push(node);
                    }
                }
                _ => {}
            }
        }
    }
    let innermost = |position: usize| {
        scopes
            .iter()
            .filter(|scope| scope.start <= position && position < scope.end.max(scope.start + 1))
            .min_by_key(|scope| scope.len())
            .cloned()
            .unwrap_or(0..text.len())
    };
    // A definition is visible where its scope contains the reference. The innermost such
    // scope wins; within it the nearest definition before the reference (else the first after,
    // for hoisted functions).
    let mut best: Option<(usize, bool, usize, Node)> = None;
    for node in definitions {
        let scope = innermost(node.start_byte());
        // A binding's scope is the scope that encloses it, not the binding node itself.
        let scope = if scope == node.byte_range() {
            innermost(node.start_byte().saturating_sub(1))
        } else {
            scope
        };
        if !(scope.start <= at.start && at.start <= scope.end) {
            continue;
        }
        let before = node.start_byte() <= at.start;
        let distance = at.start.abs_diff(node.start_byte());
        let key = (scope.len(), !before, distance);
        if best
            .as_ref()
            .is_none_or(|(len, after, dist, _)| key < (*len, *after, *dist))
        {
            best = Some((key.0, key.1, key.2, node));
        }
    }
    best.map(|(.., node)| symbol(text, node, Kind::Variable))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// (language, source, cursor marker `‸` before the reference, expected definition line).
    pub const CASES: &[(&str, &str, u32)] = &[
        (
            "rust",
            "struct Point { x: i32 }\nfn make(n: i32) -> Point {\n    let total = n + 1;\n    Point { x: ‸total }\n}\n",
            2,
        ),
        (
            "rust",
            "fn helper() -> u32 { 1 }\nfn main() {\n    let v = ‸helper();\n}\n",
            0,
        ),
        (
            "python",
            "import os\n\ndef area(width, height):\n    scale = 2\n    return width * ‸scale\n",
            3,
        ),
        (
            "python",
            "class Report:\n    pass\n\ndef build():\n    return ‸Report()\n",
            0,
        ),
        (
            "javascript",
            "function sum(a, b) { return a + b; }\nconst total = ‸sum(1, 2);\n",
            0,
        ),
        (
            "typescript",
            "interface User { id: number }\nfunction load(user: User): number {\n  const id = user.id;\n  return ‸id;\n}\n",
            2,
        ),
        (
            "tsx",
            "function Title(props: { text: string }) { return <h1>{props.text}</h1>; }\nexport const App = () => <‸Title text=\"a\" />;\n",
            0,
        ),
        (
            "go",
            "package main\n\nfunc add(a int, b int) int {\n\tsum := a + b\n\treturn ‸sum\n}\n",
            3,
        ),
        (
            "c",
            "static int twice(int n) { return n * 2; }\nint main(void) {\n  int value = 3;\n  return ‸twice(value);\n}\n",
            0,
        ),
        (
            "cpp",
            "namespace demo {\nclass Box { public: int size() const { return 1; } };\n}\nint main() {\n  int count = 2;\n  return ‸count;\n}\n",
            4,
        ),
        (
            "java",
            "public class App {\n  static int twice(int n) { return n * 2; }\n  public static void main(String[] args) {\n    int value = ‸twice(3);\n  }\n}\n",
            1,
        ),
        (
            "bash",
            "#!/bin/bash\ngreet() { echo hi; }\nname=world\n‸greet \"$name\"\n",
            1,
        ),
    ];

    pub fn split(source: &str) -> (String, usize) {
        let offset = source.find('‸').unwrap();
        (source.replace('‸', ""), offset)
    }

    #[test]
    fn every_language_resolves_a_definition_in_file() {
        for (language, source, line) in CASES {
            let (text, offset) = split(source);
            let (name, local) = resolve_local(language, &text, offset)
                .unwrap_or_else(|| panic!("{language}: nothing at the cursor"));
            let found = match local {
                Local::Binding(symbol) => symbol.line,
                Local::Unknown { definitions } => {
                    definitions
                        .first()
                        .unwrap_or_else(|| panic!("{language}: no definition of {name}"))
                        .line
                }
                Local::OnDefinition => panic!("{language}: cursor is not on a definition"),
            };
            assert_eq!(found, *line, "{language}: {name}");
        }
    }

    #[test]
    fn queries_compile_and_outline_lists_definitions() {
        for language in LANGUAGES {
            let queries = queries(language).unwrap_or_else(|| panic!("{language}: no grammar"));
            assert!(queries.tags().is_some(), "{language}: tags query failed");
            assert!(
                queries.locals().is_some(),
                "{language}: locals query failed"
            );
        }
        let outline = outline(
            "rust",
            "mod net {}\nconst MAX: u32 = 1;\nstruct A { field: u8 }\ntrait T {}\nimpl A { fn new() {} }\nfn main() {}\n",
        );
        let names: Vec<_> = outline.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert_eq!(
            names,
            [
                ("net", Kind::Module),
                ("MAX", Kind::Constant),
                ("A", Kind::Class),
                ("field", Kind::Field),
                ("T", Kind::Interface),
                ("new", Kind::Method),
                ("main", Kind::Function),
            ]
        );
        let (_, local) = resolve_local("rust", "fn main() {}\n", 3).unwrap();
        assert_eq!(local, Local::OnDefinition);
    }

    #[test]
    fn words_stop_at_punctuation_and_skip_numbers() {
        let text = "self.items.append(42)";
        assert_eq!(word_at(text, 6), Some(5..10));
        assert_eq!(word_at(text, 10), Some(5..10));
        assert_eq!(word_at(text, 19), None);
    }
}
