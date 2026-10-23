//! Partial staging: builds a patch that applies only the selected lines of a full-context
//! diff, for `git apply --cached` (stage / unstage) or `git apply` (revert), like VS Code's
//! "Stage Selected Ranges" and `git add -p`.
//!
//! Works on Git's own patch text, so CRLF, tabs and "\ No newline at end of file" survive
//! exactly. Unselected removals stay as context, unselected additions are dropped; the base
//! file is either the old side (apply selected changes) or the new side (undo them).

/// One line of the single full-context hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLine {
    pub kind: u8,
    /// Without the line break; may end in `\r`.
    pub text: String,
    /// False when Git marked it "\ No newline at end of file".
    pub eol: bool,
    /// 1-based line numbers on the old / new side.
    pub old: Option<u32>,
    pub new: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct RawPatch {
    header: Vec<String>,
    pub lines: Vec<RawLine>,
    /// The old side is /dev/null (untracked or newly added file).
    pub created: bool,
    /// The new side is /dev/null, or old and new paths differ: whole-file actions only.
    pub whole_file_only: bool,
    /// Line number − 1 → index into `lines`, per side.
    old_index: Vec<u32>,
    new_index: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Base is the old side; selected changes are applied (stage).
    Apply,
    /// Base is the new side; selected changes are undone (unstage, revert).
    Undo,
}

/// Parses a single-file, single-hunk patch from `git diff --unified=<large>`.
pub fn parse(patch: &str) -> Option<RawPatch> {
    let mut header = Vec::new();
    let mut lines: Vec<RawLine> = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    let mut hunks = 0;
    let (mut minus, mut plus) = (String::new(), String::new());
    for raw in patch.split_inclusive('\n') {
        let line = raw.strip_suffix('\n').unwrap_or(raw);
        if line.starts_with("@@ ") {
            hunks += 1;
            if hunks > 1 {
                return None;
            }
            let mut parts = line.split_whitespace().skip(1);
            let start = |s: Option<&str>, prefix: char| -> Option<u32> {
                s?.strip_prefix(prefix)?.split(',').next()?.parse().ok()
            };
            old = start(parts.next(), '-')?.max(1);
            new = start(parts.next(), '+')?.max(1);
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                return None;
            }
            if let Some(path) = line.strip_prefix("--- ") {
                minus = path.to_string();
            }
            if let Some(path) = line.strip_prefix("+++ ") {
                plus = path.to_string();
            }
            if !line.starts_with("index ") {
                header.push(line.to_string());
            }
            continue;
        }
        let (kind, text) = match line.as_bytes().first() {
            Some(b'\\') => {
                if let Some(last) = lines.last_mut() {
                    last.eol = false;
                }
                continue;
            }
            Some(&kind @ (b' ' | b'-' | b'+')) => (kind, &line[1..]),
            None => (b' ', ""),
            _ => return None,
        };
        let (o, n) = match kind {
            b' ' => (Some(old), Some(new)),
            b'-' => (Some(old), None),
            _ => (None, Some(new)),
        };
        old += u32::from(o.is_some());
        new += u32::from(n.is_some());
        lines.push(RawLine {
            kind,
            text: text.to_string(),
            eol: true,
            old: o,
            new: n,
        });
    }
    if hunks == 0 {
        return None;
    }
    let strip = |path: &str, side: &str| {
        path.trim_matches('"')
            .strip_prefix(side)
            .map(str::to_string)
            .unwrap_or_else(|| path.to_string())
    };
    let created = minus == "/dev/null";
    let deleted = plus == "/dev/null";
    let renamed = !created && !deleted && strip(&minus, "a/") != strip(&plus, "b/");
    let index = |side: fn(&RawLine) -> Option<u32>| {
        let mut map = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(number) = side(line) {
                map.resize(number as usize, u32::MAX);
                map[number as usize - 1] = i as u32;
            }
        }
        map
    };
    let old_index = index(|l| l.old);
    let new_index = index(|l| l.new);
    Some(RawPatch {
        header,
        lines,
        created,
        whole_file_only: deleted || renamed,
        old_index,
        new_index,
    })
}

