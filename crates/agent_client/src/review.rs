//! Reviewing an agent's changes hunk by hunk: a full-context patch for the diff editor (one
//! change block per [`Hunk`], in the same order) and the Direct-mode decisions, which move
//! the "before" snapshot (accept) or the file on disk (reject).

use crate::shadow::{Hunk, apply_hunks, diff_hunks};

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
        for line in &new[hunk.proposed.clone()] {
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
        .fold((0, 0), |(a, r), h| (a + h.proposed.len(), r + h.base.len()))
}

/// Direct mode, accepting hunk `index` of `before → current`: the new "before" text, which
/// already contains that hunk (so it drops out of the review).
pub fn accept_written_hunk(before: &str, current: &str, index: usize) -> String {
    apply_hunks(before, current, &[index])
}

/// Direct mode, rejecting hunk `index`: the file text with just that hunk reverted.
pub fn reject_written_hunk(before: &str, current: &str, index: usize) -> String {
    let count = diff_hunks(before, current).len();
    let keep: Vec<usize> = (0..count).filter(|i| *i != index).collect();
    apply_hunks(before, current, &keep)
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
