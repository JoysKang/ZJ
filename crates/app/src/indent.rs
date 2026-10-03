//! Per-file indentation: `.editorconfig`, then a guess from the text, then the language's
//! default. Pure logic apart from reading `.editorconfig` files, which callers do off the UI
//! thread.

use std::path::Path;

/// How the Tab key indents and how wide a tab counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Indent {
    pub hard_tabs: bool,
    pub width: usize,
}

impl Indent {
    pub const fn spaces(width: usize) -> Self {
        Self {
            hard_tabs: false,
            width,
        }
    }

    pub const fn tabs(width: usize) -> Self {
        Self {
            hard_tabs: true,
            width,
        }
    }
}

/// What one source says; unknown parts fall through to the next source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hint {
    pub hard_tabs: Option<bool>,
    pub width: Option<usize>,
}

impl Hint {
    fn or(self, other: Hint) -> Hint {
        Hint {
            hard_tabs: self.hard_tabs.or(other.hard_tabs),
            width: self.width.or(other.width),
        }
    }

    fn complete(self, default: Indent) -> Indent {
        Indent {
            hard_tabs: self.hard_tabs.unwrap_or(default.hard_tabs),
            width: self.width.unwrap_or(default.width).clamp(1, MAX_WIDTH),
        }
    }
}

/// `.editorconfig` may say anything; wider than this is a typo.
const MAX_WIDTH: usize = 16;
/// Lines looked at by [`detect`]; enough for a guess, bounded for 8 MiB files.
const DETECT_LINES: usize = 10_000;
/// Fewer indented lines than this is no evidence either way.
const MIN_INDENTED_LINES: usize = 2;
/// Candidate widths, in VS Code's order of preference on a tie.
const WIDTHS: [usize; 7] = [2, 4, 6, 8, 3, 5, 7];

/// Guesses the indentation from the text (VS Code's `guessIndentation`): tabs or spaces by
/// which more lines start with, the width by the most common change in leading spaces
/// between neighboring non-blank lines.
pub fn detect(text: &str) -> Option<Hint> {
    let mut tab_lines = 0;
    let mut space_lines = 0;
    let mut diffs = [0usize; 9];
    let mut previous = "";
    for line in text.lines().take(DETECT_LINES) {
        let content = line.trim_start_matches([' ', '\t']);
        if content.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - content.len()];
        if indent.contains('\t') {
            tab_lines += 1;
        } else if indent.len() > 1 {
            space_lines += 1;
        }
        if let Some(diff) = spaces_diff(previous, indent)
            && diff < diffs.len()
        {
            diffs[diff] += 1;
        }
        previous = indent;
    }
    if tab_lines + space_lines < MIN_INDENTED_LINES {
        return None;
    }
    let hard_tabs = (tab_lines != space_lines).then_some(tab_lines > space_lines);
    let mut width = None;
    let mut best = 0;
    for candidate in WIDTHS {
        if diffs[candidate] > best {
            best = diffs[candidate];
            width = Some(candidate);
        }
    }
    // Files indented by 2 often have deeper 4-space steps (a closing and opening line).
    if width == Some(4) && diffs[2] > 0 && diffs[2] >= diffs[4] / 2 {
        width = Some(2);
    }
    if hard_tabs == Some(true) {
        width = None;
    }
    Some(Hint { hard_tabs, width })
}

/// The change in spaces between two indentations, when only spaces differ after their
/// common prefix.
fn spaces_diff(a: &str, b: &str) -> Option<usize> {
    let common = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[common..], &b[common..]);
    if a.contains('\t') || b.contains('\t') {
        return None;
    }
    Some(a.len().abs_diff(b.len()))
}

/// The language's indentation when nothing else says.
pub fn language_default(path: &Path) -> Indent {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(name.as_str(), "makefile" | "gnumakefile")
        || name.ends_with(".mk")
        || name.ends_with(".mak")
    {
        return Indent::tabs(4);
    }
    match crate::languages::for_path(path).0 {
        "go" => Indent::tabs(4),
        "python" | "rust" | "java" | "c" | "cpp" => Indent::spaces(4),
        _ => Indent::spaces(2),
    }
}