enum Op<'a> {
    Context(&'a RawLine),
    Del(&'a RawLine),
    Add(&'a RawLine),
}

impl RawPatch {
    /// Git's exact text of a line (without CR), by side and 1-based line number.
    pub fn text(&self, old_side: bool, number: u32) -> Option<&str> {
        let map = if old_side {
            &self.old_index
        } else {
            &self.new_index
        };
        let index = *map.get((number as usize).checked_sub(1)?)?;
        let line = self.lines.get(index as usize)?;
        Some(line.text.strip_suffix('\r').unwrap_or(&line.text))
    }

    /// The patch text for the selected lines, or `None` when nothing changes.
    pub fn select_aligned(
        &self,
        selected: &dyn Fn(&RawLine) -> bool,
        direction: Direction,
        alignment: impl IntoIterator<Item = (Option<u32>, Option<u32>)>,
    ) -> Option<String> {
        if self.whole_file_only {
            return None;
        }
        let mut ops = Vec::with_capacity(self.lines.len());
        let mut changes = 0;
        let mut take = |line| {
            let line: &RawLine = line;
            let chosen = line.kind != b' ' && selected(line);
            let (taken, kept) = match direction {
                Direction::Apply => (b'-', b'+'),
                Direction::Undo => (b'+', b'-'),
            };
            match line.kind {
                b' ' => ops.push(Op::Context(line)),
                kind if kind == taken => {
                    // A base line: removed if chosen, otherwise kept as context.
                    if chosen {
                        changes += 1;
                        ops.push(Op::Del(line));
                    } else {
                        ops.push(Op::Context(line));
                    }
                }
                // A target-only line: added if chosen, otherwise left out.
                kind if kind == kept && chosen => {
                    changes += 1;
                    ops.push(Op::Add(line));
                }
                _ => {}
            }
        };
        // Use the displayed monotonic alignment, including filler rows. Re-pairing by
        // ordinal would move a replacement past an unselected insertion or deletion.
        let (mut old_count, mut new_count) = (0, 0);
        for (old, new) in alignment {
            let lookup = |number: u32, index: &[u32]| {
                self.lines
                    .get(*index.get(number.checked_sub(1)? as usize)? as usize)
            };
            let old = match old {
                Some(number) => {
                    if number != old_count + 1 {
                        return None;
                    }
                    old_count = number;
                    Some(lookup(number, &self.old_index)?)
                }
                None => None,
            };
            let new = match new {
                Some(number) => {
                    if number != new_count + 1 {
                        return None;
                    }
                    new_count = number;
                    Some(lookup(number, &self.new_index)?)
                }
                None => None,
            };
            match (old, new) {
                (Some(old), Some(new)) if std::ptr::eq(old, new) => take(old),
                (old, new) => {
                    if let Some(line) = old {
                        take(line);
                    }
                    if let Some(line) = new {
                        take(line);
                    }
                }
            }
        }
        if old_count as usize != self.old_index.len() || new_count as usize != self.new_index.len()
        {
            return None;
        }
        if changes == 0 {
            return None;
        }
        let base_count = ops.iter().filter(|op| !matches!(op, Op::Add(_))).count();
        let target_count = ops.iter().filter(|op| !matches!(op, Op::Del(_))).count();
        let last_target = ops.iter().rposition(|op| !matches!(op, Op::Del(_)));
        let mut body = String::new();
        let push = |prefix: char, line: &RawLine, eol: bool, body: &mut String| {
            body.push(prefix);
            body.push_str(&line.text);
            body.push('\n');
            if !eol {
                body.push_str("\\ No newline at end of file\n");
            }
        };
        let (mut base_lines, mut target_lines) = (0, 0);
        for (i, op) in ops.iter().enumerate() {
            // A line keeps its "no newline" only while it is still the last one on that side.
            let target_eol = |line: &RawLine| Some(i) != last_target || line.eol;
            match op {
                Op::Context(line) => {
                    let base_eol = line.eol;
                    if base_eol == target_eol(line) {
                        push(' ', line, base_eol, &mut body);
                    } else {
                        push('-', line, base_eol, &mut body);
                        push('+', line, target_eol(line), &mut body);
                    }
                    base_lines += 1;
                    target_lines += 1;
                }
                Op::Del(line) => {
                    push('-', line, line.eol, &mut body);
                    base_lines += 1;
                }
                Op::Add(line) => {
                    push('+', line, target_eol(line), &mut body);
                    target_lines += 1;
                }
            }
        }
        debug_assert_eq!((base_lines, target_lines), (base_count, target_count));
        let range = |count: usize| {
            if count == 0 {
                "0,0".to_string()
            } else {
                format!("1,{count}")
            }
        };
        let mut patch = String::new();
        for line in &self.header {
            // A selected text range must not stage or undo an existing file's mode change.
            if line.starts_with("old mode ") || line.starts_with("new mode ") {
                continue;
            }
            match direction {
                // Undoing part of a new file edits the file that now exists.
                Direction::Undo if self.created && line.starts_with("new file mode") => continue,
                Direction::Undo if self.created && line == "--- /dev/null" => {
                    let plus = self
                        .header
                        .iter()
                        .find_map(|l| l.strip_prefix("+++ "))
                        .unwrap_or("b/");
                    let minus = plus.replacen("b/", "a/", 1);
                    patch.push_str(&format!("--- {minus}\n"));
                }
                _ => {
                    patch.push_str(line);
                    patch.push('\n');
                }
            }
        }
        let base_count = if direction == Direction::Apply && self.created {
            0
        } else {
            base_count
        };
        patch.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(base_count),
            range(target_count)
        ));
        patch.push_str(&body);
        Some(patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Command};

    // These pure fixtures pair replacement positions directly. UI integration tests supply
    // DiffDoc's similarity alignment to select_aligned instead.
    impl RawPatch {
        fn select(
            &self,
            selected: &dyn Fn(&RawLine) -> bool,
            direction: Direction,
        ) -> Option<String> {
            let mut alignment = Vec::new();
            for block in self.lines.chunk_by(|a, b| a.kind != b' ' && b.kind != b' ') {
                if block[0].kind == b' ' {
                    alignment.push((block[0].old, block[0].new));
                    continue;
                }
                let split = block.partition_point(|line| line.kind == b'-');
                let (old, new) = block.split_at(split);
                for index in 0..old.len().max(new.len()) {
                    alignment.push((
                        old.get(index).and_then(|l| l.old),
                        new.get(index).and_then(|l| l.new),
                    ));
                }
            }
            self.select_aligned(selected, direction, alignment)
        }
    }

    fn git(root: &Path, args: &[&str], input: Option<&str>) -> String {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}\n{}",
            String::from_utf8_lossy(&out.stderr),
            input.unwrap_or_default()
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("zj-partial-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"], None);
        git(&root, &["config", "user.email", "t@e"], None);
        git(&root, &["config", "user.name", "t"], None);
        git(&root, &["config", "core.autocrlf", "false"], None);
        root
    }

    fn diff(root: &Path, cached: bool) -> RawPatch {
        let mut args = vec!["diff", "--no-color", "--unified=1000000"];
        if cached {
            args.push("--cached");
        }
        parse(&git(root, &args, None)).unwrap()
    }

    /// Stage one of two changes, then unstage it, then revert the other in the worktree.
    #[test]
    fn stage_unstage_and_revert_selected_lines() {
        let root = repo("basic");
        fs::write(root.join("f.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        git(&root, &["add", "."], None);
        git(&root, &["commit", "-qm", "init"], None);
        fs::write(root.join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

        let patch = diff(&root, false);
        let stage = patch
            .select(&|l| l.new == Some(2) || l.old == Some(2), Direction::Apply)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&stage));
        assert_eq!(
            git(&root, &["show", ":f.txt"], None),
            "one\nTWO\nthree\nfour\n"
        );

        let staged = diff(&root, true);
        let unstage = staged.select(&|_| true, Direction::Undo).unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&unstage));
        assert_eq!(
            git(&root, &["show", ":f.txt"], None),
            "one\ntwo\nthree\nfour\n"
        );

        let patch = diff(&root, false);
        let revert = patch
            .select(&|l| l.new == Some(5), Direction::Undo)
            .unwrap();
        git(&root, &["apply", "-"], Some(&revert));
        assert_eq!(
            fs::read_to_string(root.join("f.txt")).unwrap(),
            "one\nTWO\nthree\nfour\n"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    /// The old file has no final newline; stage only a line added after it.
    #[test]
    fn missing_newline_at_end_of_file() {
        let root = repo("eol");
        fs::write(root.join("f.txt"), "a\nb").unwrap();
        git(&root, &["add", "."], None);
        git(&root, &["commit", "-qm", "init"], None);
        fs::write(root.join("f.txt"), "A\nb\nc\n").unwrap();
        let patch = diff(&root, false);
        let stage = patch
            .select(&|l| l.new == Some(3), Direction::Apply)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&stage));
        assert_eq!(git(&root, &["show", ":f.txt"], None), "a\nb\nc\n");
        // Revert only the first-line edit in the worktree; the end stays as it is.
        let patch = diff(&root, false);
        let revert = patch
            .select(&|l| l.old == Some(1) || l.new == Some(1), Direction::Undo)
            .unwrap();
        git(&root, &["apply", "-"], Some(&revert));
        assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), "a\nb\nc\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn crlf_lines_are_kept_byte_for_byte() {
        let root = repo("crlf");
        fs::write(root.join("w.txt"), "x\r\ny\r\n").unwrap();
        git(&root, &["add", "."], None);
        git(&root, &["commit", "-qm", "init"], None);
        fs::write(root.join("w.txt"), "x\r\nY\r\nz\r\n").unwrap();
        let patch = diff(&root, false);
        let stage = patch
            .select(&|l| l.new == Some(3), Direction::Apply)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&stage));
        assert_eq!(git(&root, &["show", ":w.txt"], None), "x\r\ny\r\nz\r\n");
        fs::remove_dir_all(&root).unwrap();
    }

    /// Part of a new (intent-to-add) file is staged; later part of it is unstaged again.
    #[test]
    fn new_files_stage_and_unstage_in_parts() {
        let root = repo("new");
        fs::write(root.join("keep.txt"), "k\n").unwrap();
        git(&root, &["add", "."], None);
        git(&root, &["commit", "-qm", "init"], None);
        fs::write(root.join("new.txt"), "1\n2\n3\n").unwrap();
        // `git diff --no-index` exits 1 when the files differ, so run it directly.
        let output = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "diff",
                "--no-index",
                "--no-color",
                "--unified=1000000",
                "--",
                "/dev/null",
                "new.txt",
            ])
            .output()
            .unwrap();
        let patch = parse(&String::from_utf8(output.stdout).unwrap()).unwrap();
        assert!(patch.created);
        let stage = patch
            .select(&|l| l.new == Some(1) || l.new == Some(3), Direction::Apply)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&stage));
        assert_eq!(git(&root, &["show", ":new.txt"], None), "1\n3\n");
        let staged = diff(&root, true);
        let unstage = staged
            .select(&|l| l.new == Some(2), Direction::Undo)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&unstage));
        assert_eq!(git(&root, &["show", ":new.txt"], None), "1\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn adjacent_replacements_keep_selected_lines_in_place() {
        for selected_line in [1, 2] {
            let root = repo(&format!("adjacent-{selected_line}"));
            fs::write(root.join("f.txt"), "one\ntwo\n").unwrap();
            git(&root, &["add", "."], None);
            git(&root, &["commit", "-qm", "init"], None);
            fs::write(root.join("f.txt"), "ONE\nTWO\n").unwrap();
            let selected =
                |line: &RawLine| line.old == Some(selected_line) || line.new == Some(selected_line);
            let stage = diff(&root, false)
                .select(&selected, Direction::Apply)
                .unwrap();
            git(&root, &["apply", "--cached", "-"], Some(&stage));
            let staged = if selected_line == 1 {
                "ONE\ntwo\n"
            } else {
                "one\nTWO\n"
            };
            let undone = if selected_line == 1 {
                "one\nTWO\n"
            } else {
                "ONE\ntwo\n"
            };
            assert_eq!(git(&root, &["show", ":f.txt"], None), staged);
            git(&root, &["add", "f.txt"], None);
            let unstage = diff(&root, true)
                .select(&selected, Direction::Undo)
                .unwrap();
            git(&root, &["apply", "--cached", "-"], Some(&unstage));
            assert_eq!(git(&root, &["show", ":f.txt"], None), undone);
            git(&root, &["restore", "--staged", "f.txt"], None);
            let revert = diff(&root, false)
                .select(&selected, Direction::Undo)
                .unwrap();
            git(&root, &["apply", "-"], Some(&revert));
            assert_eq!(fs::read_to_string(root.join("f.txt")).unwrap(), undone);
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn selected_text_keeps_existing_index_and_worktree_modes() {
        use std::os::unix::fs::PermissionsExt;

        for (base_mode, changed_mode) in [(0o644, 0o755), (0o755, 0o644)] {
            let root = repo(&format!("mode-{base_mode}"));
            let path = root.join("f.txt");
            git(&root, &["config", "core.filemode", "true"], None);
            fs::write(&path, "one\nkeep\ntwo\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(base_mode)).unwrap();
            git(&root, &["add", "."], None);
            git(&root, &["commit", "-qm", "init"], None);
            fs::write(&path, "ONE\nkeep\nTWO\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(changed_mode)).unwrap();
            let index_mode = || {
                git(&root, &["ls-files", "--stage", "f.txt"], None)
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_string()
            };
            let mode = |value| format!("100{value:o}");
            let first = |line: &RawLine| line.old == Some(1) || line.new == Some(1);
            let stage = diff(&root, false).select(&first, Direction::Apply).unwrap();
            git(&root, &["apply", "--cached", "-"], Some(&stage));
            assert_eq!(git(&root, &["show", ":f.txt"], None), "ONE\nkeep\ntwo\n");
            assert_eq!(index_mode(), mode(base_mode));
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                changed_mode
            );

            // Stage the mode too, then undo only the first text change.
            git(&root, &["add", "f.txt"], None);
            let unstage = diff(&root, true).select(&first, Direction::Undo).unwrap();
            git(&root, &["apply", "--cached", "-"], Some(&unstage));
            assert_eq!(git(&root, &["show", ":f.txt"], None), "one\nkeep\nTWO\n");
            assert_eq!(index_mode(), mode(changed_mode));

            // Restore the index baseline so reverting the text also sees a mode difference.
            git(&root, &["restore", "--staged", "f.txt"], None);
            let revert = diff(&root, false).select(&first, Direction::Undo).unwrap();
            git(&root, &["apply", "-"], Some(&revert));
            assert_eq!(fs::read_to_string(&path).unwrap(), "one\nkeep\nTWO\n");
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                changed_mode
            );
            assert_eq!(index_mode(), mode(base_mode));
            fs::remove_dir_all(&root).unwrap();
        }
    }

    /// The creation mode is needed when staging part of a new executable file.
    #[test]
    fn new_executable_keeps_its_mode_when_staged_and_unstaged_in_parts() {
        use std::os::unix::fs::PermissionsExt;

        let root = repo("new-mode");
        git(&root, &["config", "core.filemode", "true"], None);
        git(&root, &["commit", "--allow-empty", "-qm", "init"], None);
        let path = root.join("script.sh");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        git(&root, &["add", "-N", "script.sh"], None);
        let stage = diff(&root, false)
            .select(&|line| matches!(line.new, Some(1 | 3)), Direction::Apply)
            .unwrap();
        assert!(stage.contains("new file mode 100755"));
        git(&root, &["apply", "--cached", "-"], Some(&stage));
        assert_eq!(git(&root, &["show", ":script.sh"], None), "one\nthree\n");
        let unstage = diff(&root, true)
            .select(&|line| line.new == Some(2), Direction::Undo)
            .unwrap();
        git(&root, &["apply", "--cached", "-"], Some(&unstage));
        assert_eq!(git(&root, &["show", ":script.sh"], None), "one\n");
        assert!(git(&root, &["ls-files", "--stage", "script.sh"], None).starts_with("100755 "));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn nothing_selected_or_renames_give_no_patch() {
        let patch = parse("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\r\n+b\n").unwrap();
        assert_eq!(
            (patch.text(true, 1), patch.text(false, 1)),
            (Some("a"), Some("b"))
        );
        assert_eq!(patch.text(false, 2), None);
        assert!(patch.select(&|_| false, Direction::Apply).is_none());
        let renamed = parse("diff --git a/x b/y\n--- a/x\n+++ b/y\n@@ -1 +1 @@\n-a\n+b\n").unwrap();
        assert!(renamed.select(&|_| true, Direction::Apply).is_none());
    }
}
