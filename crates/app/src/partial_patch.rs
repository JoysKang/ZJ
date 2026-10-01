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
    pub fn select(
        &self,
        selected: &dyn Fn(&RawLine) -> bool,
        direction: Direction,
    ) -> Option<String> {
        if self.whole_file_only {
            return None;
        }
        let mut ops = Vec::with_capacity(self.lines.len());
        let mut changes = 0;
        for line in &self.lines {
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