/// `.editorconfig` first, then [`detect`], then [`language_default`]. Reads files: call it
/// in the background. `root` is the workspace folder, where the search for `.editorconfig`
/// stops (without one it goes up to `/`).
pub fn resolve(path: &Path, root: Option<&Path>, text: &str) -> Indent {
    editorconfig(path, root)
        .or(detect(text).unwrap_or_default())
        .complete(language_default(path))
}

/// The indentation `.editorconfig` files give `path`: the nearest file wins, the search stops
/// at `root` or at a file with `root = true`. Unsupported syntax is ignored.
pub fn editorconfig(path: &Path, root: Option<&Path>) -> Hint {
    let mut found = Vec::new();
    let mut dir = path.parent();
    while let Some(current) = dir {
        match std::fs::read_to_string(current.join(".editorconfig")) {
            Ok(text) => {
                let stop = is_root(&text);
                found.push((current, text));
                if stop {
                    break;
                }
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                eprintln!("event=editorconfig_read_failed error={error}");
            }
            Err(_) => {}
        }
        if Some(current) == root {
            break;
        }
        dir = current.parent();
    }
    let mut properties = Properties::default();
    for (dir, text) in found.iter().rev() {
        if let Ok(relative) = path.strip_prefix(dir) {
            apply(text, &relative.to_string_lossy(), &mut properties);
        }
    }
    properties.hint()
}

#[derive(Default)]
struct Properties {
    style: Option<bool>,
    /// `Some(None)` is `indent_size = tab`.
    size: Option<Option<usize>>,
    tab_width: Option<usize>,
}

impl Properties {
    fn hint(&self) -> Hint {
        let size = self.size.flatten();
        let hard_tabs = self.style;
        let width = if hard_tabs == Some(true) {
            self.tab_width.or(size)
        } else {
            size.or(self.tab_width)
        };
        Hint { hard_tabs, width }
    }
}

fn pairs(text: &str) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            None
        } else if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            Some(Line::Section(section))
        } else {
            let (key, value) = line.split_once('=')?;
            Some(Line::Pair(key.trim(), value.trim()))
        }
    })
}

enum Line<'a> {
    Section(&'a str),
    Pair(&'a str, &'a str),
}

fn is_root(text: &str) -> bool {
    for line in pairs(text) {
        match line {
            Line::Section(_) => return false,
            Line::Pair(key, value) => {
                if key.eq_ignore_ascii_case("root") && value.eq_ignore_ascii_case("true") {
                    return true;
                }
            }
        }
    }
    false
}

/// Applies the sections of one file that match `relative` (the path below the file's folder).
fn apply(text: &str, relative: &str, properties: &mut Properties) {
    let mut matching = false;
    for line in pairs(text) {
        match line {
            Line::Section(pattern) => matching = section_matches(pattern, relative),
            Line::Pair(key, value) if matching => {
                let value = value.to_ascii_lowercase();
                let unset = value == "unset";
                match key.to_ascii_lowercase().as_str() {
                    "indent_style" => {
                        properties.style = match value.as_str() {
                            "tab" => Some(true),
                            "space" => Some(false),
                            _ if unset => None,
                            _ => properties.style,
                        }
                    }
                    "indent_size" => {
                        properties.size = match value.as_str() {
                            "tab" => Some(None),
                            _ if unset => None,
                            _ => value.parse().ok().map(Some).or(properties.size),
                        }
                    }
                    "tab_width" => {
                        properties.tab_width = if unset {
                            None
                        } else {
                            value.parse().ok().or(properties.tab_width)
                        }
                    }
                    _ => {}
                }
            }
            Line::Pair(..) => {}
        }
    }
}

