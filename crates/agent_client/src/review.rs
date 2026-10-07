//! Reviewing an agent's changes hunk by hunk: a full-context patch for the diff editor (one
//! change block per [`Hunk`], in the same order) and the decisions, which move the "before"
//! snapshot (accept) or the file on disk (reject).

use crate::{
    AgentClient,
    diff::{Hunk, apply_hunks, diff_hunks},
};
use std::{path::Path, sync::Mutex};

// ponytail: serialize background reviews across sessions; use weak per-path locks only if
// unrelated large-file reviews contend. UI snapshot reads never take this lock.
static DECISIONS: Mutex<()> = Mutex::new(());

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

/// Where each part occurs in `text`, in order and without overlapping.
fn find_in_order(text: &str, parts: &[&str]) -> Option<Vec<usize>> {
    let mut at = 0;
    parts
        .iter()
        .map(|part| {
            let found = at + text.get(at..)?.find(part)?;
            at = found + part.len();
            Some(found)
        })
        .collect()
}

/// The file before an edit the agent applied itself, from the edit's hunks (`(old, new)`
/// fragments with their context lines, as codex-acp sends them) and the file now, which may
/// still be the old text (the notice came first) or already the new one (the agent wrote it
/// first). `None` when that cannot be told: the fragments match neither, or both.
pub fn text_before_hunks(current: &str, hunks: &[(&str, &str)]) -> Option<String> {
    if hunks.is_empty()
        || hunks
            .iter()
            .any(|(old, new)| old.is_empty() || new.is_empty())
    {
        return None;
    }
    let olds: Vec<&str> = hunks.iter().map(|(old, _)| *old).collect();
    let news: Vec<&str> = hunks.iter().map(|(_, new)| *new).collect();
    match (find_in_order(current, &olds), find_in_order(current, &news)) {
        (Some(_), None) => Some(current.to_string()),
        (None, Some(at)) => {
            let mut before = String::with_capacity(current.len());
            let mut from = 0;
            for (start, (old, new)) in at.into_iter().zip(hunks) {
                before.push_str(&current[from..start]);
                before.push_str(old);
                from = start + new.len();
            }
            before.push_str(&current[from..]);
            Some(before)
        }
        _ => None,
    }
}

/// The review base and current text of a changed file: (before, after). `None` when nothing
/// is pending for it.
pub fn review_texts(client: &AgentClient, path: &Path) -> Option<(Option<String>, String)> {
    let _decision = DECISIONS.lock().unwrap();
    read_review_texts(client, path)
}

fn read_review_texts(client: &AgentClient, path: &Path) -> Option<(Option<String>, String)> {
    let before = client.snapshot(path)?;
    // Bounded like the agent's own reads: a huge generated file is not read for a review.
    let after = match crate::fs::read_disk(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    Some((before, after))
}

/// Accepts or rejects a whole file: accepting forgets the snapshot, rejecting restores it on
/// disk (or removes a file the agent created).
pub fn resolve_file(client: &AgentClient, path: &Path, accept: bool) -> Result<(), String> {
    let _decision = DECISIONS.lock().unwrap();
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
    let _decision = DECISIONS.lock().unwrap();
    let name = file_name(path);
    let Some((before, current)) = read_review_texts(client, path) else {
        return Err(STALE_REVIEW.into());
    };
    let base = before.clone().unwrap_or_default();
    let (patch, hunks) = full_context_patch(&base, &current);
    if patch != shown || index >= hunks.len() {
        return Err(STALE_REVIEW.into());
    }
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
    fn the_text_before_hunks_comes_from_either_side() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nB\nc\nd\ne\nf\n";
        let hunks = [("a\nb\nc\n", "a\nB\nc\n"), ("d\ne\n", "d\ne\nf\n")];
        // Written already, or not yet: the same text before.
        assert_eq!(text_before_hunks(after, &hunks).as_deref(), Some(before));
        assert_eq!(text_before_hunks(before, &hunks).as_deref(), Some(before));
        // Neither side (the file moved on), or an empty fragment: unknown.
        assert_eq!(text_before_hunks("x\n", &hunks), None);
        assert_eq!(text_before_hunks(after, &[("a\n", "")]), None);
    }

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
