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

/// Rust paths `use` and `extern crate` name, in source order, split on `::`. Leading
/// qualifiers (`crate`, `self`, `super`, `std`, …) stay in the path; a braced group keeps
/// only the path before `{`, because its members are items of that module.
pub fn rust_modules(text: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = text.lines().take(MAX_LINES).collect();
    let mut modules: Vec<Vec<String>> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        let is_use = line.starts_with("use ") || line.starts_with("pub use ");
        let is_crate = line.starts_with("extern crate ");
        if !is_use && !is_crate {
            index += 1;
            continue;
        }
        let mut end = index;
        while end + 1 < lines.len() && continues(lines[end].trim_end()) {
            end += 1;
        }
        let statement = lines[index..=end].join(" ");
        let rest = statement
            .trim_start()
            .strip_prefix("pub use ")
            .or_else(|| statement.trim_start().strip_prefix("use "))
            .or_else(|| statement.trim_start().strip_prefix("extern crate "));
        if let Some(rest) = rest {
            let rest = rest.split([';', '{']).next().unwrap_or_default();
            let rest = rest.split(" as ").next().unwrap_or(rest);
            let path: Vec<String> = rest
                .split("::")
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
            if !path.is_empty() && !modules.contains(&path) {
                modules.push(path);
            }
        }
        index = end + 1;
    }
    modules
}

/// Go import paths, in source order, from a single `import "x"` or an `import ( … )` block.
pub fn go_modules(text: &str) -> Vec<String> {
    let mut modules: Vec<String> = Vec::new();
    let mut in_block = false;
    for line in text.lines().take(MAX_LINES) {
        let line = line.trim();
        if in_block {
            if line.starts_with(')') {
                in_block = false;
            } else if let Some(path) = quoted_value(line)
                && !modules.iter().any(|known| known == path)
            {
                modules.push(path.to_string());
            }
            continue;
        }
        let Some(rest) = line.strip_prefix("import ") else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with('(') {
            in_block = true;
        } else if let Some(path) = quoted_value(rest)
            && !modules.iter().any(|known| known == path)
        {
            modules.push(path.to_string());
        }
    }
    modules
}

/// Ruby `require` paths, in source order. `require_relative` and `require` given a relative
/// path stay inside the workspace, so only the installed-library spellings are listed.
pub fn ruby_requires(text: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in text.lines().take(MAX_LINES) {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("require") else {
            continue;
        };
        // `require_dependency` and similar helpers are not Ruby's own require.
        if !rest.starts_with([' ', '(', '\t']) {
            continue;
        }
        let Some(start) = rest.find(['\'', '"']) else {
            continue;
        };
        let quote = rest[start..].chars().next().unwrap_or('\'');
        let quoted = &rest[start + quote.len_utf8()..];
        let Some(end) = quoted.find(quote) else {
            continue;
        };
        let path = &quoted[..end];
        if !path.is_empty()
            && !path.starts_with(['.', '/'])
            // `require "#{name}/x"` names nothing fixed.
            && !path.contains("#{")
            && !paths.iter().any(|known| known == path)
        {
            paths.push(path.to_string());
        }
    }
    paths
}

/// The first quoted string in `text`, with `"` or a backtick.
fn quoted_value(text: &str) -> Option<&str> {
    let start = text.find(['"', '`'])?;
    let quote = text[start..].chars().next()?;
    let rest = &text[start + quote.len_utf8()..];
    Some(&rest[..rest.find(quote)?])
}

/// Module specifiers JavaScript / TypeScript imports, in source order: the quoted path after
/// `from`, inside `require(`, or of a side-effect `import 'x'`.
pub fn javascript_specifiers(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().take(MAX_LINES).collect();
    let mut specifiers: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if !is_import("javascript", line) {
            index += 1;
            continue;
        }
        let mut end = index;
        while end + 1 < lines.len() && continues(lines[end].trim_end()) {
            end += 1;
        }
        let statement = lines[index..=end].join(" ");
        let specifier = quoted_after(&statement, "from ")
            .or_else(|| quoted_after(&statement, "require("))
            .or_else(|| {
                let rest = statement.trim_start().strip_prefix("import ")?.trim_start();
                let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
                let rest = &rest[quote.len_utf8()..];
                Some(&rest[..rest.find(quote)?])
            });
        if let Some(specifier) = specifier
            && !specifier.is_empty()
            && !specifiers.iter().any(|known| known == specifier)
        {
            specifiers.push(specifier.to_string());
        }
        index = end + 1;
    }
    specifiers
}

/// The quoted string following `marker`.
fn quoted_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = text.get(text.find(marker)? + marker.len()..)?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let rest = &rest[quote.len_utf8()..];
    Some(&rest[..rest.find(quote)?])
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
    fn rust_and_go_keep_their_module_paths() {
        let rust = "use serde::{Deserialize, Serialize};\npub use tokio::runtime::Runtime as Rt;\nuse std::collections::HashMap;\nextern crate libc;\n";
        assert_eq!(
            rust_modules(rust),
            [
                vec!["serde"],
                vec!["tokio", "runtime", "Runtime"],
                vec!["std", "collections", "HashMap"],
                vec!["libc"],
            ]
        );
        let go = "import \"fmt\"\n\nimport (\n\t\"example.com/a/b\"\n\talias \"example.com/c\"\n\t_ \"embed\"\n)\n";
        assert_eq!(
            go_modules(go),
            ["fmt", "example.com/a/b", "example.com/c", "embed"]
        );
        assert!(rust_modules("fn main() { let x = 1; }").is_empty());
        assert!(go_modules("var x = \"example.com/a\"\n").is_empty());
    }

    #[test]
    fn javascript_specifiers_keep_the_quoted_path() {
        let text = "import React from 'react';\nimport {\n  a,\n} from \"../lib/util\";\nconst x = require('left-pad');\nimport './side-effect.css';\n";
        assert_eq!(
            javascript_specifiers(text),
            ["react", "../lib/util", "left-pad", "./side-effect.css"]
        );
        assert!(javascript_specifiers("const a = 'from x';\n").is_empty());
    }

    #[test]
    fn ruby_requires_keep_the_load_path() {
        let text = "require 'json'\nrequire \"active_support/core_ext\"\nrequire_relative 'helper'\nrequire './local'\nrequire_dependency 'app/models/user'\nname = \"require 'nope'\"\n";
        assert_eq!(ruby_requires(text), ["json", "active_support/core_ext"]);
        assert!(ruby_requires("puts 'require foo'\n").is_empty());
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