/// EditorConfig globs: `*`, `**`, `?` and one level of `{a,b}`; a pattern without `/`
/// matches the file name at any depth. Anything else (`[...]`, `{1..3}`) never matches.
fn section_matches(pattern: &str, relative: &str) -> bool {
    if pattern.contains(['[', '\\']) || pattern.contains("..") {
        return false;
    }
    let (pattern, subject) = if pattern.contains('/') {
        (pattern.trim_start_matches('/'), relative)
    } else {
        (pattern, relative.rsplit('/').next().unwrap_or(relative))
    };
    expand_braces(pattern)
        .iter()
        .any(|pattern| glob(pattern.as_bytes(), subject.as_bytes()))
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let Some(close) = pattern[open..].find('}').map(|i| open + i) else {
        return vec![pattern.to_owned()];
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    pattern[open + 1..close]
        .split(',')
        .flat_map(|choice| expand_braces(&format!("{head}{choice}{tail}")))
        .collect()
}

fn glob(pattern: &[u8], subject: &[u8]) -> bool {
    match pattern {
        [] => subject.is_empty(),
        [b'*', b'*', rest @ ..] => (0..=subject.len()).any(|i| glob(rest, &subject[i..])),
        [b'*', rest @ ..] => (0..=subject.len())
            .take_while(|&i| i == 0 || subject[i - 1] != b'/')
            .any(|i| glob(rest, &subject[i..])),
        [b'?', rest @ ..] => {
            matches!(subject, [first, tail @ ..] if *first != b'/' && glob(rest, tail))
        }
        [first, rest @ ..] => matches!(subject, [s, tail @ ..] if s == first && glob(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn hint(hard_tabs: Option<bool>, width: Option<usize>) -> Option<Hint> {
        Some(Hint { hard_tabs, width })
    }

    #[test]
    fn detects_tabs() {
        let go = "package main\n\nfunc main() {\n\tif x {\n\t\ty()\n\t}\n}\n";
        assert_eq!(detect(go), hint(Some(true), None));
    }

    #[test]
    fn detects_two_and_four_spaces() {
        let two = "a:\n  b:\n    c: 1\n  d: 2\ne:\n  f: 3\n";
        assert_eq!(detect(two), hint(Some(false), Some(2)));
        let four = "def f():\n    if x:\n        y()\n    return 1\n\nclass A:\n    pass\n";
        assert_eq!(detect(four), hint(Some(false), Some(4)));
    }

    #[test]
    fn majority_wins_when_mixed() {
        // Tabs for indentation; one space-indented continuation and doc-comment stars.
        let mixed = "func f() {\n\ta()\n\tb()\n\t/*\n\t * doc\n\t */\n    c()\n\td()\n}\n";
        assert_eq!(detect(mixed), hint(Some(true), None));
        let even = "a\n\tb\nc\n    d\n";
        assert_eq!(detect(even), hint(None, Some(4)));
    }

    #[test]
    fn too_few_samples_is_no_guess() {
        assert_eq!(detect(""), None);
        assert_eq!(detect("one line\nanother\n"), None);
        assert_eq!(detect("a\n    b\n\n   \n"), None);
    }

    #[test]
    fn language_defaults() {
        let of = |p: &str| language_default(Path::new(p));
        assert_eq!(of("main.go"), Indent::tabs(4));
        assert_eq!(of("Makefile"), Indent::tabs(4));
        assert_eq!(of("rules.mk"), Indent::tabs(4));
        assert_eq!(of("a.py"), Indent::spaces(4));
        assert_eq!(of("a.rs"), Indent::spaces(4));
        assert_eq!(of("A.java"), Indent::spaces(4));
        assert_eq!(of("a.hpp"), Indent::spaces(4));
        assert_eq!(of("a.ts"), Indent::spaces(2));
        assert_eq!(of("README"), Indent::spaces(2));
    }

    #[test]
    fn section_globs() {
        assert!(section_matches("*", "src/a.rs"));
        assert!(section_matches("*.rs", "src/deep/a.rs"));
        assert!(!section_matches("*.rs", "src/a.go"));
        assert!(section_matches("*.{js,ts}", "web/x.ts"));
        assert!(!section_matches("*.{js,ts}", "web/x.tsx"));
        assert!(section_matches("{Makefile,*.mk}", "Makefile"));
        assert!(section_matches("src/*.go", "src/a.go"));
        assert!(!section_matches("src/*.go", "src/x/a.go"));
        assert!(section_matches("src/**.go", "src/x/a.go"));
        assert!(section_matches("/lib/**", "lib/a/b.c"));
        assert!(section_matches("a?.c", "ab.c"));
        // Unsupported syntax never matches.
        assert!(!section_matches("*.[ch]", "a.c"));
        assert!(!section_matches("{1..3}.txt", "1.txt"));
    }

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zj-indent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("project/src")).unwrap();
        root
    }

    #[test]
    fn editorconfig_sections_nearest_wins() {
        let root = fixture("sections");
        let project = root.join("project");
        std::fs::write(
            project.join(".editorconfig"),
            "root = true\n\n[*]\nindent_style = space\nindent_size = 2\n\n\
             [*.go]\nindent_style = tab\n\n[*.{py,rs}]\nindent_size = 4\n\n[*.[ch]]\nindent_size = 8\n",
        )
        .unwrap();
        std::fs::write(
            project.join("src/.editorconfig"),
            "; nested\n[*.rs]\nindent_size = 3\ntab_width = 6\n",
        )
        .unwrap();
        let of = |p: &str| editorconfig(&project.join(p), Some(&project));
        assert_eq!(
            of("a.js"),
            Hint {
                hard_tabs: Some(false),
                width: Some(2)
            }
        );
        assert_eq!(
            of("a.go"),
            Hint {
                hard_tabs: Some(true),
                width: Some(2)
            }
        );
        assert_eq!(
            of("a.py"),
            Hint {
                hard_tabs: Some(false),
                width: Some(4)
            }
        );
        assert_eq!(
            of("src/a.rs"),
            Hint {
                hard_tabs: Some(false),
                width: Some(3)
            }
        );
        assert_eq!(
            of("a.c"),
            Hint {
                hard_tabs: Some(false),
                width: Some(2)
            }
        );
        // `.editorconfig` beats the text, which beats the language.
        let rust = "fn f() {\n\tx();\n\ty();\n}\n";
        assert_eq!(
            resolve(&project.join("src/a.rs"), Some(&project), rust),
            Indent::spaces(3)
        );
        assert_eq!(
            resolve(&project.join("a.go"), Some(&project), "x\n    y\n    z\n"),
            Indent::tabs(2)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn editorconfig_stops_at_root_true_and_workspace() {
        let root = fixture("root");
        let project = root.join("project");
        std::fs::write(root.join(".editorconfig"), "[*]\nindent_style = tab\n").unwrap();
        std::fs::write(project.join(".editorconfig"), "[*]\nindent_size = 8\n").unwrap();
        let file = project.join("src/a.ts");
        // Without `root = true`, the search goes on above the project.
        assert_eq!(
            editorconfig(&file, None),
            Hint {
                hard_tabs: Some(true),
                width: Some(8)
            }
        );
        // The workspace folder bounds it.
        assert_eq!(
            editorconfig(&file, Some(&project)),
            Hint {
                hard_tabs: None,
                width: Some(8)
            }
        );
        std::fs::write(
            project.join(".editorconfig"),
            "root = true\n[*]\nindent_size = 8\n",
        )
        .unwrap();
        assert_eq!(
            editorconfig(&file, None),
            Hint {
                hard_tabs: None,
                width: Some(8)
            }
        );
        // Detection fills in what `.editorconfig` leaves open.
        assert_eq!(resolve(&file, None, "a\n\tb\n\tc\n"), Indent::tabs(8));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn falls_back_to_language_default() {
        let root = fixture("default");
        let file = root.join("project/src/main.go");
        assert_eq!(
            resolve(&file, Some(&root.join("project")), "package main\n"),
            Indent::tabs(4)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
