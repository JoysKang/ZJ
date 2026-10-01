//! Diff editor interaction: line selection and copy, change blocks for the overview ruler and
//! gutter actions, and staging / unstaging / reverting blocks or selected lines through a
//! generated partial patch (`git apply`).

use super::diff_view::DiffList;
use super::*;
use crate::{
    diff_doc::{DiffDoc, LineKind, RowKind},
    partial_patch::{Direction, RawLine},
};
use std::collections::HashSet;

gpui_kit::actions!(diff, [CopyDiff, SelectAllDiff]);

/// Selected rows of one list, from the row where the drag started to the current row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffSelection {
    pub list: DiffList,
    pub anchor: usize,
    pub head: usize,
}

impl DiffSelection {
    pub fn rows(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.min(self.head)..=self.anchor.max(self.head)
    }

    pub fn contains(&self, list: DiffList, row: usize) -> bool {
        self.list == list && self.rows().contains(&row)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    Added,
    Removed,
    Modified,
}

/// A run of changed rows: `start..end` in the current layout's rows.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub kind: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HunkAction {
    Stage,
    Unstage,
    Revert,
}

/// Change blocks in row order for the side-by-side (`inline == false`) or inline layout.
pub fn blocks(doc: &DiffDoc, inline: bool) -> Vec<Block> {
    let mut blocks = Vec::new();
    if inline {
        for &start in &doc.inline_changes {
            let end = (start..doc.inline.len())
                .find(|&i| doc.inline[i].kind == LineKind::Same)
                .unwrap_or(doc.inline.len());
            let rows = &doc.inline[start..end];
            let removed = rows.iter().any(|r| r.kind == LineKind::Removed);
            let added = rows.iter().any(|r| r.kind == LineKind::Added);
            blocks.push(Block {
                start,
                end,
                kind: kind(added, removed),
            });
        }
    } else {
        for &start in &doc.changes {
            let end = (start..doc.rows.len())
                .find(|&i| doc.rows[i].kind == RowKind::Same)
                .unwrap_or(doc.rows.len());
            let rows = &doc.rows[start..end];
            blocks.push(Block {
                start,
                end,
                kind: kind(
                    rows.iter().any(|r| r.new.is_some()),
                    rows.iter().any(|r| r.old.is_some()),
                ),
            });
        }
    }
    blocks
}

fn kind(added: bool, removed: bool) -> BlockKind {
    match (added, removed) {
        (true, false) => BlockKind::Added,
        (false, true) => BlockKind::Removed,
        _ => BlockKind::Modified,
    }
}

/// Changed old / new line numbers in `rows` of the given layout.
pub fn changed_lines(
    doc: &DiffDoc,
    inline: bool,
    rows: impl Iterator<Item = usize>,
) -> (HashSet<u32>, HashSet<u32>) {
    let (mut old, mut new) = (HashSet::new(), HashSet::new());
    for index in rows {
        if inline {
            let Some(row) = doc.inline.get(index) else {
                continue;
            };
            match row.kind {
                LineKind::Removed => {
                    old.insert(doc.old.lines[row.line as usize].number);
                }
                LineKind::Added => {
                    new.insert(doc.new.lines[row.line as usize].number);
                }
                LineKind::Same => {}
            }
        } else {
            let Some(row) = doc.rows.get(index) else {
                continue;
            };
            if row.kind == RowKind::Changed {
                if let Some(o) = row.old {
                    old.insert(doc.old.lines[o as usize].number);
                }
                if let Some(n) = row.new {
                    new.insert(doc.new.lines[n as usize].number);
                }
            }
        }
    }
    (old, new)
}

impl Prototype {
    pub(super) fn diff_staged(&self) -> bool {
        self.preview_diff.as_ref().is_some_and(|tab| {
            matches!(
                tab.request.operation,
                Operation::Diff {
                    side: DiffSide::Staged,
                    ..
                }
            )
        })
    }

    /// Whether lines of this diff can be staged, unstaged or reverted on their own.
    pub(super) fn diff_partial_ok(&self) -> bool {
        self.diff_raw
            .as_ref()
            .is_some_and(|raw| !raw.whole_file_only)
            && self.diff_doc.is_some()
            && !self
                .preview_diff
                .as_ref()
                .and_then(|tab| {
                    self.groups
                        .iter()
                        .find(|g| g.repo.id == tab.request.repo.id)
                })
                .is_none_or(|g| g.write_pending)
    }

    pub(super) fn diff_select(
        &mut self,
        list: DiffList,
        row: usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.diff_focus.focus(window, cx);
        let anchor = self
            .diff_selection
            .filter(|s| extend && s.list == list)
            .map_or(row, |s| s.anchor);
        self.diff_selection = Some(DiffSelection {
            list,
            anchor,
            head: row,
        });
        self.diff_dragging = true;
        cx.notify();
    }

    pub(super) fn diff_drag_to(&mut self, list: DiffList, row: usize, cx: &mut Context<Self>) {
        if !self.diff_dragging {
            return;
        }
        if let Some(selection) = &mut self.diff_selection
            && selection.list == list
            && selection.head != row
        {
            selection.head = row;
            cx.notify();
        }
    }

