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
