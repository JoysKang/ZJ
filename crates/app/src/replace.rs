//! Find and replace semantics shared by the editor's find widget and the Search view, following
//! VS Code:
//! - Aa / ab / .* build the regex;
//! - matches never span lines unless the pattern contains `\n`;
//! - `^` / `$` also work before `\r\n`;
//! - in regex mode the replacement expands `$0` / `$&`, `$1`–`$99`, `${name}` and `$$`, and the
//!   escapes `\n`, `\t`, `\\` and `\u` `\l` `\U` `\L` `\E`;
//! - "保留大小写" (AB) carries the case of each match over to the replacement;
//! - a `\n` in the replacement becomes the document's own line ending.

use regex::{Captures, Regex, RegexBuilder};
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub pattern: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// The regex source for a query; shared with [`crate::text_search`].
pub fn pattern_source(pattern: &str, whole_word: bool, regex: bool) -> String {
    let pattern = if regex {
        pattern.to_string()
    } else {
        regex::escape(pattern)
    };
    if whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    }
}

/// A compiled query.
pub struct Finder {
    regex: Regex,
    /// Matches may contain a line break only when the pattern asks for one.
    multiline: bool,
}

impl Finder {
    pub fn new(query: &Query) -> Result<Self, String> {
        if query.pattern.is_empty() {
            return Err(String::new());
        }
        let regex = RegexBuilder::new(&pattern_source(
            &query.pattern,
            query.whole_word,
            query.regex,
        ))
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .crlf(true)
        .size_limit(16 * 1024 * 1024)
        .build()
        .map_err(|error| format!("正则表达式无效：{error}"))?;
        Ok(Self {
            regex,
            multiline: query.regex && query.pattern.contains("\\n"),
        })
    }

    fn accepts(&self, found: &regex::Match) -> bool {
        found.start() < found.end() && (self.multiline || !found.as_str().contains('\n'))
    }

    /// Byte ranges of all matches, in order, at most `limit`.
    pub fn find_all(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        self.regex
            .find_iter(text)
            .filter(|found| self.accepts(found))
            .take(limit)
            .map(|found| found.range())
            .collect()
    }

    /// Matches inside `within` only (find in selection).
    pub fn find_in(&self, text: &str, within: &Range<usize>, limit: usize) -> Vec<Range<usize>> {
        self.find_all(text, usize::MAX)
            .into_iter()
            .filter(|range| range.start >= within.start && range.end <= within.end)
            .take(limit)
            .collect()
    }

    /// The text that replaces the match at `range` (which must come from this finder).
    pub fn replacement_for(
        &self,
        text: &str,
        range: &Range<usize>,
        replace: &Replacement,
    ) -> Option<String> {
        let captures = self.regex.captures_at(text, range.start)?;
        let whole = captures.get(0)?;
        if whole.range() != *range {
            return None;
        }
        Some(replace.expand(&captures))
    }

    /// The edits that replace the matches whose ranges are in `only` (sorted by start; all
    /// matches when `None`): each match's range and its replacement text.
    pub fn edits(
        &self,
        text: &str,
        only: Option<&[Range<usize>]>,
        replace: &Replacement,
    ) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for captures in self.regex.captures_iter(text) {
            let Some(whole) = captures.get(0) else {
                continue;
            };
            if !self.accepts(&whole) {
                continue;
            }
            if let Some(only) = only {
                let wanted = only
                    .binary_search_by_key(&whole.start(), |range| range.start)
                    .is_ok_and(|i| only[i].end == whole.end());
                if !wanted {
                    continue;
                }
            }
            edits.push((whole.range(), replace.expand(&captures)));
        }
        edits
    }

    /// Replaces the matches whose ranges are in `only` (all of them when `None`). Returns
    /// the new text and how many were replaced.
    pub fn replace(
        &self,
        text: &str,
        only: Option<&[Range<usize>]>,
        replace: &Replacement,
    ) -> (String, usize) {
        let edits = self.edits(text, only, replace);
        (apply(text, &edits), edits.len())
    }

    /// Like [`Self::edits`] for the matches an earlier search found at `spans`, but `None`
    /// when the text no longer has a match at every one of them (the file changed).
    pub fn edits_at(
        &self,
        text: &str,
        spans: &[Range<usize>],
        replace: &Replacement,
    ) -> Option<Vec<(Range<usize>, String)>> {
        let mut spans = spans.to_vec();
        spans.sort_by_key(|range| range.start);
        spans.dedup();
        let edits = self.edits(text, Some(&spans), replace);
        (edits.len() == spans.len()).then_some(edits)
    }
}

