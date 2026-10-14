//! A tree of Git changes; leaves retain their original status indices.

use super::Row;
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
};
use workspace_editor_git::{DiffSide, Status};

#[derive(Clone)]
pub(super) struct FolderRow {
    pub(super) path: PathBuf,
    pub(super) label: PathBuf,
    pub(super) depth: usize,
}

impl FolderRow {
    pub(super) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.path.ancestors().take(self.label.components().count())
    }

    pub(super) fn is_collapsed(&self, collapsed: &HashSet<PathBuf>) -> bool {
        self.paths().any(|path| collapsed.contains(path))
    }
}

#[derive(Default)]
pub(super) struct ChangeTree {
    folders: BTreeMap<OsString, ChangeTree>,
    files: BTreeMap<OsString, usize>,
}

impl ChangeTree {
    pub(super) fn new(
        status: &Status,
        changes: &[usize],
        collapsed: &mut HashSet<PathBuf>,
    ) -> Self {
        let mut tree = Self::default();
        let mut present = HashSet::new();
        for &index in changes {
            let path = &status.changes[index].path;
            let mut node = &mut tree;
            let mut directory = PathBuf::new();
            if let Some(parent) = path.parent() {
                for name in parent.components() {
                    directory.push(name.as_os_str());
                    present.insert(directory.clone());
                    node = node.folders.entry(name.as_os_str().to_owned()).or_default();
                }
            }
            if let Some(name) = path.file_name() {
                node.files.insert(name.to_owned(), index);
            }
        }
        collapsed.retain(|path| present.contains(path));
        tree
    }

    pub(super) fn append_rows(
        &self,
        group: usize,
        side: DiffSide,
        collapsed: &HashSet<PathBuf>,
        rows: &mut Vec<Row>,
    ) {
        self.append_children(Path::new(""), 0, group, side, collapsed, rows);
    }

    fn append_children(
        &self,
        parent: &Path,
        depth: usize,
        group: usize,
        side: DiffSide,
        collapsed: &HashSet<PathBuf>,
        rows: &mut Vec<Row>,
    ) {
        for (name, mut folder) in &self.folders {
            let mut path = parent.join(name);
            while folder.files.is_empty() && folder.folders.len() == 1 {
                let (name, child) = folder.folders.first_key_value().unwrap();
                path.push(name);
                folder = child;
            }
            let row = FolderRow {
                label: path.strip_prefix(parent).unwrap().to_owned(),
                path,
                depth,
            };
            let expanded = !row.is_collapsed(collapsed);
            let path = row.path.clone();
            rows.push(Row::Folder(group, row, side));
            if expanded {
                folder.append_children(&path, depth + 1, group, side, collapsed, rows);
            }
        }
        rows.extend(
            self.files
                .values()
                .map(|&index| Row::File(group, index, side, depth)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workspace_editor_git::{Change, ChangeKind};

    fn status(paths: &[&str]) -> Status {
        Status {
            changes: paths
                .iter()
                .map(|path| Change {
                    path: PathBuf::from(path),
                    original_path: None,
                    index: b'.',
                    worktree: b'M',
                    kind: ChangeKind::Tracked,
                    submodule: *b"N...",
                })
                .collect(),
            ..Default::default()
        }
    }

    fn rows(status: &Status, collapsed: &mut HashSet<PathBuf>) -> Vec<Row> {
        let changes = (0..status.changes.len()).collect::<Vec<_>>();
        let tree = ChangeTree::new(status, &changes, collapsed);
        let mut rows = Vec::new();
        tree.append_rows(7, DiffSide::Worktree, collapsed, &mut rows);
        rows
    }

    fn snapshot(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Folder(_, folder, _) => format!(
                    "folder {} [{}] {}",
                    folder.path.display(),
                    folder.label.display(),
                    folder.depth
                ),
                Row::File(_, index, _, depth) => format!("file {index} {depth}"),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn compact_chain_preserves_visual_depth_order_and_status_indices() {
        let status = status(&[
            "crates/app/src/workbench/z.rs",
            "root.rs",
            "crates/app/src/workbench/a.rs",
        ]);
        let rows = rows(&status, &mut HashSet::new());
        assert_eq!(
            snapshot(&rows),
            [
                "folder crates/app/src/workbench [crates/app/src/workbench] 0",
                "file 2 1",
                "file 0 1",
                "file 1 0",
            ]
        );
        assert!(rows.iter().all(|row| matches!(
            row,
            Row::Folder(7, _, DiffSide::Worktree) | Row::File(7, _, DiffSide::Worktree, _)
        )));
    }

    #[test]
    fn branches_and_direct_files_stop_compaction() {
        let status = status(&[
            "a/file.rs",
            "a/b/c/child.rs",
            "branch/left/a.rs",
            "branch/right/b.rs",
            "root.rs",
        ]);
        assert_eq!(
            snapshot(&rows(&status, &mut HashSet::new())),
            [
                "folder a [a] 0",
                "folder a/b/c [b/c] 1",
                "file 1 2",
                "file 0 1",
                "folder branch [branch] 0",
                "folder branch/left [left] 1",
                "file 2 2",
                "folder branch/right [right] 1",
                "file 3 2",
                "file 4 0",
            ]
        );
    }

    #[test]
    fn legacy_fold_keys_in_the_chain_collapse_and_expand_together() {
        let status = status(&["a/b/c/file.rs"]);
        let mut collapsed = HashSet::from([PathBuf::from("a"), PathBuf::from("a/b")]);
        let folded = rows(&status, &mut collapsed);
        assert_eq!(folded.len(), 1);
        let Row::Folder(_, folder, _) = &folded[0] else {
            panic!("folder expected")
        };
        assert_eq!(
            folder.paths().collect::<Vec<_>>(),
            [Path::new("a/b/c"), Path::new("a/b"), Path::new("a")]
        );
        assert!(folder.is_collapsed(&collapsed));
        for path in folder.paths() {
            collapsed.remove(path);
        }
        assert!(collapsed.is_empty());
        assert_eq!(rows(&status, &mut collapsed).len(), 2);
    }

    #[test]
    fn refresh_branch_preserves_tail_fold_without_hiding_new_sibling() {
        let status = status(&["a/b/c/file.rs", "a/b/sibling/new.rs"]);
        let mut collapsed = HashSet::from([PathBuf::from("a/b/c")]);
        assert_eq!(
            snapshot(&rows(&status, &mut collapsed)),
            [
                "folder a/b [a/b] 0",
                "folder a/b/c [c] 1",
                "folder a/b/sibling [sibling] 1",
                "file 1 2",
            ]
        );
        collapsed.insert(PathBuf::from("a"));
        assert_eq!(
            snapshot(&rows(&status, &mut collapsed)),
            ["folder a/b [a/b] 0"]
        );
    }

    #[test]
    fn shortening_or_removing_a_chain_prunes_vanished_fold_keys() {
        let mut collapsed = HashSet::from([PathBuf::from("a/b/c")]);
        assert_eq!(rows(&status(&["a/b/file.rs"]), &mut collapsed).len(), 2);
        assert!(collapsed.is_empty());
        assert_eq!(rows(&status(&["a/b/c/file.rs"]), &mut collapsed).len(), 2);
        collapsed.insert(PathBuf::from("a"));
        assert!(rows(&status(&[]), &mut collapsed).is_empty());
        assert!(collapsed.is_empty());
        assert_eq!(rows(&status(&["a/b/c/file.rs"]), &mut collapsed).len(), 2);
    }
}
