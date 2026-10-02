//! Text helpers shared by indexing and search: CJK bigrams, query terms, snippets, titles.

use std::ops::Range;

/// Han, kana and Hangul: scripts written without spaces between words.
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF       // Hiragana, Katakana
        | 0x3400..=0x4DBF     // CJK Extension A
        | 0x4E00..=0x9FFF     // CJK Unified Ideographs
        | 0xAC00..=0xD7AF     // Hangul syllables
        | 0xF900..=0xFAFF     // CJK Compatibility Ideographs
        | 0x20000..=0x3134F   // CJK Extensions B–G
    )
}

/// Overlapping two-character tokens for every CJK run, separated by spaces, so a unicode61
/// table can answer two-character queries that the trigram table cannot. A lone CJK character
/// becomes its own token. Non-CJK text is dropped (the trigram table covers it).
pub(crate) fn cjk_bigrams(text: &str) -> String {
    let mut out = String::new();
    let mut run: Vec<char> = Vec::new();
    let flush = |run: &mut Vec<char>, out: &mut String| {
        match run.len() {
            0 => {}
            1 => {
                out.push(run[0]);
                out.push(' ');
            }
            _ => {
                for pair in run.windows(2) {
                    out.push(pair[0]);
                    out.push(pair[1]);
                    out.push(' ');
                }
            }
        }
        run.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out.truncate(out.trim_end().len());
    out
}

/// How one query term is looked up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TermPlan {
    /// Three or more characters: substring match in the trigram table.
    Trigram,
    /// Exactly two CJK characters: token match in the bigram table.
    Bigram,
    /// Anything shorter: `LIKE` over titles, file paths and session metadata.
    Like,
}

#[derive(Clone, Debug)]
pub(crate) struct Term {
    pub text: String,
    /// ASCII-lowercased copy used for highlighting; byte offsets match `text`.
    pub folded: String,
    pub plan: TermPlan,
}

/// Splits a query on whitespace; quotes are treated as ordinary separators.
pub(crate) fn parse_query(query: &str) -> Vec<Term> {
    let mut terms: Vec<Term> = Vec::new();
    for raw in query.split(|c: char| c.is_whitespace() || c == '"') {
        if raw.is_empty() {
            continue;
        }
        let chars = raw.chars().count();
        let plan = if chars >= 3 {
            TermPlan::Trigram
        } else if chars == 2 && raw.chars().all(is_cjk) {
            TermPlan::Bigram
        } else {
            TermPlan::Like
        };
        let folded = raw.to_ascii_lowercase();
        if terms.iter().any(|t| t.folded == folded) {
            continue;
        }
        terms.push(Term {
            text: raw.to_string(),
            folded,
            plan,
        });
        if terms.len() == 8 {
            break;
        }
    }
    terms
}

/// One FTS5 phrase: the term in double quotes with embedded quotes doubled.
pub(crate) fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

/// `LIKE` pattern with `\` as the escape character.
pub(crate) fn like_pattern(term: &str) -> String {
    let mut out = String::from("%");
    for c in term.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// A short excerpt of a matching document with the matched byte ranges (into `text`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snippet {
    pub text: String,
    pub highlights: Vec<Range<usize>>,
    /// Text was cut before / after the excerpt.
    pub leading_ellipsis: bool,
    pub trailing_ellipsis: bool,
}

const SNIPPET_BEFORE: usize = 24;
const SNIPPET_CHARS: usize = 96;

/// Byte ranges of every (ASCII-case-insensitive) occurrence of each term in `text`.
pub(crate) fn find_ranges(text: &str, terms: &[Term]) -> Vec<Range<usize>> {
    let folded = text.to_ascii_lowercase();
    let mut ranges = Vec::new();
    for term in terms {
        if term.folded.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(pos) = folded[from..].find(&term.folded) {
            let start = from + pos;
            let end = start + term.folded.len();
            ranges.push(start..end);
            from = end;
        }
    }
    ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    // Merge overlaps so the UI never paints a byte twice.
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => merged.push(r),
        }
    }
    merged
}

/// An excerpt around the first match, on one line, with highlight ranges relative to it.
pub(crate) fn snippet(text: &str, terms: &[Term]) -> Snippet {
    let ranges = find_ranges(text, terms);
    let first = ranges.first().map(|r| r.start).unwrap_or(0);
    // Character boundaries around the first match.
    let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    let first_char = starts.partition_point(|&i| i < first);
    let start_char = first_char.saturating_sub(SNIPPET_BEFORE);
    let end_char = (start_char + SNIPPET_CHARS).min(starts.len());
    let start = starts.get(start_char).copied().unwrap_or(text.len());
    let end = starts.get(end_char).copied().unwrap_or(text.len());
    let excerpt: String = text[start..end]
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let highlights = ranges
        .into_iter()
        .filter(|r| r.start >= start && r.end <= end)
        .map(|r| (r.start - start)..(r.end - start))
        .collect();
    Snippet {
        text: excerpt,
        highlights,
        leading_ellipsis: start > 0,
        trailing_ellipsis: end < text.len(),
    }
}

/// Placeholder title from the first prompt: `@` references removed, whitespace collapsed,
/// at most 24 characters.
pub fn placeholder_title(prompt: &str) -> String {
    let mut words = Vec::new();
    for word in prompt.split_whitespace() {
        if word.starts_with('@') && word.len() > 1 {
            continue;
        }
        words.push(word);
    }
    let joined = words.join(" ");
    let mut title: String = joined.chars().take(24).collect();
    if joined.chars().count() > 24 {
        title.push('…');
    }
    if title.is_empty() {
        title = "新会话".to_string();
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bigrams_cover_cjk_runs_only() {
        assert_eq!(cjk_bigrams("修复重连 bug"), "修复 复重 重连");
        assert_eq!(cjk_bigrams("a重b"), "重");
        assert_eq!(cjk_bigrams("hello"), "");
    }

    #[test]
    fn query_plans_by_length_and_script() {
        let plans: Vec<_> = parse_query("重连 快照恢复 db x \"ws\"")
            .into_iter()
            .map(|t| (t.text, t.plan))
            .collect();
        assert_eq!(
            plans,
            vec![
                ("重连".into(), TermPlan::Bigram),
                ("快照恢复".into(), TermPlan::Trigram),
                ("db".into(), TermPlan::Like),
                ("x".into(), TermPlan::Like),
                ("ws".into(), TermPlan::Like),
            ]
        );
    }

    #[test]
    fn snippet_ranges_point_at_matches() {
        let terms = parse_query("Socket 重连");
        let text = "第一行\n这里讨论 WebSocket 断线后的重连策略，以及快照。";
        let s = snippet(text, &terms);
        let marked: Vec<&str> = s.highlights.iter().map(|r| &s.text[r.clone()]).collect();
        assert_eq!(marked, vec!["Socket", "重连"]);
        assert!(!s.text.contains('\n'));
    }

    #[test]
    fn placeholder_title_strips_mentions() {
        assert_eq!(
            placeholder_title("@src/main.rs 解释 这个函数"),
            "解释 这个函数"
        );
        assert_eq!(placeholder_title("   "), "新会话");
        assert_eq!(placeholder_title(&"长".repeat(30)).chars().count(), 25);
    }
}
