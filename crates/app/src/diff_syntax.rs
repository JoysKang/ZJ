//! Diff highlighting with add/remove colors.
//!
//! tree-sitter-diff's bundled query tags additions as `@string` and deletions as `@keyword`,
//! which themes color arbitrarily (Solarized makes deletions green). This registers the same
//! grammar under our own name with captures reserved for diffs: `predictive` and `hint` are
//! Kit's inlay/ghost-text styles, which no grammar and no feature here uses otherwise.

use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};

pub const LANGUAGE: &str = "zj-diff";

const HIGHLIGHTS: &str = r#"
(addition) @predictive
(deletion) @hint
[(old_file) (new_file)] @title
(commit) @constant
(location) @label
(command) @variable.special
"#;

pub fn register() {
    LanguageRegistry::singleton().register(
        LANGUAGE,
        &LanguageConfig::new(
            LANGUAGE,
            tree_sitter_diff::LANGUAGE.into(),
            vec![],
            HIGHLIGHTS,
            "",
            "",
        ),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn query_compiles_against_the_grammar() {
        let language: tree_sitter::Language = tree_sitter_diff::LANGUAGE.into();
        assert!(tree_sitter::Query::new(&language, super::HIGHLIGHTS).is_ok());
    }
}
