//! Full-text search over a workspace, VS Code style: case / whole word / regular expression,
//! comma-separated include and exclude globs, and the default excludes (`.gitignore`, caches,
//! dependency folders) unless turned off. Runs on background threads, streams results, stops
//! on cancellation, and skips binaries and files over [`MAX_FILE_BYTES`].

use regex::bytes::{Regex, RegexBuilder};
use std::{
    fs,
    io::Read,
    ops::Range,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use workspace_editor_core::is_excluded_dir;

pub const MAX_MATCHES: usize = 10_000;
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Files beyond the index walked when excludes are off (ignored folders: `target/` alone can
/// hold hundreds of thousands).
pub const MAX_WALK_FILES: usize = 200_000;
/// Bytes checked for NUL before a file counts as binary.
const BINARY_PROBE: usize = 8 * 1024;
/// A preview keeps this many characters before the first match on its line (VS Code keeps
/// a few words; more pushes the match out of a narrow sidebar, especially in CJK text).
const PREVIEW_LEAD: usize = 12;
const PREVIEW_MAX: usize = 240;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Comma-separated globs (`*.py`, `src/**`, `./api`); empty means everything.
    pub include: String,
    pub exclude: String,
    /// `.gitignore` applies (and dot files hidden by default stay out); the usual folders to
    /// skip are in `exclude`, where the user sees and edits them.
    pub use_excludes: bool,
    pub show_hidden: bool,
}

#[derive(Clone, Debug)]
pub struct LineMatch {
    /// 0-based line and byte column of the first match, for opening the file there.
    pub line: u32,
    pub column: u32,
    pub len: u32,
    /// The line, shortened around the first match, and the matches inside it. `ranges[i]`
    /// is `spans[i]` moved into the preview (empty when that match was cut off).
    pub preview: String,
    pub ranges: Vec<Range<usize>>,
    /// Byte ranges of the matches in the file, for replacing exactly these.
    pub spans: Vec<Range<usize>>,
}

#[derive(Clone, Debug)]
pub struct FileMatches {
    pub path: PathBuf,
    pub relative: PathBuf,
    pub lines: Vec<LineMatch>,
    pub count: usize,
    /// The file as it was searched; a replace skips the file if it changed since.
    pub stamp: Option<crate::files::FileStamp>,
}

#[derive(Default)]
pub struct Progress {
    pub results: Mutex<Vec<FileMatches>>,
    pub matches: AtomicUsize,
    pub files_searched: AtomicUsize,
    pub truncated: AtomicBool,
    /// Excludes off: there were more ignored files than `MAX_WALK_FILES`, some went unsearched.
    pub files_capped: AtomicBool,
    pub done: AtomicBool,
}

/// A comma- (or line-) separated glob list. `./x` and patterns with `/` are anchored at the root; a bare
/// pattern (`*.py`, `node_modules`) matches at any depth. A match on a folder covers what is
/// inside it.
pub struct Globs(Option<regex::Regex>);

impl Globs {
    pub fn parse(list: &str) -> Result<Self, String> {
        // Commas inside `{a,b}` belong to the pattern.
        let mut patterns = Vec::new();
        let (mut depth, mut start) = (0i32, 0);
        for (i, c) in list.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                // A line break (⇧⏎ in the list) separates like a comma.
                ',' | '\n' if depth <= 0 => {
                    patterns.push(&list[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        patterns.push(&list[start..]);
        let parts: Vec<String> = patterns
            .into_iter()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|pattern| {
                let (anchored, body) = match pattern.strip_prefix("./") {
                    Some(rest) => (true, rest),
                    None => (pattern.contains('/'), pattern.trim_start_matches('/')),
                };
                let body = body.trim_end_matches('/');
                let glob = glob_to_regex(body);
                if anchored {
                    format!("^{glob}(?:/.*)?$")
                } else {
                    format!("(?:^|/){glob}(?:/.*)?$")
                }
            })
            .collect();
        if parts.is_empty() {
            return Ok(Self(None));
        }
        regex::Regex::new(&parts.join("|"))
            .map(|regex| Self(Some(regex)))
            .map_err(|error| format!("文件匹配模式无效：{error}"))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn matches(&self, relative: &str) -> bool {
        self.0
            .as_ref()
            .is_some_and(|regex| regex.is_match(relative))
    }
}

/// The include pattern for one folder (relative to the root): anchored, with characters a glob
/// would read as syntax made literal. A comma cannot be written in a glob list, so it matches
/// any one character instead.
pub fn folder_pattern(relative: &str) -> String {
    let mut pattern = String::from("./");
    for c in relative.chars() {
        match c {
            '*' | '?' | '[' | '{' | '}' => {
                pattern.push('[');
                pattern.push(c);
                pattern.push(']');
            }
            ',' => pattern.push('?'),
            c => pattern.push(c),
        }
    }
    pattern
}

fn glob_to_regex(glob: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    let mut braces = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                // `**/` matches zero or more folders; a trailing `**` anything.
                if chars.get(i + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    i += 2;
                } else {
                    out.push_str(".*");
                    i += 1;
                }
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '{' => {
                braces += 1;
                out.push_str("(?:");
            }
            '}' if braces > 0 => {
                braces -= 1;
                out.push(')');
            }
            ',' if braces > 0 => out.push('|'),
            '[' => match chars[i..].iter().position(|c| *c == ']') {
                Some(end) if end > 1 => {
                    let class: String = chars[i + 1..i + end].iter().collect();
                    let class = class.replacen('!', "^", usize::from(class.starts_with('!')));
                    out.push('[');
                    // `[` would open a nested class in Rust's regex, `&&` / `~~` are set
                    // operations there: all literal in a glob.
                    for c in class.chars() {
                        if matches!(c, '\\' | '[' | '&' | '~') {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push(']');
                    i += end;
                }
                _ => out.push_str("\\["),
            },
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out.push_str(&")".repeat(braces));
    out
}

pub struct Matcher {
    regex: Regex,
    /// Matches stay within one line unless the pattern asks for a line break.
    multiline: bool,
    include: Globs,
    exclude: Globs,
    use_excludes: bool,
    show_hidden: bool,
}

impl Matcher {
    pub fn new(options: &Options) -> Result<Self, String> {
        let pattern =
            crate::replace::pattern_source(&options.pattern, options.whole_word, options.regex);
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .multi_line(true)
            .crlf(true)
            .size_limit(16 * 1024 * 1024)
            .build()
            .map_err(|error| format!("正则表达式无效：{error}"))?;
        Ok(Self {
            regex,
            multiline: options.regex && options.pattern.contains("\\n"),
            include: Globs::parse(&options.include)?,
            exclude: Globs::parse(&options.exclude)?,
            use_excludes: options.use_excludes,
            show_hidden: options.show_hidden,
        })
    }

    /// Whether a workspace-relative file path is searched at all.
    pub fn wants(&self, relative: &Path) -> bool {
        let text = relative.to_string_lossy();
        if !self.include.is_empty() && !self.include.matches(&text) {
            return false;
        }
        if self.exclude.matches(&text) {
            return false;
        }
        if self.use_excludes && !self.show_hidden && crate::files::path_hidden_by_default(relative)
        {
            return false;
        }
        true
    }

    /// Matches in one file's bytes, or `None` for a binary file.
    pub fn search_bytes(&self, bytes: &[u8], budget: usize) -> Option<(Vec<LineMatch>, usize)> {
        if bytes[..bytes.len().min(BINARY_PROBE)].contains(&0) {
            return None;
        }
        let mut lines: Vec<LineMatch> = Vec::new();
        let mut count = 0;
        let mut line = 0u32;
        let mut scanned = 0usize;
        // Byte bounds of each line in `lines`, for building the previews afterwards.
        let mut bounds: Vec<(usize, usize)> = Vec::new();
        // Line by line unless the pattern spans lines (as `replace::Finder` does), so a
        // candidate running over a line break never hides the matches within a line.
        let regions: Box<dyn Iterator<Item = Range<usize>>> = if self.multiline {
            Box::new(std::iter::once(0..bytes.len()))
        } else {
            let mut start = 0;
            Box::new(bytes.split_inclusive(|b| *b == b'\n').map(move |line| {
                let mut end = start + line.len();
                if bytes[..end].ends_with(b"\n") {
                    end -= 1;
                }
                if bytes[start..end].ends_with(b"\r") {
                    end -= 1;
                }
                let range = start..end;
                start += line.len();
                range
            }))
        };
        let found_all = regions.flat_map(|region| {
            self.regex
                .find_iter(&bytes[region.clone()])
                .map(move |found| found.start() + region.start..found.end() + region.start)
        });
        for found in found_all {
            if found.start == found.end {
                continue;
            }
            if count >= budget {
                break;
            }
            count += 1;
            line += memchr_count(&bytes[scanned..found.start]) as u32;
            scanned = found.start;
            let start = bytes[..found.start]
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |i| i + 1);
            let end = bytes[found.start..]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |i| found.start + i);
            let range = found.start - start..found.end.min(end) - start;
            if bounds.last() == Some(&(start, end)) {
                if let Some(last) = lines.last_mut() {
                    last.ranges.push(range);
                    last.spans.push(found.clone());
                }
                continue;
            }
            bounds.push((start, end));
            // The editor drops a byte order mark, so first-line columns start after it.
            let bom = if start == 0 && bytes.starts_with("\u{feff}".as_bytes()) {
                3
            } else {
                0
            };
            lines.push(LineMatch {
                line,
                column: range.start.saturating_sub(bom) as u32,
                len: (range.end - range.start) as u32,
                preview: String::new(),
                ranges: vec![range],
                spans: vec![found.clone()],
            });
        }
        for (found, (start, end)) in lines.iter_mut().zip(bounds) {
            let (preview, ranges) = preview_line(&bytes[start..end], &found.ranges);
            found.preview = preview;
            found.ranges = ranges;
        }
        Some((lines, count))
    }
}

fn memchr_count(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == b'\n').count()
}

/// The line shortened around its first match, and the match ranges moved into it. Lines
/// that are not UTF-8 are shown lossily; ranges that no longer line up are dropped.
fn preview_line(text: &[u8], matches: &[Range<usize>]) -> (String, Vec<Range<usize>>) {
    let text = text.strip_suffix(b"\r").unwrap_or(text);
    // Work in the decoded line: an invalid byte becomes a 3-byte U+FFFD there, so the match
    // offsets move with it.
    let (lossy, to_lossy) = decode_lossy(text);
    let matches: Vec<Range<usize>> = matches
        .iter()
        .map(|r| to_lossy(r.start.min(text.len()))..to_lossy(r.end.min(text.len())))
        .collect();
    let matches = matches.as_slice();
    let first = matches.first().map_or(0, |r| r.start);
    // Keep a little context before the first match, starting at a character boundary.
    let lead_bytes = lossy[..first.min(lossy.len())]
        .char_indices()
        .rev()
        .nth(PREVIEW_LEAD)
        .map_or(0, |(i, _)| i);
    let indent = lossy[lead_bytes..]
        .len()
        .saturating_sub(lossy[lead_bytes..].trim_start().len());
    let cut = lead_bytes + indent;
    let mut preview: String = lossy[cut..].chars().take(PREVIEW_MAX).collect();
    if cut > 0 && lead_bytes > 0 {
        preview.insert(0, '…');
    }
    let shift = |offset: usize| -> usize {
        let prefix = if cut > 0 && lead_bytes > 0 {
            '…'.len_utf8()
        } else {
            0
        };
        offset.saturating_sub(cut) + prefix
    };
    // One range per match, kept in step with the spans; a match that was cut off is empty.
    let ranges = matches
        .iter()
        .map(|r| {
            if r.start < cut || !lossy.is_char_boundary(r.start.min(lossy.len())) {
                return 0..0;
            }
            let range = shift(r.start)..shift(r.end).min(preview.len());
            if range.start < range.end
                && preview.is_char_boundary(range.start)
                && preview.is_char_boundary(range.end)
            {
                range
            } else {
                0..0
            }
        })
        .collect();
    (preview, ranges)
}

/// The line decoded lossily, and a map from a byte offset in `bytes` to the matching offset
/// in the decoded text (an offset inside an invalid sequence maps to its replacement).
fn decode_lossy(bytes: &[u8]) -> (String, impl Fn(usize) -> usize) {
    let mut text = String::with_capacity(bytes.len());
    // (offset in `bytes`, offset in `text`) where each invalid sequence starts and ends.
    let mut breaks: Vec<(usize, usize, usize)> = Vec::new();
    let mut rest = bytes;
    let mut at = 0;
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                // The first `valid_up_to` bytes are valid UTF-8.
                text.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                let bad = error.error_len().unwrap_or(rest.len() - valid);
                breaks.push((at + valid, at + valid + bad, text.len()));
                text.push(char::REPLACEMENT_CHARACTER);
                at += valid + bad;
                rest = &rest[valid + bad..];
            }
        }
    }
    let map = move |offset: usize| {
        // Each invalid sequence before `offset` changes the length by 3 − its byte count.
        let mut lossy = offset as isize;
        for &(start, end, replaced) in &breaks {
            if offset < start {
                break;
            }
            if offset < end {
                return replaced;
            }
            lossy += 3 - (end - start) as isize;
        }
        lossy.max(0) as usize
    };
    (text, map)
}

