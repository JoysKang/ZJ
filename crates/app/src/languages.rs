//! File → highlighter language. Only grammars compiled into GPUI Kit (see the workspace
//! `gpui-kit` features) are returned; everything else is plain text with a display name.

use crate::editing::Comment;
use std::path::Path;

/// (Kit highlighter language, status bar display name).
pub fn for_path(path: &Path) -> (&'static str, &'static str) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match name.as_str() {
        "cargo.lock" | "pipfile" | "poetry.lock" | "uv.lock" => return ("toml", "TOML"),
        ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" | ".profile" | "pkgbuild" => {
            return ("bash", "Shell Script");
        }
        ".clang-format" | ".prettierrc.yaml" => return ("yaml", "YAML"),
        "tsconfig.json" | "jsconfig.json" | ".eslintrc" | ".babelrc" => {
            return ("json", "JSON with Comments");
        }
        _ => {}
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext)
        .unwrap_or_default();
    match extension {
        "rs" => ("rust", "Rust"),
        "md" | "markdown" | "mdx" => ("markdown", "Markdown"),
        "diff" | "patch" => (crate::diff_syntax::LANGUAGE, "Diff"),
        "toml" => ("toml", "TOML"),
        "json" | "json5" => ("json", "JSON"),
        "jsonc" | "code-workspace" => ("json", "JSON with Comments"),
        "js" | "mjs" | "cjs" => ("javascript", "JavaScript"),
        "jsx" => ("javascript", "JavaScript JSX"),
        "ts" | "mts" | "cts" => ("typescript", "TypeScript"),
        "tsx" => ("tsx", "TypeScript JSX"),
        "py" | "pyi" | "pyw" => ("python", "Python"),
        "sh" | "bash" | "zsh" | "ksh" | "command" => ("bash", "Shell Script"),
        "html" | "htm" | "xhtml" => ("html", "HTML"),
        "css" => ("css", "CSS"),
        "yaml" | "yml" => ("yaml", "YAML"),
        "go" => ("go", "Go"),
        "c" | "h" => ("c", "C"),
        "cpp" | "cc" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "ipp" | "inl" => ("cpp", "C++"),
        "java" => ("java", "Java"),
        "sql" => ("sql", "SQL"),
        _ => ("plain", "纯文本"),
    }
}

/// What ⌘/ inserts for the file (VS Code's language configurations); `None` for plain text
/// and diffs.
pub fn comment_for(path: &Path) -> Option<Comment> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(name.as_str(), "makefile" | "gnumakefile") || name.ends_with(".mk") {
        return Some(Comment::Line("#"));
    }
    Some(match for_path(path).0 {
        "rust" | "javascript" | "typescript" | "tsx" | "go" | "c" | "cpp" | "java" | "json" => {
            Comment::Line("//")
        }
        "python" | "bash" | "yaml" | "toml" => Comment::Line("#"),
        "sql" => Comment::Line("--"),
        "css" => Comment::Block("/*", "*/"),
        "html" | "markdown" => Comment::Block("<!--", "-->"),
        _ => return None,
    })
}

