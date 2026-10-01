//! Full-file diff document for the VS Code-style diff editor, built once off the UI thread.
//!
//! Input is a unified patch produced with full context (`git diff -U<large>`), so the old and
//! new files can be rebuilt line by line. The document holds both texts once, their syntax
//! spans, char-level change ranges for modified line pairs, and the row layout for the
//! side-by-side and inline modes.

use gpui_kit::{
    HighlightStyle,
    component::highlighter::{HighlightTheme, SyntaxHighlighter},
};
use std::ops::Range;

/// Lines longer than this are shown without syntax or char-level highlighting.
const MAX_STYLED_LINE: usize = 4_096;
/// Token-level LCS budget (old tokens × new tokens) per modified line pair.
const MAX_LCS_CELLS: usize = 40_000;
const MAX_LINES: usize = 200_000;
/// Line-alignment budget (removed × added lines) per change block.
const MAX_ALIGN_CELLS: usize = 20_000;
/// Minimum token similarity for a removed and an added line to count as one edited line.
const SIMILAR: f32 = 0.5;
const TAB: &str = "    ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Same,
    Removed,
    Added,
}

/// One line of one side: its text range and the final, non-overlapping style runs.
#[derive(Debug, Default)]
pub struct Line {
    pub number: u32,
    pub text: Range<usize>,
    /// Ranges relative to the line start; `background_color` marks char-level changes.
    pub runs: Vec<(Range<usize>, HighlightStyle)>,
}

#[derive(Debug, Default)]
pub struct Side {
    pub text: String,
    pub lines: Vec<Line>,
}

impl Side {
    pub fn line_text(&self, index: usize) -> &str {
        &self.text[self.lines[index].text.clone()]
    }
}

/// A side-by-side row: indexes into `old.lines` / `new.lines`; `None` is a filler cell.
#[derive(Clone, Copy, Debug)]
pub struct PairRow {
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub kind: RowKind,
    /// Both sides are versions of the same line, so char-level changes are worth showing.
    pub similar: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Changed,
}

/// An inline row shows exactly one line of one side.
#[derive(Clone, Copy, Debug)]
pub struct InlineRow {
    pub kind: LineKind,
    /// Index into `old.lines` for Same/Removed, `new.lines` for Added.
    pub line: u32,
    /// For Same rows, the matching new line (for the second gutter number).
    pub other: Option<u32>,
}

#[derive(Debug, Default)]
pub struct DiffDoc {
    pub old: Side,
    pub new: Side,
    pub rows: Vec<PairRow>,
    pub inline: Vec<InlineRow>,
    /// First row of every change block, for previous / next change navigation.
    pub changes: Vec<usize>,
    pub inline_changes: Vec<usize>,
    /// Widest line in characters, for the horizontal scroll width.
    pub columns: usize,
    pub added: usize,
    pub removed: usize,
}

/// Colors for the changed-text overlay (VS Code `diffEditor.*TextBackground`).
#[derive(Clone, Copy)]
pub struct ChangeColors {
    pub inserted_text: gpui_kit::Hsla,
    pub removed_text: gpui_kit::Hsla,
}

fn expand_tabs(line: &str, out: &mut String) {
    if line.contains('\t') {
        out.push_str(&line.replace('\t', TAB));
    } else {
        out.push_str(line);
    }
}

#[derive(Default)]
struct Builder {
    old: Side,
    new: Side,
}

impl Builder {
    fn push(side: &mut Side, number: u32, text: &str) -> u32 {
        let start = side.text.len();
        expand_tabs(text, &mut side.text);
        let end = side.text.len();
        side.text.push('\n');
        side.lines.push(Line {
            number,
            text: start..end,
            runs: Vec::new(),
        });
        (side.lines.len() - 1) as u32
    }
}

