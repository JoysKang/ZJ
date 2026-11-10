//! Best-effort import names of a file: which modules and symbols it says it pulls in. Used to
//! rank cross-file definitions (`symbol_index::rank`). Only lines that start like an import are
//! read, so a mention in code or a comment cannot count, and the answer is a flat list of
//! identifier-shaped names — a false positive only reorders candidates, it never decides a jump
//! by itself. Pure and cheap: no query, no tree.

/// How many leading lines are searched for import statements.
const MAX_LINES: usize = 4000;

/// Words that carry no module or symbol meaning in an import statement.
const STOPWORDS: &[&str] = &[
    "and",
    "as",
    "break",
    "case",
    "class",
    "const",
    "continue",
    "crate",
    "def",
    "default",
    "else",
    "export",
    "extends",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "from",
    "if",
    "implements",
    "import",
    "in",
    "include",
    "is",
    "let",
    "mod",
    "namespace",
    "new",
    "not",
    "null",
    "of",
    "or",
    "package",
    "private",
    "protected",
    "pub",
    "public",
    "require",
    "return",
    "self",
    "source",
    "static",
    "super",
    "switch",
    "template",
    "true",
    "type",
    "typedef",
    "use",
    "using",
    "var",
    "void",
    "while",
];

/// Line prefixes that start an import statement in a navigation language.
fn prefixes(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["use ", "pub use ", "mod ", "pub mod ", "extern crate "],
        "python" => &["import ", "from "],
        "javascript" | "typescript" | "tsx" => &["import ", "export "],
        "go" => &["import ", "\"", "`"],
        "c" | "cpp" => &["#include", "#import"],
        "java" => &["import "],
        "bash" => &["source ", ". "],
        _ => &[],
    }
}

fn is_import(language: &str, line: &str) -> bool {
    if prefixes(language)
        .iter()
        .any(|prefix| line.starts_with(prefix))
    {
        return true;
    }
    // `const x = require('...')`, and the closing line of a multi-line `import {...} from '...'`
    // whose first line already matched; neither can be caught by a prefix alone.
    matches!(language, "javascript" | "typescript" | "tsx")
        && (line.contains("require(") || line.contains("from '") || line.contains("from \""))
}

/// A statement continues while its line ends with an opener or a separator (a trailing comma):
/// `use a::{`, `from x import (`, `import {`. Everything else is the end of the statement.
fn continues(line: &str) -> bool {
    line.ends_with(['(', '{', '[', ',', '\\'])
}

/// Identifier-shaped tokens of a statement, in order, without stopwords.
fn tokens(statement: &str) -> impl Iterator<Item = &str> {
    statement
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|token| {
            !token.is_empty()
                && !token.starts_with(|c: char| c.is_ascii_digit())
                && !STOPWORDS.contains(&token.to_ascii_lowercase().as_str())
        })
}

/// A module a Python import statement names: how many dots lead it (0 is an absolute
/// import, 1 the importing file's own package) and the dotted path after them. An empty
/// path with a level is `from . import x`, which names the package's `__init__`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    pub level: usize,
    pub parts: Vec<String>,
}

/// Python modules `text` imports, in source order. Unlike `names`, this keeps the dotted
/// path, so `from db.models import X` can be resolved to the file it names.
pub fn python_modules(text: &str) -> Vec<Module> {
    let lines: Vec<&str> = text.lines().take(MAX_LINES).collect();
    let mut modules = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        let from = line.strip_prefix("from ");
        let import = line.strip_prefix("import ");
        if from.is_none() && import.is_none() {
            index += 1;
            continue;
        }
        let mut end = index;
        while end + 1 < lines.len() && continues(lines[end].trim_end()) {
            end += 1;
        }
        let statement = lines[index..=end].join(" ");
        if let Some(rest) = statement.trim_start().strip_prefix("from ") {
            if let Some(module) = python_module(rest.split_whitespace().next().unwrap_or("")) {
                modules.push(module);
            }
        } else if let Some(rest) = statement.trim_start().strip_prefix("import ") {
            for item in rest.split(',') {
                if let Some(module) = python_module(item.split_whitespace().next().unwrap_or("")) {
                    modules.push(module);
                }
            }
        }
        index = end + 1;
    }
    modules
}