    pub(super) fn diff_select_all(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = &self.diff_doc else { return };
        let (list, count) = if self.diff_is_inline() {
            (DiffList::Inline, doc.inline.len())
        } else {
            (DiffList::Modified, doc.rows.len())
        };
        if count > 0 {
            self.diff_selection = Some(DiffSelection {
                list,
                anchor: 0,
                head: count - 1,
            });
            cx.notify();
        }
    }

    /// ⌘C: Git's exact text of the selected lines of the list the selection is in.
    pub(super) fn copy_diff_selection(&mut self, cx: &mut Context<Self>) {
        let (Some(doc), Some(selection)) = (&self.diff_doc, self.diff_selection) else {
            return;
        };
        let raw = self.diff_raw.clone();
        let line = |old_side: bool, index: u32| -> String {
            let side = if old_side { &doc.old } else { &doc.new };
            let number = side.lines[index as usize].number;
            raw.as_ref()
                .and_then(|raw| raw.text(old_side, number))
                .map(str::to_string)
                .unwrap_or_else(|| side.line_text(index as usize).to_string())
        };
        let mut lines = Vec::new();
        for row in selection.rows() {
            match selection.list {
                DiffList::Inline => {
                    if let Some(r) = doc.inline.get(row) {
                        lines.push(line(r.kind != LineKind::Added, r.line));
                    }
                }
                DiffList::Original => {
                    if let Some(o) = doc.rows.get(row).and_then(|r| r.old) {
                        lines.push(line(true, o));
                    }
                }
                DiffList::Modified => {
                    if let Some(n) = doc.rows.get(row).and_then(|r| r.new) {
                        lines.push(line(false, n));
                    }
                }
            }
        }
        if !lines.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
        }
    }

    /// Stages, unstages or reverts the changed lines of `rows` (a block or the selection).
    pub(super) fn diff_apply_rows(
        &mut self,
        action: HunkAction,
        rows: std::ops::Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(doc), Some(raw), Some(tab)) =
            (&self.diff_doc, self.diff_raw.clone(), &self.preview_diff)
        else {
            return;
        };
        let (old, new) = changed_lines(doc, self.diff_is_inline(), rows);
        let selected = |line: &RawLine| match line.kind {
            b'-' => line.old.is_some_and(|n| old.contains(&n)),
            b'+' => line.new.is_some_and(|n| new.contains(&n)),
            _ => false,
        };
        let (direction, cached) = match action {
            HunkAction::Stage => (Direction::Apply, true),
            HunkAction::Unstage => (Direction::Undo, true),
            HunkAction::Revert => (Direction::Undo, false),
        };
        let Some(patch) = raw.select(&selected, direction) else {
            return;
        };
        let path = match &tab.request.operation {
            Operation::Diff { path, .. } | Operation::UntrackedDiff { path } => path.clone(),
            Operation::Status => return,
        };
        let Some(group) = self
            .groups
            .iter()
            .find(|g| g.repo.id == tab.request.repo.id)
        else {
            return;
        };
        let Some(Ok(status)) = &group.status else {
            return;
        };
        let request = WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation: WriteOperation::ApplyPatch {
                path,
                patch,
                cached,
            },
        };
        self.diff_selection = None;
        self.request_git_write(request, window, cx);
    }

    /// The selection as a row range, for the toolbar actions.
    pub(super) fn diff_selection_rows(&self) -> Option<std::ops::Range<usize>> {
        let selection = self.diff_selection?;
        let inline = self.diff_is_inline();
        if inline != (selection.list == DiffList::Inline) {
            return None;
        }
        let rows = selection.rows();
        Some(*rows.start()..*rows.end() + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockKind, DiffDoc, HashSet, blocks, changed_lines};
    use crate::diff_doc::ChangeColors;
    use gpui_kit::component::highlighter::HighlightTheme;

    #[test]
    fn blocks_and_changed_lines_follow_both_layouts() {
        let patch = "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,4 +1,4 @@\n one\n-two\n+TWO\n three\n-four\n+four!\n+five\n";
        let colors = ChangeColors {
            inserted_text: gpui_kit::Hsla::default(),
            removed_text: gpui_kit::Hsla::default(),
        };
        let doc = DiffDoc::parse(patch, None, &HighlightTheme::default_dark(), colors).unwrap();
        let pair = blocks(&doc, false);
        assert_eq!(pair.len(), 2);
        assert_eq!(
            (pair[0].start, pair[0].end, pair[0].kind),
            (1, 2, BlockKind::Modified)
        );
        let (old, new) = changed_lines(&doc, false, pair[1].start..pair[1].end);
        assert_eq!(old, HashSet::from([4]));
        assert_eq!(new, HashSet::from([4, 5]));
        let inline = blocks(&doc, true);
        assert_eq!(inline.len(), 2);
        let (old, new) = changed_lines(&doc, true, inline[0].start..inline[0].end);
        assert_eq!((old, new), (HashSet::from([2]), HashSet::from([2])));
    }
}
