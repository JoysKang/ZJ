//! Reviewing an agent's changes hunk by hunk: a full-context patch for the diff editor (one
//! change block per [`Hunk`], in the same order) and the decisions, which move the "before"
//! snapshot (accept) or the file on disk (reject).

use crate::{
    AgentClient,
    diff::{Hunk, apply_hunks, diff_hunks},
};
use std::path::Path;

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A unified patch with every line as context, so the diff editor can show the whole file.
/// Change block `i` of the parsed patch is `hunks[i]`.
pub fn full_context_patch(before: &str, after: &str) -> (String, Vec<Hunk>) {
    let hunks = diff_hunks(before, after);
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let new: Vec<&str> = after.split_inclusive('\n').collect();
    let mut out = String::with_capacity(before.len() + after.len() + 64);
    out.push_str(&format!("@@ -1,{} +1,{} @@\n", old.len(), new.len()));
    let mut push = |prefix: char, line: &str| {
        out.push(prefix);
        out.push_str(line.strip_suffix('\n').unwrap_or(line));
        out.push('\n');
    };
    let mut at = 0;
    for hunk in &hunks {
        for line in &old[at..hunk.base.start] {
            push(' ', line);
        }
        for line in &old[hunk.base.clone()] {
            push('-', line);
        }
        for line in &new[hunk.after.clone()] {
            push('+', line);
        }
        at = hunk.base.end;
    }
    for line in &old[at..] {
        push(' ', line);
    }
    (out, hunks)
}

/// Lines added and removed between two versions.
pub fn line_counts(before: &str, after: &str) -> (usize, usize) {
    diff_hunks(before, after)
        .iter()
        .fold((0, 0), |(a, r), h| (a + h.after.len(), r + h.base.len()))
}

/// Accepting hunk `index` of `before → current`: the new "before" text, which
/// already contains that hunk (so it drops out of the review).
pub fn accept_written_hunk(before: &str, current: &str, index: usize) -> String {
    apply_hunks(before, current, &[index])
}

/// Rejecting hunk `index`: the file text with just that hunk reverted.
pub fn reject_written_hunk(before: &str, current: &str, index: usize) -> String {
    let count = diff_hunks(before, current).len();
    let keep: Vec<usize> = (0..count).filter(|i| *i != index).collect();
    apply_hunks(before, current, &keep)
}

/// The review base and current text of a changed file: (before, after). `None` when nothing
/// is pending for it.
pub fn review_texts(client: &AgentClient, path: &Path) -> Option<(Option<String>, String)> {
    let before = client.snapshot(path)?;
    let after = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    Some((before, after))
}

/// Accepts or rejects a whole file: accepting forgets the snapshot, rejecting restores it on
/// disk (or removes a file the agent created).
pub fn resolve_file(client: &AgentClient, path: &Path, accept: bool) -> Result<(), String> {
    let name = file_name(path);
    let Some(before) = client.snapshot(path) else {
        return Ok(());
    };
    if !accept {
        match &before {
            Some(text) => crate::fs::write_atomic(path, text),
            None => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        }
        .map_err(|e| format!("{name} 未能还原：{e}"))?;
        eprintln!("event=agent_review_reverted_file");
    }
    client.set_snapshot(path, None);
    Ok(())
}

/// Returned when the file changed after the review was shown: block `index` may now be a
/// different change, so nothing is applied and the caller reloads the review.
pub const STALE_REVIEW: &str = "这个文件在审阅期间又有修改，已重新载入，请再选一次";

/// Accepts or rejects one hunk (block `index` of the review diff). `shown` is the
/// [`full_context_patch`] the user was looking at; if the file has moved on since, block
/// `index` could be another change, so the call fails with [`STALE_REVIEW`].
pub fn resolve_hunk(
    client: &AgentClient,
    path: &Path,
    index: usize,
    accept: bool,
    shown: &str,
) -> Result<(), String> {
    let name = file_name(path);
    let current = review_texts(client, path)
        .map(|(before, after)| full_context_patch(before.as_deref().unwrap_or(""), &after).0);
    if current.as_deref() != Some(shown) {
        return Err(STALE_REVIEW.into());
    }
    let Some((before, current)) = review_texts(client, path) else {
        return Ok(());
    };
    let base = before.clone().unwrap_or_default();
    if accept {
        let next = accept_written_hunk(&base, &current, index);
        client.set_snapshot(path, (next != current).then_some(Some(next)));
    } else {
        let next = reject_written_hunk(&base, &current, index);
        let result = if before.is_none() && next.is_empty() {
            std::fs::remove_file(path)
        } else {
            crate::fs::write_atomic(path, &next)
        };
        result.map_err(|e| format!("{name} 未能还原这一处：{e}"))?;
        if next == base {
            client.set_snapshot(path, None);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_blocks_follow_hunks() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nB\nc\nd\nE\nf\n";
        let (patch, hunks) = full_context_patch(before, after);
        assert_eq!(hunks.len(), 2);
        assert_eq!(patch, "@@ -1,5 +1,6 @@\n a\n-b\n+B\n c\n d\n-e\n+E\n+f\n");
        // A new file is all additions.
        let (patch, hunks) = full_context_patch("", "x\ny");
        assert_eq!(patch, "@@ -1,0 +1,2 @@\n+x\n+y\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(line_counts(before, after), (3, 2));
    }

    #[test]
    fn written_hunks_accept_into_the_snapshot_or_revert_on_disk() {
        let before = "a\nb\nc\nd\ne\n";
        let current = "a\nB\nc\nd\nE\n";
        let new_before = accept_written_hunk(before, current, 0);
        assert_eq!(new_before, "a\nB\nc\nd\ne\n");
        assert_eq!(diff_hunks(&new_before, current).len(), 1);
        let reverted = reject_written_hunk(before, current, 1);
        assert_eq!(reverted, "a\nB\nc\nd\ne\n");
        assert_eq!(reject_written_hunk(before, current, 0), "a\nb\nc\nd\nE\n");
    }
}