/// One dotted module name as Python writes it: `..db.models`, `.`, `db.models`.
fn python_module(text: &str) -> Option<Module> {
    let level = text.chars().take_while(|c| *c == '.').count();
    let parts: Vec<String> = text[level..]
        .split('.')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() && level == 0 {
        return None;
    }
    let identifier = |part: &str| {
        part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if !parts.iter().all(|part| identifier(part)) {
        return None;
    }
    Some(Module { level, parts })
}

/// Modules and symbols `text` imports, deduplicated, in source order.
pub fn names(language: &str, text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().take(MAX_LINES).collect();
    let mut names: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if !is_import(language, line) {
            index += 1;
            continue;
        }
        let mut end = index;
        while end + 1 < lines.len() && continues(lines[end].trim_end()) {
            end += 1;
        }
        let statement = lines[index..=end].join(" ");
        for token in tokens(&statement) {
            if !names.iter().any(|name| name == token) {
                names.push(token.to_string());
            }
        }
        index = end + 1;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_of(language: &str, text: &str) -> Vec<String> {
        names(language, text)
    }

    #[test]
    fn reads_imports_per_language() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "rust",
                "use crate::net::client::connect;\nmod util;\n",
                &["net", "client", "connect", "util"],
            ),
            (
                "rust",
                "use std::{\n    collections::HashMap,\n    fmt,\n};\n",
                &["std", "collections", "HashMap", "fmt"],
            ),
            (
                "python",
                "import os\nfrom net.client import connect, Client\n",
                &["os", "net", "client", "connect", "Client"],
            ),
            (
                "javascript",
                "import { connect } from './net/client';\nconst x = require('left-pad');\n",
                &["net", "client", "connect", "left", "pad"],
            ),
            (
                "typescript",
                "import {\n  connect,\n  Client,\n} from '../net/client';\n",
                &["net", "client", "connect", "Client"],
            ),
            (
                "go",
                "import (\n\t\"fmt\"\n\t\"example.com/net/client\"\n)\n",
                &["fmt", "example", "com", "net", "client"],
            ),
            (
                "c",
                "#include \"net/client.h\"\n#include <stdio.h>\n",
                &["net", "client", "h", "stdio"],
            ),
            (
                "java",
                "import com.example.net.Client;\n",
                &["com", "example", "net", "Client"],
            ),
            (
                "bash",
                "source ./scripts/lib.sh\n",
                &["scripts", "lib", "sh"],
            ),
        ];
        for (language, text, expected) in cases {
            let found = names_of(language, text);
            for name in *expected {
                assert!(
                    found.iter().any(|n| n == name),
                    "{language}: {name} in {found:?}"
                );
            }
        }
    }

    #[test]
    fn python_modules_keep_the_dotted_path() {
        let text =
            "import os, a.b as c\nfrom ..db.models import (\n    X,\n)\nfrom . import helper\n";
        assert_eq!(
            python_modules(text),
            [
                Module {
                    level: 0,
                    parts: vec!["os".into()]
                },
                Module {
                    level: 0,
                    parts: vec!["a".into(), "b".into()]
                },
                Module {
                    level: 2,
                    parts: vec!["db".into(), "models".into()]
                },
                Module {
                    level: 1,
                    parts: Vec::new()
                },
            ]
        );
        assert!(python_modules("print('from x import y')\n").is_empty());
    }

    #[test]
    fn code_that_is_not_an_import_is_ignored() {
        // The same words the import test looks for, in positions that are not imports.
        assert!(
            names(
                "rust",
                "let client = connect();\n// use net::client::connect;\n"
            )
            .is_empty()
        );
        assert!(
            names("python", "connect = client.fetch()\nprint('import os')\n").is_empty(),
            "a call and a string are not imports"
        );
        assert!(names("markdown", "import { connect } from './net/client'\n").is_empty());
    }

    #[test]
    fn a_language_without_imports_has_no_names() {
        assert!(names("json", "{\"a\": 1}").is_empty());
        assert!(names("plain", "use crate::a;").is_empty());
    }
}
