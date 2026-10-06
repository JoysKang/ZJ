//! Syntax colors for fenced code in agent replies. Kit's `TextView` renders the Markdown; the
//! colors come from the editor's syntax theme and are worked out off the UI thread
//! (`workbench/agent/highlights.rs`).

use gpui_kit::{HighlightStyle, component::highlighter::HighlightTheme};
use std::ops::Range;

/// Syntax runs over a code block's text.
pub type Runs = Vec<(Range<usize>, HighlightStyle)>;

/// Fence tag → the highlighter's language name (only grammars compiled into Kit).
pub fn fence_language(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().to_ascii_lowercase();
    let ext = match tag.as_str() {
        "" => return None,
        "rust" | "rs" => "rs",
        "python" | "py" => "py",
        "javascript" | "js" | "jsx" => "js",
        "typescript" | "ts" => "ts",
        "tsx" => "tsx",
        "bash" | "sh" | "shell" | "zsh" | "console" => "sh",
        "go" | "golang" => "go",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "hpp" => "cpp",
        "java" => "java",
        "sql" => "sql",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "json" | "jsonc" => "json",
        "html" => "html",
        "css" => "css",
        "markdown" | "md" => "md",
        "diff" | "patch" => "diff",
        _ => return None,
    };
    match crate::languages::for_path(std::path::Path::new(&format!("x.{ext}"))).0 {
        "plain" => None,
        language => Some(language),
    }
}

/// Syntax runs for one fenced block; none for unknown languages and very large blocks.
pub fn highlight(tag: &str, code: &str, theme: &HighlightTheme) -> Runs {
    match fence_language(tag) {
        Some(language) if code.len() <= 256 * 1024 => {
            crate::diff_doc::highlight(language, code, theme)
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_fences_get_syntax_runs() {
        assert_eq!(fence_language("Rust"), Some("rust"));
        assert_eq!(fence_language("py"), Some("python"));
        assert_eq!(fence_language("text"), None);
        let theme = crate::theme::highlight_theme_for_tests(true);
        assert!(!highlight("rust", "fn main() { let x = 1; }", &theme).is_empty());
        assert!(highlight("", "fn main() {}", &theme).is_empty());
    }
}
