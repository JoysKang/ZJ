//! Ordered name rules shared by file tree and search visibility checks.

use crate::text_search::Globs;
use std::{
    ffi::OsStr,
    fmt,
    path::{Component, Path},
    sync::{Arc, LazyLock},
};

struct Rule {
    glob: Globs,
    hidden: bool,
}

#[derive(Clone)]
pub struct HiddenFiles {
    patterns: Arc<Vec<String>>,
    rules: Arc<Vec<Rule>>,
}

impl HiddenFiles {
    pub fn new(patterns: Vec<String>) -> Result<Self, String> {
        let rules = patterns
            .iter()
            .map(|pattern| {
                let (hidden, body) = match pattern.strip_prefix('!') {
                    Some(body) => (false, body),
                    None => (true, pattern.as_str()),
                };
                if body.trim().is_empty() || body.contains('/') {
                    return Err(format!(
                        "hidden_files 规则 {pattern:?} 无效：请填写文件或目录名称模式，不支持空模式或路径"
                    ));
                }
                let glob = Globs::parse(body)
                    .map_err(|error| format!("hidden_files 规则 {pattern:?} 无效：{error}"))?;
                Ok(Rule { glob, hidden })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            patterns: Arc::new(patterns),
            rules: Arc::new(rules),
        })
    }

    pub fn patterns(&self) -> &[String] {
        self.patterns.as_slice()
    }

    pub fn name_hidden(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.glob.matches(&name))
            .is_some_and(|rule| rule.hidden)
    }

    pub fn path_hidden(&self, relative: &Path) -> bool {
        relative.components().any(|component| match component {
            Component::Normal(name) => self.name_hidden(name),
            _ => false,
        })
    }
}

impl Default for HiddenFiles {
    fn default() -> Self {
        static DEFAULT: LazyLock<HiddenFiles> = LazyLock::new(|| {
            HiddenFiles::new(
                [
                    ".*",
                    "!.github",
                    "!.gitlab",
                    "!.cargo",
                    "!.gitignore",
                    "!.gitattributes",
                    "!.gitmodules",
                    "!.gitlab-ci.yml",
                    "!.editorconfig",
                    "!.dockerignore",
                    "!.env*",
                    "!.prettierrc*",
                    "!.eslintrc*",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            )
            .expect("default hidden file rules are valid")
        });
        DEFAULT.clone()
    }
}

impl fmt::Debug for HiddenFiles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HiddenFiles")
            .field("patterns", &self.patterns)
            .finish()
    }
}

impl PartialEq for HiddenFiles {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for HiddenFiles {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_keep_project_configuration_visible() {
        let rules = HiddenFiles::default();
        for name in [
            ".github",
            ".gitlab",
            ".cargo",
            ".gitignore",
            ".gitattributes",
            ".gitmodules",
            ".gitlab-ci.yml",
            ".editorconfig",
            ".dockerignore",
            ".env",
            ".env.local",
            ".prettierrc.json",
            ".eslintrc.js",
            "src",
        ] {
            assert!(!rules.name_hidden(OsStr::new(name)), "{name}");
        }
        assert!(rules.name_hidden(OsStr::new(".cache")));
        assert!(rules.path_hidden(Path::new(".cache/.env")));
        assert!(!rules.path_hidden(Path::new(".github/workflows/ci.yml")));
        assert!(!rules.path_hidden(Path::new("./src/main.rs")));
    }

    #[test]
    fn last_matching_rule_wins_for_each_name() {
        let rules =
            HiddenFiles::new(vec!["*.log".into(), "!keep.log".into(), "keep.*".into()]).unwrap();
        assert!(rules.name_hidden(OsStr::new("keep.log")));
        let rules =
            HiddenFiles::new(vec!["*.log".into(), "!keep.log".into(), "cache".into()]).unwrap();
        assert!(rules.name_hidden(OsStr::new("debug.log")));
        assert!(!rules.name_hidden(OsStr::new("keep.log")));
        assert!(rules.path_hidden(Path::new("src/cache/keep.log")));
        assert!(!rules.path_hidden(Path::new("src/keep.log")));
        assert!(Arc::ptr_eq(&rules.rules, &rules.clone().rules));
    }

    #[test]
    fn empty_list_and_invalid_rules() {
        assert!(
            !HiddenFiles::new(Vec::new())
                .unwrap()
                .path_hidden(Path::new(".cache/file"))
        );
        for pattern in ["", "!", "src/cache", "[z-a]"] {
            assert!(HiddenFiles::new(vec![pattern.into()]).is_err(), "{pattern}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_still_match_dot_prefixes() {
        use std::os::unix::ffi::OsStrExt;
        let rules = HiddenFiles::default();
        assert!(rules.name_hidden(OsStr::from_bytes(b".cache\xff")));
        assert!(!rules.name_hidden(OsStr::from_bytes(b"plain\xff")));
    }
}
