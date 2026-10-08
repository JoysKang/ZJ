//! Applying Codex's unified diffs, to show a file change as before ↔ after. Adapted from
//! Orbvane (MIT OR Apache-2.0, github.com/sbaruwal/orbvane, `crates/acp/src/codex.rs`).

/// Applies a unified diff to `old`. Each hunk is found by its lines (context and removed),
/// at the position its header names when that matches, else searching forward; None when a
/// hunk can't be placed.
pub(crate) fn apply_unified(old: &str, diff: &str) -> Option<String> {
    let lines: Vec<&str> = old.split_inclusive('\n').collect();
    let trim = |l: &str| {
        l.strip_suffix('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .unwrap_or(l)
            .to_string()
    };
    // (header's old start, lines before, lines after)
    let mut hunks: Vec<(Option<usize>, Vec<String>, Vec<String>)> = Vec::new();
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            let start = header
                .trim()
                .strip_prefix('-')
                .and_then(|s| s.split([',', ' ']).next())
                .and_then(|n| n.parse::<usize>().ok());
            hunks.push((start, Vec::new(), Vec::new()));
            continue;
        }
        let Some((_, before, after)) = hunks.last_mut() else {
            continue;
        };
        if line.starts_with("\\ ") {
            continue;
        }
        match line.chars().next() {
            Some('-') => before.push(line[1..].to_string()),
            Some('+') => after.push(line[1..].to_string()),
            Some(' ') => {
                before.push(line[1..].to_string());
                after.push(line[1..].to_string());
            }
            None => {
                before.push(String::new());
                after.push(String::new());
            }
            _ => {}
        }
    }
    if hunks.is_empty() {
        return None;
    }
    let ends_with_newline = old.is_empty() || old.ends_with('\n');
    let old_lines: Vec<String> = lines.iter().map(|l| trim(l)).collect();
    let mut out: Vec<String> = Vec::new();
    let mut at = 0;
    for (start, before, after) in hunks {
        let fits = |i: usize| {
            i + before.len() <= old_lines.len() && old_lines[i..i + before.len()] == before[..]
        };
        let hinted = start
            .map(|s| s.saturating_sub(1))
            .filter(|&i| i >= at && fits(i));
        let found = hinted.or_else(|| (at..=old_lines.len()).find(|&i| fits(i)))?;
        out.extend(old_lines[at..found].iter().cloned());
        out.extend(after);
        at = found + before.len();
    }
    out.extend(old_lines[at..].iter().cloned());
    let mut text = out.join("\n");
    if !text.is_empty() && ends_with_newline {
        text.push('\n');
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_unified_diffs() {
        let old = "one\ntwo\nthree\nfour\n";
        let diff = "--- a/notes.txt\n+++ b/notes.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n";
        assert_eq!(
            apply_unified(old, diff).as_deref(),
            Some("one\nTWO\nthree\nfour\n")
        );
        // A wrong line number: found by its lines.
        let diff = "@@ -9,2 +9,2 @@\n three\n-four\n+4\n";
        assert_eq!(
            apply_unified(old, diff).as_deref(),
            Some("one\ntwo\nthree\n4\n")
        );
        // Two hunks, and an addition at the end.
        let diff = "@@ -1,1 +1,1 @@\n-one\n+1\n@@ -4,1 +4,2 @@\n four\n+five\n";
        assert_eq!(
            apply_unified(old, diff).as_deref(),
            Some("1\ntwo\nthree\nfour\nfive\n")
        );
        // A hunk that doesn't fit.
        assert_eq!(apply_unified(old, "@@ -1 +1 @@\n-nine\n+9\n"), None);
        // A new file.
        assert_eq!(
            apply_unified("", "@@ -0,0 +1,2 @@\n+a\n+b\n").as_deref(),
            Some("a\nb\n")
        );
    }
}