impl DiffDoc {
    /// Returns `None` for patches that are not a plain two-sided text diff (binary, combined),
    /// so the caller can fall back to showing Git's text.
    pub fn parse(
        patch: &str,
        language: Option<&str>,
        theme: &HighlightTheme,
        colors: ChangeColors,
    ) -> Option<Self> {
        let mut builder = Builder::default();
        let mut rows = Vec::new();
        let mut removed: Vec<u32> = Vec::new();
        let mut added: Vec<u32> = Vec::new();
        let (mut old_no, mut new_no) = (0u32, 0u32);
        let mut in_hunk = false;
        let mut seen_hunk = false;
        for raw in patch.split_inclusive('\n') {
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.starts_with("@@@")
                || line.starts_with("Binary files ")
                || line.starts_with("GIT binary patch")
            {
                return None;
            }
            if line.starts_with("@@ ") {
                flush(&builder, &mut rows, &mut removed, &mut added);
                let mut parts = line.split_whitespace().skip(1);
                let position = |s: Option<&str>, prefix: char| -> Option<u32> {
                    s?.strip_prefix(prefix)?.split(',').next()?.parse().ok()
                };
                old_no = position(parts.next(), '-')?;
                new_no = position(parts.next(), '+')?;
                in_hunk = true;
                seen_hunk = true;
                continue;
            }
            if !in_hunk {
                continue;
            }
            if builder.old.lines.len() + builder.new.lines.len() > MAX_LINES {
                return None;
            }
            match line.as_bytes().first() {
                Some(b'-') => {
                    removed.push(Builder::push(&mut builder.old, old_no, &line[1..]));
                    old_no += 1;
                }
                Some(b'+') => {
                    added.push(Builder::push(&mut builder.new, new_no, &line[1..]));
                    new_no += 1;
                }
                Some(b' ') => {
                    flush(&builder, &mut rows, &mut removed, &mut added);
                    let old = Builder::push(&mut builder.old, old_no, &line[1..]);
                    let new = Builder::push(&mut builder.new, new_no, &line[1..]);
                    rows.push(PairRow {
                        old: Some(old),
                        new: Some(new),
                        kind: RowKind::Same,
                        similar: true,
                    });
                    old_no += 1;
                    new_no += 1;
                }
                // An empty context line can lose its leading space in some patch tools.
                None => {
                    flush(&builder, &mut rows, &mut removed, &mut added);
                    let old = Builder::push(&mut builder.old, old_no, "");
                    let new = Builder::push(&mut builder.new, new_no, "");
                    rows.push(PairRow {
                        old: Some(old),
                        new: Some(new),
                        kind: RowKind::Same,
                        similar: true,
                    });
                    old_no += 1;
                    new_no += 1;
                }
                Some(b'\\') => {} // "\ No newline at end of file"
                _ => in_hunk = false,
            }
        }
        flush(&builder, &mut rows, &mut removed, &mut added);
        if !seen_hunk || rows.is_empty() {
            return None;
        }
        let mut doc = DiffDoc {
            old: builder.old,
            new: builder.new,
            rows,
            ..Default::default()
        };
        doc.layout();
        doc.style(language, theme, colors);
        Some(doc)
    }

    fn layout(&mut self) {
        let mut previous_changed = false;
        let mut pending_added: Vec<u32> = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let changed = row.kind == RowKind::Changed;
            if changed && !previous_changed {
                self.changes.push(index);
                // The block's first inline row is the next one pushed (removed or added).
                self.inline_changes.push(self.inline.len());
            }
            previous_changed = changed;
            match row.kind {
                RowKind::Same => {
                    for line in pending_added.drain(..) {
                        self.inline.push(InlineRow {
                            kind: LineKind::Added,
                            line,
                            other: None,
                        });
                    }
                    self.inline.push(InlineRow {
                        kind: LineKind::Same,
                        line: row.old.unwrap_or_default(),
                        other: row.new,
                    });
                }
                RowKind::Changed => {
                    // VS Code's inline view lists a block's removed lines, then its added lines.
                    if let Some(old) = row.old {
                        self.inline.push(InlineRow {
                            kind: LineKind::Removed,
                            line: old,
                            other: None,
                        });
                        self.removed += 1;
                    }
                    if let Some(new) = row.new {
                        pending_added.push(new);
                        self.added += 1;
                    }
                }
            }
        }
        for line in pending_added {
            self.inline.push(InlineRow {
                kind: LineKind::Added,
                line,
                other: None,
            });
        }
        self.columns = self
            .old
            .lines
            .iter()
            .map(|line| &self.old.text[line.text.clone()])
            .chain(
                self.new
                    .lines
                    .iter()
                    .map(|l| &self.new.text[l.text.clone()]),
            )
            .map(|text| text.chars().count())
            .max()
            .unwrap_or(0);
    }

    fn style(&mut self, language: Option<&str>, theme: &HighlightTheme, colors: ChangeColors) {
        let old_syntax = language.map(|lang| highlight(lang, &self.old.text, theme));
        let new_syntax = language.map(|lang| highlight(lang, &self.new.text, theme));
        let mut old_changes: Vec<Vec<Range<usize>>> = vec![Vec::new(); self.old.lines.len()];
        let mut new_changes: Vec<Vec<Range<usize>>> = vec![Vec::new(); self.new.lines.len()];
        for row in &self.rows {
            if let (RowKind::Changed, true, Some(old), Some(new)) =
                (row.kind, row.similar, row.old, row.new)
            {
                let (a, b) = (
                    self.old.line_text(old as usize),
                    self.new.line_text(new as usize),
                );
                if a.len() <= MAX_STYLED_LINE && b.len() <= MAX_STYLED_LINE {
                    let (left, right) = char_changes(a, b);
                    old_changes[old as usize] = left;
                    new_changes[new as usize] = right;
                }
            }
        }
        assign_runs(&mut self.old, old_syntax, &old_changes, colors.removed_text);
        assign_runs(
            &mut self.new,
            new_syntax,
            &new_changes,
            colors.inserted_text,
        );
    }
}