/// The text with `edits` (sorted, non-overlapping) applied.
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (range, replacement) in edits {
        out.push_str(&text[last..range.start]);
        out.push_str(replacement);
        last = range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// First line, last line and the edits of a run of changed lines.
type Block<'a> = (usize, usize, Vec<&'a (Range<usize>, String)>);

/// A full-context unified patch from `old` to `old` with `edits` applied, for the diff
/// editor: every line is context except the lines the edits touch.
pub fn preview_patch(old: &str, edits: &[(Range<usize>, String)]) -> String {
    // Start offset of every line, plus the end of the text.
    let mut starts: Vec<usize> = std::iter::once(0)
        .chain(old.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    if starts.last() != Some(&old.len()) {
        starts.push(old.len());
    }
    let line_of = |offset: usize| {
        starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    };
    // Blocks of whole lines that change, each with its edits.
    let mut blocks: Vec<Block> = Vec::new();
    for edit in edits {
        let first = line_of(edit.0.start);
        let last = line_of(edit.0.end.saturating_sub(1).max(edit.0.start));
        match blocks.last_mut() {
            Some(block) if first <= block.1 => {
                block.1 = block.1.max(last);
                block.2.push(edit);
            }
            _ => blocks.push((first, last, vec![edit])),
        }
    }
    let lines = starts.len() - 1;
    let line = |i: usize| -> &str { &old[starts[i]..starts[i + 1]] };
    let mut body = String::new();
    let (mut old_count, mut new_count) = (0usize, 0usize);
    let push = |body: &mut String, prefix: char, text: &str| {
        body.push(prefix);
        body.push_str(text);
        if !text.ends_with('\n') {
            body.push_str("\n\\ No newline at end of file\n");
        }
    };
    let mut next = 0;
    for (first, last, block_edits) in blocks {
        for i in next..first {
            push(&mut body, ' ', line(i));
            old_count += 1;
            new_count += 1;
        }
        let (start, end) = (starts[first], starts[last + 1]);
        for i in first..=last {
            push(&mut body, '-', line(i));
            old_count += 1;
        }
        let shifted: Vec<(Range<usize>, String)> = block_edits
            .iter()
            .map(|(range, text)| (range.start - start..range.end - start, text.clone()))
            .collect();
        let replaced = apply(&old[start..end], &shifted);
        for new_line in replaced.split_inclusive('\n') {
            push(&mut body, '+', new_line);
            new_count += 1;
        }
        next = last + 1;
    }
    for i in next..lines {
        push(&mut body, ' ', line(i));
        old_count += 1;
        new_count += 1;
    }
    format!("@@ -1,{old_count} +1,{new_count} @@\n{body}")
}

/// What a match is replaced with.
#[derive(Clone, Debug)]
pub struct Replacement {
    pub template: String,
    /// `$1` and escapes are interpreted (the query is a regex).
    pub regex: bool,
    pub preserve_case: bool,
    /// The document's line ending, inserted for `\n`.
    pub eol: &'static str,
}

impl Replacement {
    pub fn new(template: impl Into<String>, regex: bool, preserve_case: bool) -> Self {
        Self {
            template: template.into(),
            regex,
            preserve_case,
            eol: "\n",
        }
    }

    pub fn with_eol(mut self, eol: &'static str) -> Self {
        self.eol = eol;
        self
    }

    fn expand(&self, captures: &Captures) -> String {
        let matched = captures.get(0).map_or("", |m| m.as_str());
        let text = if self.regex {
            expand_template(&self.template, captures, self.eol)
        } else {
            self.template.clone()
        };
        if self.preserve_case {
            preserve_case(matched, &text)
        } else {
            text
        }
    }
}

/// The line ending a file uses (its first one).
pub fn eol_of(text: &str) -> &'static str {
    match text.find('\n') {
        Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

#[derive(Clone, Copy, PartialEq)]
enum CaseOp {
    None,
    Upper,
    Lower,
}

/// VS Code's replace pattern: `$` references and backslash escapes with case operators.
fn expand_template(template: &str, captures: &Captures, eol: &str) -> String {
    let mut out = String::new();
    let mut span = CaseOp::None;
    let mut next = CaseOp::None;
    let push = |out: &mut String, piece: &str, span: CaseOp, next: &mut CaseOp| {
        for c in piece.chars() {
            let op = if *next != CaseOp::None {
                std::mem::replace(next, CaseOp::None)
            } else {
                span
            };
            match op {
                CaseOp::Upper => out.extend(c.to_uppercase()),
                CaseOp::Lower => out.extend(c.to_lowercase()),
                CaseOp::None => out.push(c),
            }
        }
    };
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            i += 2;
            match chars[i - 1] {
                'n' => push(&mut out, eol, span, &mut next),
                't' => push(&mut out, "\t", span, &mut next),
                '\\' => push(&mut out, "\\", span, &mut next),
                'u' => next = CaseOp::Upper,
                'l' => next = CaseOp::Lower,
                'U' => span = CaseOp::Upper,
                'L' => span = CaseOp::Lower,
                'E' => span = CaseOp::None,
                other => {
                    let piece: String = ['\\', other].iter().collect();
                    push(&mut out, &piece, span, &mut next);
                }
            }
            continue;
        }
        if c == '$' && i + 1 < chars.len() {
            let d = chars[i + 1];
            if d == '$' {
                push(&mut out, "$", span, &mut next);
                i += 2;
                continue;
            }
            if d == '&' {
                push(&mut out, &captures[0], span, &mut next);
                i += 2;
                continue;
            }
            if d == '{' {
                if let Some(end) = chars[i + 2..].iter().position(|c| *c == '}') {
                    let name: String = chars[i + 2..i + 2 + end].iter().collect();
                    let group = match name.parse::<usize>() {
                        Ok(n) => captures.get(n),
                        Err(_) => captures.name(&name),
                    };
                    if let Some(group) = group {
                        push(&mut out, group.as_str(), span, &mut next);
                        i += 3 + end;
                        continue;
                    }
                }
            } else if let Some(first) = d.to_digit(10) {
                // Two digits when that group exists ($12), otherwise one ($1 then "2").
                let two = chars
                    .get(i + 2)
                    .and_then(|c| c.to_digit(10))
                    .map(|second| (first * 10 + second) as usize)
                    .filter(|n| *n < captures.len());
                let (group, used) = match two {
                    Some(n) => (n, 3),
                    None => (first as usize, 2),
                };
                if group < captures.len() {
                    let value = captures.get(group).map_or("", |m| m.as_str());
                    push(&mut out, value, span, &mut next);
                    i += used;
                    continue;
                }
            }
        }
        let mut buffer = [0; 4];
        push(&mut out, c.encode_utf8(&mut buffer), span, &mut next);
        i += 1;
    }
    out
}

