//! Subsequence fuzzy matching for quick open. Inputs are already lowercased.

const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 24;
const BOUNDARY: i32 = 30;
const NAME_BONUS: i32 = 60;
const NAME_PREFIX: i32 = 40;

/// Prepares a query: lowercase, whitespace removed (so `src main` matches `src/main.rs`).
pub fn query_chars(query: &str) -> Vec<char> {
    query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Scores `candidate` (lowercase relative path) against `query`, or `None` when the query is not
/// a subsequence. `name_start` is the byte offset where the file name begins.
///
/// Matches inside the file name are preferred; within a region, consecutive runs and characters
/// at word boundaries score higher, gaps and long paths score lower.
pub fn score(query: &[char], candidate: &str, name_start: usize) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let name = &candidate[name_start..];
    if let Some(score) = greedy(query, name) {
        let prefix = if name.starts_with(query[0]) {
            NAME_PREFIX
        } else {
            0
        };
        return Some(score + NAME_BONUS + prefix - length_penalty(candidate));
    }
    greedy(query, candidate).map(|score| score - length_penalty(candidate))
}

fn length_penalty(candidate: &str) -> i32 {
    (candidate.chars().count() / 4) as i32
}

fn greedy(query: &[char], text: &str) -> Option<i32> {
    let mut wanted = query.iter().peekable();
    let mut score = 0;
    let mut previous: Option<char> = None;
    let mut last_match: Option<usize> = None;
    for (index, c) in text.chars().enumerate() {
        let Some(&&q) = wanted.peek() else {
            break;
        };
        if c == q {
            score += MATCH;
            if last_match.is_some_and(|last| last + 1 == index) {
                score += CONSECUTIVE;
            } else {
                let gap = last_match.map_or(index, |last| index - last - 1);
                score -= gap.min(12) as i32;
            }
            if previous.is_none_or(|p| matches!(p, '/' | '_' | '-' | '.' | ' ')) {
                score += BOUNDARY;
            }
            last_match = Some(index);
            wanted.next();
        }
        previous = Some(c);
    }
    wanted.peek().is_none().then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, candidates: &[&'a str]) -> Vec<&'a str> {
        let q = query_chars(query);
        let mut scored: Vec<_> = candidates
            .iter()
            .filter_map(|c| {
                let start = c.rfind('/').map_or(0, |i| i + 1);
                score(&q, c, start).map(|s| (s, *c))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    #[test]
    fn requires_subsequence() {
        let q = query_chars("mnrs");
        assert!(score(&q, "src/main.rs", 4).is_some());
        assert!(score(&q, "src/lib.rs", 4).is_none());
        assert_eq!(score(&[], "anything", 0), Some(0));
    }

    #[test]
    fn file_name_beats_scattered_directories() {
        assert_eq!(
            rank("main", &["src/domain/mod.rs", "src/domain/main.rs"])[0],
            "src/domain/main.rs"
        );
        assert_eq!(
            rank("lib", &["crates/lib_utils/mod.rs", "src/lib.rs"])[0],
            "src/lib.rs"
        );
    }

    #[test]
    fn consecutive_and_boundaries_beat_scattered() {
        assert_eq!(
            rank("status", &["stack_trace_utils.rs", "status.rs"])[0],
            "status.rs"
        );
        assert_eq!(
            rank("gs", &["bigs.rs", "git_service.rs"])[0],
            "git_service.rs"
        );
    }

    #[test]
    fn shorter_path_wins_ties_and_query_ignores_case_and_spaces() {
        assert_eq!(
            rank("Main RS", &["a/very/deep/tree/main.rs", "main.rs"]),
            vec!["main.rs", "a/very/deep/tree/main.rs"]
        );
    }

    #[test]
    fn non_ascii_paths_match() {
        let q = query_chars("说明");
        let candidate = "docs/开发说明.md";
        assert!(score(&q, candidate, candidate.rfind('/').unwrap() + 1).is_some());
    }
}