/// Emits one change block as rows. Removed and added lines that are versions of the same
/// line share a row (as VS Code aligns them); the rest are zipped between those anchors.
fn flush(builder: &Builder, rows: &mut Vec<PairRow>, removed: &mut Vec<u32>, added: &mut Vec<u32>) {
    let anchors = align(
        &removed
            .iter()
            .map(|&i| builder.old.line_text(i as usize))
            .collect::<Vec<_>>(),
        &added
            .iter()
            .map(|&i| builder.new.line_text(i as usize))
            .collect::<Vec<_>>(),
    );
    let (mut i, mut j) = (0, 0);
    let zip = |rows: &mut Vec<PairRow>, end: (usize, usize), i: &mut usize, j: &mut usize| {
        while *i < end.0 || *j < end.1 {
            let old = (*i < end.0).then(|| removed[*i]);
            let new = (*j < end.1).then(|| added[*j]);
            *i += usize::from(old.is_some());
            *j += usize::from(new.is_some());
            // Lone lines zipped side by side are not edits of each other.
            rows.push(PairRow {
                old,
                new,
                kind: RowKind::Changed,
                similar: false,
            });
        }
    };
    for (a, b) in anchors {
        zip(rows, (a, b), &mut i, &mut j);
        rows.push(PairRow {
            old: Some(removed[a]),
            new: Some(added[b]),
            kind: RowKind::Changed,
            similar: true,
        });
        i = a + 1;
        j = b + 1;
    }
    zip(rows, (removed.len(), added.len()), &mut i, &mut j);
    removed.clear();
    added.clear();
}

/// Monotone pairs (removed index, added index) maximizing total similarity.
fn align(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    if n * m > MAX_ALIGN_CELLS {
        return (0..n.min(m)).map(|k| (k, k)).collect();
    }
    let bags: Vec<Vec<&str>> = old.iter().map(|line| bag(line)).collect();
    let new_bags: Vec<Vec<&str>> = new.iter().map(|line| bag(line)).collect();
    let mut score = vec![0f32; n * m];
    for i in 0..n {
        for j in 0..m {
            score[i * m + j] = similarity(&bags[i], &new_bags[j]);
        }
    }
    let at = |i: usize, j: usize| i * (m + 1) + j;
    let mut dp = vec![0f32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = dp[at(i + 1, j)].max(dp[at(i, j + 1)]);
            let s = score[i * m + j];
            if s >= SIMILAR {
                best = best.max(dp[at(i + 1, j + 1)] + s);
            }
            dp[at(i, j)] = best;
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        let s = score[i * m + j];
        if s >= SIMILAR && dp[at(i, j)] == dp[at(i + 1, j + 1)] + s {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Sorted non-whitespace tokens of a line.
fn bag(line: &str) -> Vec<&str> {
    let mut tokens: Vec<&str> = tokens(line)
        .into_iter()
        .map(|range| &line[range])
        .filter(|token| !token.trim().is_empty())
        .collect();
    tokens.sort_unstable();
    tokens
}

/// Dice coefficient over token bags, weighted by token length.
fn similarity(a: &[&str], b: &[&str]) -> f32 {
    let total: usize = a.iter().chain(b).map(|t| t.len()).sum();
    if total == 0 {
        return 1.0;
    }
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                common += a[i].len();
                i += 1;
                j += 1;
            }
        }
    }
    (2 * common) as f32 / total as f32
}