/// VS Code's `buildReplaceStringWithCasePreserved`.
pub fn preserve_case(matched: &str, replacement: &str) -> String {
    if matched.is_empty() {
        return replacement.to_string();
    }
    for (separator, other) in [('-', '_'), ('_', '-')] {
        if splits_alike(matched, replacement, separator)
            && !splits_alike(matched, replacement, other)
        {
            return matched
                .split(separator)
                .zip(replacement.split(separator))
                .map(|(m, r)| preserve_case(m, r))
                .collect::<Vec<_>>()
                .join(&separator.to_string());
        }
    }
    if matched.to_uppercase() == matched {
        return replacement.to_uppercase();
    }
    if matched.to_lowercase() == matched {
        return replacement.to_lowercase();
    }
    let mut chars = replacement.chars();
    let (Some(first_matched), Some(first)) = (matched.chars().next(), chars.next()) else {
        return replacement.to_string();
    };
    if first_matched.is_uppercase() {
        first.to_uppercase().chain(chars).collect()
    } else if first_matched.is_lowercase() {
        first.to_lowercase().chain(chars).collect()
    } else {
        replacement.to_string()
    }
}

fn splits_alike(matched: &str, replacement: &str, separator: char) -> bool {
    matched.contains(separator)
        && replacement.contains(separator)
        && matched.split(separator).count() == replacement.split(separator).count()
}