/// Every grammar this build relies on, with a small sample, for tests and smoke checks.
#[cfg(test)]
pub const SAMPLES: &[(&str, &str)] = &[
    (
        "rust",
        "/// doc\nfn main() { let x: u32 = 1; println!(\"{x}\"); }\n",
    ),
    (
        "python",
        "import os\n\nclass A:\n    def f(self, n: int = 1):\n        return \"x\" if n else None\n",
    ),
    (
        "javascript",
        "export function f(a = 1) { return `v${a}`; }\nconst x = { k: true };\n",
    ),
    (
        "typescript",
        "interface U { id: number }\nexport const f = (u: U): string => `${u.id}`;\n",
    ),
    (
        "tsx",
        "export function App(): JSX.Element { return <div className=\"a\">{1}</div>; }\n",
    ),
    ("json", "{ \"name\": \"demo\", \"n\": 1, \"ok\": true }\n"),
    ("toml", "[package]\nname = \"demo\"\nversion = 1\n"),
    ("yaml", "name: ci\njobs:\n  build:\n    steps: [1, true]\n"),
    (
        "go",
        "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(1) }\n",
    ),
    (
        "bash",
        "#!/bin/bash\nfor f in \"$@\"; do echo \"$f\"; done\n",
    ),
    (
        "html",
        "<!doctype html>\n<html><body class=\"a\"><h1>Hi</h1></body></html>\n",
    ),
    ("css", ".app { color: #333; margin: 0 auto; }\n"),
    (
        "c",
        "#include <stdio.h>\nstatic int add(int a, int b) { return a + b; }\nint main(void) { printf(\"%d\", add(1, 2)); }\n",
    ),
    (
        "cpp",
        "#include <vector>\nnamespace demo { class Box { public: int size() const { return 1; } }; }\nint main() { std::vector<int> v{1}; return v.size(); }\n",
    ),
    (
        "java",
        "package demo;\npublic class App { public static void main(String[] args) { int n = 1; System.out.println(\"hi\" + n); } }\n",
    ),
    (
        "sql",
        "SELECT id, name FROM users WHERE id = 1 AND name LIKE 'a%';\n",
    ),
    (
        "markdown",
        "# Title\n\n- item\n\n```rust\nfn main() {}\n```\n",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::highlighter::SyntaxHighlighter;

    #[test]
    fn maps_extensions_and_names() {
        let lang = |p: &str| for_path(Path::new(p)).0;
        assert_eq!(lang("a/main.rs"), "rust");
        assert_eq!(lang("tool.PY"), "python");
        assert_eq!(lang("stubs.pyi"), "python");
        assert_eq!(lang("x.mjs"), "javascript");
        assert_eq!(lang("x.cjs"), "javascript");
        assert_eq!(lang("x.jsx"), "javascript");
        assert_eq!(lang("x.ts"), "typescript");
        assert_eq!(lang("x.tsx"), "tsx");
        assert_eq!(lang("ci.yml"), "yaml");
        assert_eq!(lang("run.zsh"), "bash");
        assert_eq!(lang("Cargo.lock"), "toml");
        assert_eq!(lang("tsconfig.json"), "json");
        assert_eq!(lang("main.go"), "go");
        assert_eq!(lang("x.h"), "c");
        assert_eq!(lang("x.hpp"), "cpp");
        assert_eq!(lang("x.cc"), "cpp");
        assert_eq!(lang("App.java"), "java");
        assert_eq!(lang("q.sql"), "sql");
        assert_eq!(lang("README"), "plain");
    }

    #[test]
    fn comment_tokens() {
        let comment = |p: &str| comment_for(Path::new(p));
        assert_eq!(comment("a.rs"), Some(Comment::Line("//")));
        assert_eq!(comment("a.tsx"), Some(Comment::Line("//")));
        assert_eq!(comment("a.py"), Some(Comment::Line("#")));
        assert_eq!(comment("Makefile"), Some(Comment::Line("#")));
        assert_eq!(comment("Cargo.lock"), Some(Comment::Line("#")));
        assert_eq!(comment("q.sql"), Some(Comment::Line("--")));
        assert_eq!(comment("a.css"), Some(Comment::Block("/*", "*/")));
        assert_eq!(comment("README.md"), Some(Comment::Block("<!--", "-->")));
        assert_eq!(comment("notes.txt"), None);
        assert_eq!(comment("x.diff"), None);
    }

    /// Each language has a registered grammar, parses its sample, and yields highlight
    /// captures that the app's own syntax themes (dark and light) give a non-default color.
    #[test]
    fn every_grammar_highlights_with_app_themes() {
        for dark in [true, false] {
            let theme = crate::theme::highlight_theme_for_tests(dark);
            for (language, sample) in SAMPLES {
                let mut highlighter = SyntaxHighlighter::new(language);
                assert_eq!(
                    highlighter.language().as_ref(),
                    *language,
                    "{language}: grammar not registered"
                );
                highlighter.update(None, &gpui_kit::component::Rope::from(*sample), None);
                let plain: gpui_kit::Hsla = gpui_kit::rgb(if dark {
                    crate::theme::DARK.code
                } else {
                    crate::theme::LIGHT.code
                })
                .into();
                let colored = highlighter
                    .styles(&(0..sample.len()), &theme)
                    .into_iter()
                    .filter(|(_, style)| style.color.is_some_and(|color| color != plain))
                    .count();
                assert!(
                    colored >= 3,
                    "{language} (dark={dark}): only {colored} colored spans"
                );
            }
        }
    }
}