/// Searches `files` (workspace-relative) under `root` with a few threads, appending to
/// `progress` as files finish, until done, cancelled or [`MAX_MATCHES`] is reached.
pub fn run(
    root: &Path,
    files: Vec<PathBuf>,
    matcher: &Matcher,
    progress: &Progress,
    cancel: &AtomicBool,
) {
    // Marks the search done however it ends, so the view never polls a search that died.
    struct Finished<'a>(&'a AtomicBool);
    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }
    let _finished = Finished(&progress.done);
    let queue = Mutex::new(files.into_iter());
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut buffer = Vec::new();
                loop {
                    if cancel.load(Ordering::Relaxed) || progress.truncated.load(Ordering::Relaxed)
                    {
                        return;
                    }
                    let Some(relative) = queue.lock().unwrap().next() else {
                        return;
                    };
                    let path = root.join(&relative);
                    buffer.clear();
                    // Non-blocking, so a named pipe (or a symlink to one) opens at once and
                    // is skipped below instead of waiting for a writer; reads of regular
                    // files are unaffected.
                    let Ok(file) = fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&path)
                    else {
                        continue;
                    };
                    let metadata = file.metadata().ok();
                    if metadata
                        .as_ref()
                        .is_some_and(|m| !m.is_file() || m.len() > MAX_FILE_BYTES)
                    {
                        continue;
                    }
                    let stamp = metadata.as_ref().map(crate::files::FileStamp::of);
                    if file
                        .take(MAX_FILE_BYTES + 1)
                        .read_to_end(&mut buffer)
                        .is_err()
                    {
                        continue;
                    }
                    progress.files_searched.fetch_add(1, Ordering::Relaxed);
                    let used = progress.matches.load(Ordering::Relaxed);
                    let budget = MAX_MATCHES.saturating_sub(used);
                    let Some((lines, count)) = matcher.search_bytes(&buffer, budget) else {
                        continue;
                    };
                    if count == 0 {
                        continue;
                    }
                    if progress.matches.fetch_add(count, Ordering::Relaxed) + count >= MAX_MATCHES {
                        progress.truncated.store(true, Ordering::Relaxed);
                    }
                    progress.results.lock().unwrap().push(FileMatches {
                        path,
                        relative,
                        lines,
                        count,
                        stamp,
                    });
                }
            });
        }
    });
}