/// The differing part of two texts: `old[start..end]` became `new[start..new_end]`, on
/// character boundaries.
pub fn changed_span(old: &str, new: &str) -> (usize, usize, usize) {
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    (prefix, old.len() - suffix, new.len() - suffix)
}

/// Byte offset → UTF-16 offset, for the editor's input-handler API.
pub fn utf16_offset(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finder(pattern: &str, case: bool, word: bool, regex: bool) -> Finder {
        Finder::new(&Query {
            pattern: pattern.into(),
            case_sensitive: case,
            whole_word: word,
            regex,
        })
        .unwrap()
    }

    #[test]
    fn toggles() {
        let text = "Foo foo food FOO";
        assert_eq!(
            finder("foo", false, false, false).find_all(text, 99).len(),
            4
        );
        assert_eq!(
            finder("foo", true, false, false).find_all(text, 99).len(),
            2
        );
        assert_eq!(
            finder("foo", false, true, false).find_all(text, 99).len(),
            3
        );
        assert_eq!(
            finder("fo+d?", true, false, true).find_all(text, 99).len(),
            2
        );
        // Literal mode escapes regex syntax.
        assert_eq!(
            finder("a.b", true, false, false).find_all("a.b axb", 99),
            vec![0..3]
        );
        assert!(
            Finder::new(&Query {
                pattern: "(".into(),
                regex: true,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn regex_groups() {
        let f = finder(r"(\w+)@(\w+)", true, false, true);
        let r = Replacement::new("$2 at $1 ($0) $$ $&", true, false);
        let (out, n) = f.replace("me@host, you@there", None, &r);
        assert_eq!(n, 2);
        assert_eq!(
            out,
            "host at me (me@host) $ me@host, there at you (you@there) $ you@there"
        );
        // `$1a` is group 1 then "a"; `$12` falls back to `$1` + "2" when there is no group 12.
        let r = Replacement::new("$1a $12 ${2}", true, false);
        assert_eq!(f.replace("x@y", None, &r).0, "xa x2 y");
        // Named groups and case operators.
        let f = finder(r"(?P<word>[a-z]+)", true, false, true);
        let r = Replacement::new(r"\u${word}-\U$1\E-\l$1", true, false);
        assert_eq!(f.replace("hello", None, &r).0, "Hello-HELLO-hello");
        // Without regex mode the template is literal.
        let f = finder("x", true, false, false);
        assert_eq!(
            f.replace("x", None, &Replacement::new("$0\\n", false, false))
                .0,
            "$0\\n"
        );
    }

    #[test]
    fn preserve_case_like_vs_code() {
        assert_eq!(preserve_case("FOO", "bar"), "BAR");
        assert_eq!(preserve_case("foo", "BaR"), "bar");
        assert_eq!(preserve_case("Foo", "bar"), "Bar");
        assert_eq!(preserve_case("fOO", "Bar"), "bar");
        assert_eq!(preserve_case("Foo-Bar", "baz-qux"), "Baz-Qux");
        assert_eq!(preserve_case("FOO_bar", "baz_qux"), "BAZ_qux");
        assert_eq!(preserve_case("中文", "bar"), "BAR");
        let f = finder("foo", false, false, false);
        let r = Replacement::new("bar", false, true);
        assert_eq!(f.replace("foo Foo FOO", None, &r).0, "bar Bar BAR");
    }

    #[test]
    fn crlf_documents() {
        let text = "one\r\ntwo\r\n";
        assert_eq!(eol_of(text), "\r\n");
        assert_eq!(eol_of("a\nb"), "\n");
        // `$` matches before \r\n, and the match does not include the \r.
        let f = finder("o$", true, false, true);
        assert_eq!(f.find_all(text, 99), vec![7..8]);
        // A `\n` in the replacement becomes the file's line ending.
        let f = finder("one", true, false, true);
        let r = Replacement::new(r"1\n2", true, false).with_eol(eol_of(text));
        assert_eq!(f.replace(text, None, &r).0, "1\r\n2\r\ntwo\r\n");
        // Matches never span lines unless the pattern asks for it.
        let f = finder(r"one\s+two", true, false, true);
        assert!(f.find_all(text, 99).is_empty());
        let f = finder(r"one\r\ntwo", true, false, true);
        assert_eq!(f.find_all(text, 99), vec![0..8]);
    }

    #[test]
    fn stale_spans_and_preview_patch() {
        let f = finder("foo", true, false, false);
        let text = "a foo\nb\nfoo foo\n";
        let spans = f.find_all(text, 99);
        let r = Replacement::new("bar", false, false);
        // Ignore the middle match: only the others are replaced.
        let edits = f
            .edits_at(text, &[spans[0].clone(), spans[2].clone()], &r)
            .unwrap();
        assert_eq!(apply(text, &edits), "a bar\nb\nfoo bar\n");
        // A span that is no longer a match means the file changed since the search.
        assert!(f.edits_at("a fo\nb\nfoo foo\n", &spans, &r).is_none());
        let patch = preview_patch(text, &edits);
        assert_eq!(
            patch,
            "@@ -1,3 +1,3 @@\n-a foo\n+a bar\n b\n-foo foo\n+foo bar\n"
        );
        // A replacement with a line break, and a last line without one.
        let f = finder("x", true, false, true);
        let edits = f.edits("1\nx", None, &Replacement::new(r"y\nz", true, false));
        assert_eq!(
            preview_patch("1\nx", &edits),
            "@@ -1,2 +1,3 @@\n 1\n-x\n\\ No newline at end of file\n+y\n+z\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn changed_spans() {
        assert_eq!(changed_span("abc", "abc"), (3, 3, 3));
        assert_eq!(changed_span("a foo b", "a bar b"), (2, 5, 5));
        assert_eq!(changed_span("中文", "中国文"), (3, 3, 6));
        assert_eq!(changed_span("aaa", "aa"), (2, 3, 2));
    }

    #[test]
    fn multi_byte_text() {
        let text = "优先；至少 优先 😀优先";
        let f = finder("优先", true, false, false);
        let found = f.find_all(text, 99);
        assert_eq!(found.len(), 3);
        for range in &found {
            assert_eq!(&text[range.clone()], "优先");
        }
        let (out, n) = f.replace(
            text,
            Some(&found[1..2]),
            &Replacement::new("首要", false, false),
        );
        assert_eq!((out.as_str(), n), ("优先；至少 首要 😀优先", 1));
        assert_eq!(utf16_offset(text, found[2].start), 11);
        assert_eq!(
            f.replacement_for(text, &found[0], &Replacement::new("x", false, false)),
            Some("x".into())
        );
        assert_eq!(
            f.replacement_for(text, &(1..4), &Replacement::new("x", false, false)),
            None
        );
        // Find in selection.
        assert_eq!(f.find_in(text, &(0..22), 99), found[..2].to_vec());
    }
}