fn highlight(
    language: &str,
    text: &str,
    theme: &HighlightTheme,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let rope = gpui_kit::component::Rope::from(text);
    let mut highlighter = SyntaxHighlighter::new(language);
    if highlighter.language().as_ref() != language {
        return Vec::new();
    }
    highlighter.update(None, &rope, None);
    highlighter
        .styles(&(0..text.len()), theme)
        .into_iter()
        .filter(|(_, style)| *style != HighlightStyle::default())
        .collect()
}

/// Splits whole-text syntax spans into per-line runs and overlays change backgrounds.
fn assign_runs(
    side: &mut Side,
    syntax: Option<Vec<(Range<usize>, HighlightStyle)>>,
    changes: &[Vec<Range<usize>>],
    background: gpui_kit::Hsla,
) {
    let syntax = syntax.unwrap_or_default();
    let mut cursor = 0;
    for (index, line) in side.lines.iter_mut().enumerate() {
        let len = line.text.len();
        if len == 0 || len > MAX_STYLED_LINE {
            continue;
        }
        while cursor < syntax.len() && syntax[cursor].0.end <= line.text.start {
            cursor += 1;
        }
        let mut spans: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
        let mut i = cursor;
        while i < syntax.len() && syntax[i].0.start < line.text.end {
            let (range, style) = &syntax[i];
            let start = range.start.max(line.text.start) - line.text.start;
            let end = range.end.min(line.text.end) - line.text.start;
            if start < end {
                spans.push((start..end, *style));
            }
            i += 1;
        }
        line.runs = overlay(&spans, &changes[index], len, background);
    }
}