/// Every file below `root` except `.git` and symlinks, for searching with excludes off: all
/// of `known` (the index, i.e. the files that are not ignored), then what the index leaves out,
/// walked breadth first, at most `limit` of those. `false` when the limit cut that short; the
/// known files are never dropped (a deep `target/` once crowded out every source file).
pub fn walk_all(
    root: &Path,
    known: Vec<PathBuf>,
    limit: usize,
    cancel: &AtomicBool,
) -> (Vec<PathBuf>, bool) {
    let seen: std::collections::HashSet<PathBuf> = known.iter().cloned().collect();
    let mut files = known;
    let mut extra = 0;
    let mut complete = true;
    let mut pending = std::collections::VecDeque::from([PathBuf::new()]);
    'walk: while let Some(dir) = pending.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(entries) = fs::read_dir(root.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if kind.is_dir() && name.as_bytes() != b".git" {
                pending.push_back(dir.join(name));
            } else if kind.is_file() {
                let path = dir.join(name);
                if seen.contains(&path) {
                    continue;
                }
                if extra >= limit {
                    complete = false;
                    break 'walk;
                }
                extra += 1;
                files.push(path);
            }
        }
    }
    files.sort();
    (files, complete)
}

/// Files from the quick-open index (already following `.gitignore` and the excluded
/// folders) that the include / exclude globs want.
pub fn filter(
    files: impl IntoIterator<Item = PathBuf>,
    matcher: &Matcher,
    use_excludes: bool,
) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|relative| {
            (!use_excludes
                || !relative
                    .components()
                    .any(|c| is_excluded_dir(c.as_os_str())))
                && matcher.wants(relative)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crowd_of_ignored_files_never_hides_the_indexed_ones() {
        let root = std::env::temp_dir().join(format!("zj-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("crates/app/src")).unwrap();
        fs::write(root.join("crates/app/src/main.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::create_dir_all(root.join("target/debug/deps")).unwrap();
        for i in 0..5 {
            fs::write(root.join(format!("target/debug/deps/lib-{i}.d")), "").unwrap();
        }
        fs::write(root.join("target/.rustc_info.json"), "").unwrap();
        let known: Vec<PathBuf> = ["crates/app/src/main.rs", "README.md"]
            .map(PathBuf::from)
            .to_vec();
        let stop = AtomicBool::new(false);
        // Room for two ignored files: the shallowest one comes first, the index stays whole.
        let (files, complete) = walk_all(&root, known.clone(), 2, &stop);
        assert!(!complete);
        assert_eq!(files.len(), 4);
        assert!(known.iter().all(|k| files.contains(k)));
        assert!(files.contains(&PathBuf::from("target/.rustc_info.json")));
        let (files, complete) = walk_all(&root, known, 100, &stop);
        assert!(complete);
        assert_eq!(files.len(), 8);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_folder_pattern_matches_only_that_folder() {
        let pattern = folder_pattern("src/a,b/c*{x}[y]");
        assert_eq!(pattern, "./src/a?b/c[*][{]x[}][[]y]");
        let globs = Globs::parse(&pattern).unwrap();
        assert!(globs.matches("src/a,b/c*{x}[y]/main.rs"));
        assert!(globs.matches("src/a,b/c*{x}[y]/deep/mod.rs"));
        assert!(!globs.matches("src/a,b/cd{x}[y]/main.rs"));
        assert!(!globs.matches("other/src/a,b/c*{x}[y]/main.rs"));
        assert!(
            Globs::parse(&folder_pattern("app"))
                .unwrap()
                .matches("app/x.rs")
        );
        // Typed by hand: `&&` in a class is literal, not Rust regex's intersection.
        assert!(Globs::parse("[&&~]x.rs").unwrap().matches("&x.rs"));
        let lines = Globs::parse("*.md\n target ,\n*.log").unwrap();
        assert!(
            lines.matches("a/README.md") && lines.matches("target/x") && lines.matches("x.log")
        );
        assert!(!lines.matches("src/main.rs"));
    }

    fn options(pattern: &str) -> Options {
        Options {
            pattern: pattern.into(),
            use_excludes: true,
            ..Default::default()
        }
    }

    #[test]
    fn globs_follow_vs_code_rules() {
        let globs = Globs::parse("*.py, ./api, src/**/*.ts, node_modules, {a,b}.md").unwrap();
        for yes in [
            "main.py",
            "deep/x/main.py",
            "api/routes.go",
            "src/ui/x.ts",
            "src/x.ts",
            "web/node_modules/pkg/i.js",
            "a.md",
            "docs/b.md",
        ] {
            assert!(globs.matches(yes), "{yes}");
        }
        for no in [
            "main.pyc",
            "web/api/routes.go",
            "lib/src/x.ts",
            "c.md",
            "node_modules_x/y",
        ] {
            assert!(!globs.matches(no), "{no}");
        }
        assert!(Globs::parse("").unwrap().is_empty());
        assert!(Globs::parse("[!a]x").unwrap().matches("bx"));
    }

    #[test]
    fn matching_case_words_regex_and_previews() {
        let text = b"Hello world\n  let hello = 1; // HELLO\nnothing\nhello_there\n";
        let m = Matcher::new(&options("hello")).unwrap();
        let (lines, count) = m.search_bytes(text, 100).unwrap();
        assert_eq!(count, 4);
        assert_eq!(lines.iter().map(|l| l.line).collect::<Vec<_>>(), [0, 1, 3]);
        assert_eq!(lines[1].ranges.len(), 2);
        assert_eq!(lines[1].preview, "let hello = 1; // HELLO");
        assert_eq!(&lines[1].preview[lines[1].ranges[0].clone()], "hello");
        assert_eq!(lines[1].column, 6);
        let m = Matcher::new(&Options {
            case_sensitive: true,
            whole_word: true,
            ..options("hello")
        })
        .unwrap();
        assert_eq!(m.search_bytes(text, 100).unwrap().1, 1);
        let m = Matcher::new(&Options {
            regex: true,
            ..options(r"hel+o\s\w+")
        })
        .unwrap();
        assert_eq!(m.search_bytes(text, 100).unwrap().1, 1);
        assert!(
            Matcher::new(&Options {
                regex: true,
                ..options("(")
            })
            .is_err()
        );
        // Literal mode escapes regex syntax.
        let m = Matcher::new(&options("a.b")).unwrap();
        assert_eq!(m.search_bytes(b"a.b axb", 10).unwrap().1, 1);
        assert!(m.search_bytes(b"a.b\0", 10).is_none());
        assert_eq!(m.search_bytes(b"a.b a.b a.b", 2).unwrap().1, 2);
        let long = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        let (lines, _) = Matcher::new(&options("needle"))
            .unwrap()
            .search_bytes(long.as_bytes(), 1)
            .unwrap();
        assert!(
            lines[0].preview.starts_with('…')
                && lines[0].preview.chars().count() <= PREVIEW_MAX + 1
        );
        assert_eq!(&lines[0].preview[lines[0].ranges[0].clone()], "needle");
    }

    #[test]
    fn first_line_columns_skip_a_byte_order_mark() {
        let m = Matcher::new(&options("x")).unwrap();
        let (lines, _) = m.search_bytes("\u{feff}ab x\nx\n".as_bytes(), 100).unwrap();
        assert_eq!((lines[0].column, lines[1].column), (3, 0));
        // The spans stay in file bytes, for replacing on disk.
        assert_eq!(lines[0].spans[0], 6..7);
    }

    #[test]
    fn workspace_search_keeps_matches_within_lines() {
        let m = Matcher::new(&Options {
            regex: true,
            ..options("[^,]+")
        })
        .unwrap();
        let (lines, count) = m.search_bytes(b"a,b\nc,d\n", 100).unwrap();
        assert_eq!(count, 4);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].spans, [4..5, 6..7]);
    }

    #[test]
    fn previews_of_lines_that_are_not_utf8_keep_their_matches() {
        // "中文 hello" in GBK: four invalid bytes for UTF-8, then ASCII.
        let line = b"\xd6\xd0\xce\xc4 hello";
        let (preview, ranges) = preview_line(line, std::slice::from_ref(&(5..10)));
        assert_eq!(&preview[ranges[0].clone()], "hello");
        // Matches right after lone invalid bytes.
        let (preview, ranges) = preview_line(b"\xe9a \xe9b", &[1..2, 4..5]);
        assert_eq!(&preview[ranges[0].clone()], "a");
        assert_eq!(&preview[ranges[1].clone()], "b");
        let (preview, _) = preview_line(b"\xff\xfe x", std::slice::from_ref(&(1..3)));
        assert!(preview.ends_with(" x"));
    }

    #[test]
    fn excludes_includes_and_hidden_folders() {
        let m = Matcher::new(&Options {
            include: "*.py".into(),
            exclude: "tests".into(),
            ..options("x")
        })
        .unwrap();
        assert!(m.wants(Path::new("app/main.py")));
        assert!(!m.wants(Path::new("app/main.rs")));
        assert!(!m.wants(Path::new("app/tests/t.py")));
        assert!(!m.wants(Path::new(".venv/lib/x.py")));
        // Only what 排除的文件 lists is skipped: the usual folders are in its default text.
        assert!(m.wants(Path::new("dist/x.py")));
        let defaults = Matcher::new(&Options {
            exclude: crate::settings::SEARCH_EXCLUDE_DEFAULT.into(),
            ..options("x")
        })
        .unwrap();
        for skipped in [
            "dist/x.py",
            "web/node_modules/a/x.js",
            "target/debug/deps/x.d",
            "web/app.min.js",
            "web/app.js.map",
        ] {
            assert!(!defaults.wants(Path::new(skipped)), "{skipped}");
        }
        assert!(defaults.wants(Path::new("crates/app/src/main.rs")));
        let all = Matcher::new(&Options {
            use_excludes: false,
            ..options("x")
        })
        .unwrap();
        assert!(all.wants(Path::new(".venv/lib/x.py")) && all.wants(Path::new("dist/x.py")));
        assert_eq!(
            filter(
                vec!["a/node_modules/x.js".into(), "a/x.js".into()],
                &all,
                true
            ),
            vec![PathBuf::from("a/x.js")]
        );
    }

    /// `ZJ_SEARCH_BENCH=/path cargo test --release bench_text_search -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_text_search() {
        let Some(root) = std::env::var_os("ZJ_SEARCH_BENCH").map(PathBuf::from) else {
            return;
        };
        let started = std::time::Instant::now();
        let (files, _) = walk_all(&root, Vec::new(), MAX_WALK_FILES, &AtomicBool::new(false));
        let matcher = Matcher::new(&options("return x")).unwrap();
        let files = filter(files, &matcher, true);
        let listed = started.elapsed();
        let progress = Progress::default();
        run(
            &root,
            files.clone(),
            &matcher,
            &progress,
            &AtomicBool::new(false),
        );
        eprintln!(
            "files={} searched={} matches={} truncated={} list_ms={} total_ms={}",
            files.len(),
            progress.files_searched.load(Ordering::Relaxed),
            progress.matches.load(Ordering::Relaxed),
            progress.truncated.load(Ordering::Relaxed),
            listed.as_millis(),
            started.elapsed().as_millis()
        );
    }

    #[test]
    fn skips_pipes_instead_of_blocking() {
        let root = std::env::temp_dir().join(format!("zj-search-fifo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let fifo = std::ffi::CString::new(root.join("pipe").to_str().unwrap()).unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        // Tracked symlinks are listed as files; one may point at a pipe.
        std::os::unix::fs::symlink("pipe", root.join("link")).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let dir = root.clone();
        std::thread::spawn(move || {
            let matcher = Matcher::new(&options("needle")).unwrap();
            let progress = Progress::default();
            let files = vec!["pipe".into(), "link".into(), "a.txt".into()];
            run(&dir, files, &matcher, &progress, &AtomicBool::new(false));
            let _ = sender.send(progress.results.into_inner().unwrap().len());
        });
        let found = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("searching a pipe blocked");
        assert_eq!(found, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runs_over_files_and_stops_at_the_cap() {
        let root = std::env::temp_dir().join(format!("zj-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "fn needle() {}\nneedle();\n").unwrap();
        fs::write(root.join("src/b.rs"), "nothing\n").unwrap();
        fs::write(root.join("src/c.bin"), b"needle\0").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/needle"), "needle").unwrap();
        let (files, complete) =
            walk_all(&root, Vec::new(), MAX_WALK_FILES, &AtomicBool::new(false));
        assert_eq!(files.len(), 3);
        assert!(complete);
        let matcher = Matcher::new(&options("needle")).unwrap();
        let progress = Progress::default();
        run(&root, files, &matcher, &progress, &AtomicBool::new(false));
        let results = progress.results.into_inner().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].count, 2);
        assert_eq!(progress.files_searched.load(Ordering::Relaxed), 3);
        assert!(progress.done.load(Ordering::Relaxed));
        // The cap: many matches stop the search and mark it truncated.
        fs::write(
            root.join("src/many.txt"),
            "needle\n".repeat(MAX_MATCHES + 10),
        )
        .unwrap();
        let progress = Progress::default();
        run(
            &root,
            vec!["src/many.txt".into()],
            &matcher,
            &progress,
            &AtomicBool::new(false),
        );
        assert!(progress.truncated.load(Ordering::Relaxed));
        assert_eq!(progress.matches.load(Ordering::Relaxed), MAX_MATCHES);
        fs::remove_dir_all(root).unwrap();
    }
}