/// Merges sorted, non-overlapping syntax spans with change ranges into style runs.
fn overlay(
    syntax: &[(Range<usize>, HighlightStyle)],
    changes: &[Range<usize>],
    len: usize,
    background: gpui_kit::Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    if changes.is_empty() {
        return syntax.to_vec();
    }
    let mut cuts = vec![0, len];
    for (range, _) in syntax {
        cuts.extend([range.start, range.end]);
    }
    for range in changes {
        cuts.extend([range.start.min(len), range.end.min(len)]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let mut style = syntax
            .iter()
            .find(|(range, _)| range.start <= start && end <= range.end)
            .map(|(_, style)| *style)
            .unwrap_or_default();
        if changes
            .iter()
            .any(|range| range.start <= start && end <= range.end)
        {
            style.background_color = Some(background);
        }
        if style != HighlightStyle::default() {
            runs.push((start..end, style));
        }
    }
    runs
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else if c.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Words, whitespace runs and single punctuation characters, as byte ranges.
fn tokens(text: &str) -> Vec<Range<usize>> {
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut previous: Option<Class> = None;
    for (index, c) in text.char_indices() {
        let current = class(c);
        let joins = previous == Some(current) && current != Class::Other;
        match tokens.last_mut() {
            Some(last) if joins => last.end = index + c.len_utf8(),
            _ => tokens.push(index..index + c.len_utf8()),
        }
        previous = Some(current);
    }
    tokens
}

/// Changed byte ranges on each side of a modified line pair (token-level LCS).
pub fn char_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let a = tokens(old);
    let b = tokens(new);
    let same = |i: usize, j: usize| old[a[i].clone()] == new[b[j].clone()];
    // Trim the common prefix and suffix; most edits touch one region.
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && same(prefix, prefix) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && same(a.len() - 1 - suffix, b.len() - 1 - suffix)
    {
        suffix += 1;
    }
    let (a_mid, b_mid) = (prefix..a.len() - suffix, prefix..b.len() - suffix);
    let mut keep_a = vec![false; a.len()];
    let mut keep_b = vec![false; b.len()];
    for i in 0..prefix {
        keep_a[i] = true;
        keep_b[i] = true;
    }
    for k in 0..suffix {
        keep_a[a.len() - 1 - k] = true;
        keep_b[b.len() - 1 - k] = true;
    }
    let (n, m) = (a_mid.len(), b_mid.len());
    if n > 0 && m > 0 && n * m <= MAX_LCS_CELLS {
        // dp[i][j] = LCS length of a_mid[i..] and b_mid[j..].
        let mut dp = vec![0u16; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[at(i, j)] = if same(a_mid.start + i, b_mid.start + j) {
                    dp[at(i + 1, j + 1)] + 1
                } else {
                    dp[at(i + 1, j)].max(dp[at(i, j + 1)])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if same(a_mid.start + i, b_mid.start + j) {
                keep_a[a_mid.start + i] = true;
                keep_b[b_mid.start + j] = true;
                i += 1;
                j += 1;
            } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    (
        join_spaced(old, ranges(&a, &keep_a)),
        join_spaced(new, ranges(&b, &keep_b)),
    )
}

/// Joins changed ranges separated only by whitespace, so `let total: usize` reads as one edit.
fn join_spaced(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match out.last_mut() {
            Some(last) if text[last.end..range.start].trim().is_empty() => last.end = range.end,
            _ => out.push(range),
        }
    }
    out
}

fn ranges(tokens: &[Range<usize>], keep: &[bool]) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (token, kept) in tokens.iter().zip(keep) {
        if *kept {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end == token.start => last.end = token.end,
            _ => out.push(token.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> ChangeColors {
        ChangeColors {
            inserted_text: gpui_kit::green(),
            removed_text: gpui_kit::red(),
        }
    }

    const PATCH: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,4 +1,5 @@\n fn main() {\n-    let x = 1;\n+    let x = 2;\n+    let y = x;\n }\n\tdone\n\\ No newline at end of file\n";

    #[test]
    fn rebuilds_both_files_and_aligns_rows() {
        let theme = HighlightTheme::default_dark();
        let doc = DiffDoc::parse(PATCH, Some("rust"), &theme, colors()).unwrap();
        assert_eq!(doc.old.lines.len(), 3);
        assert_eq!(doc.new.lines.len(), 4);
        assert_eq!(doc.rows.len(), 4);
        // Modified pair, then an added line with a filler on the left.
        assert!(doc.rows[1].old.is_some() && doc.rows[1].new.is_some());
        assert!(doc.rows[2].old.is_none() && doc.rows[2].new.is_some());
        assert_eq!(doc.changes, vec![1]);
        assert_eq!((doc.added, doc.removed), (2, 1));
        assert_eq!(doc.new.line_text(1), "    let x = 2;");
        // Inline view: removed line first, then the two added lines.
        let kinds: Vec<_> = doc.inline.iter().map(|row| row.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Same,
                LineKind::Removed,
                LineKind::Added,
                LineKind::Added,
                LineKind::Same
            ]
        );
        // Syntax highlighting reached the rebuilt text.
        assert!(!doc.new.lines[0].runs.is_empty());
        // The changed literal carries a change background.
        let changed: Vec<_> = doc.new.lines[1]
            .runs
            .iter()
            .filter(|(_, style)| style.background_color.is_some())
            .map(|(range, _)| &doc.new.line_text(1)[range.clone()])
            .collect();
        assert_eq!(changed, ["2"]);
    }

    #[test]
    fn char_changes_find_the_edited_tokens() {
        let (a, b) = char_changes(
            "counts.entry(word).or_insert(0)",
            "counts.entry(word.to_lowercase()).or_insert(0)",
        );
        assert!(a.is_empty());
        assert_eq!(
            b.iter()
                .map(|r| &"counts.entry(word.to_lowercase()).or_insert(0)"[r.clone()])
                .collect::<Vec<_>>(),
            [".to_lowercase()"]
        );
        let (a, b) = char_changes("中文 old", "中文 new");
        assert_eq!((a.len(), b.len()), (1, 1));
    }

    /// An inserted line before an edited one gets its own row, as VS Code aligns it.
    #[test]
    fn edited_lines_pair_by_similarity() {
        let patch = "@@ -1,2 +1,3 @@\n-println!(\"{} words\", n);\n+let total = n + 1;\n+println!(\"{} words, {} total\", n, total);\n }\n";
        let theme = HighlightTheme::default_dark();
        let doc = DiffDoc::parse(patch, None, &theme, colors()).unwrap();
        let shape: Vec<_> = doc
            .rows
            .iter()
            .map(|row| (row.old.is_some(), row.new.is_some(), row.similar))
            .collect();
        assert_eq!(
            shape,
            [(false, true, false), (true, true, true), (true, true, true)]
        );
        let changed: Vec<_> = doc.new.lines[1]
            .runs
            .iter()
            .filter(|(_, style)| style.background_color.is_some())
            .map(|(range, _)| &doc.new.line_text(1)[range.clone()])
            .collect();
        assert_eq!(changed, [", {} total", ", total"]);
    }

    #[test]
    fn binary_and_combined_patches_fall_back() {
        let theme = HighlightTheme::default_dark();
        assert!(DiffDoc::parse("Binary files a and b differ\n", None, &theme, colors()).is_none());
        assert!(DiffDoc::parse("@@@ -1 -1 +1 @@@\n", None, &theme, colors()).is_none());
    }
}
