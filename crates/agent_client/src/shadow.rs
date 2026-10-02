//! Shadow buffers for [`WriteMode::AcceptFirst`](crate::WriteMode::AcceptFirst): the agent's
//! `fs/write_text_file` lands here; disk changes only when the user accepts a file or a
//! selection of hunks.
//!
//! Limitation: only writes that go through ACP are intercepted. Shell commands the agent runs
//! itself (`sed -i`, `cargo fmt`, code generators) read and write the real disk.

use crate::fs::write_atomic;
use std::{
    collections::BTreeMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingEdit {
    pub path: PathBuf,
    /// Disk content when the agent first wrote the file; `None` for a new file.
    pub base: Option<String>,
    /// The agent's latest version.
    pub proposed: String,
}

/// One changed region, in line indices (0-based, end-exclusive) of base and proposed text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub base: Range<usize>,
    pub proposed: Range<usize>,
}

#[derive(Default)]
pub struct ShadowStore {
    edits: Mutex<BTreeMap<PathBuf, PendingEdit>>,
}

fn conflict(path: &Path) -> io::Error {
    io::Error::other(format!(
        "{} 在 Agent 提出修改后又被改动过；请重新查看差异，或放弃这次修改",
        path.display()
    ))
}

fn not_pending(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{} 没有待接受的修改", path.display()),
    )
}

impl ShadowStore {
    /// Records the agent's write. The first write captures the disk content as the base.
    pub(crate) fn propose(&self, path: &Path, text: String) -> io::Result<()> {
        let mut edits = self.edits.lock().unwrap();
        if let Some(edit) = edits.get_mut(path) {
            edit.proposed = text;
            return Ok(());
        }
        let base = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        edits.insert(
            path.to_path_buf(),
            PendingEdit {
                path: path.to_path_buf(),
                base,
                proposed: text,
            },
        );
        Ok(())
    }

    /// The agent reads back what it wrote, not the untouched disk file.
    pub(crate) fn proposed_text(&self, path: &Path) -> Option<String> {
        self.edits
            .lock()
            .unwrap()
            .get(path)
            .map(|e| e.proposed.clone())
    }

    pub fn pending_paths(&self) -> Vec<PathBuf> {
        self.edits.lock().unwrap().keys().cloned().collect()
    }

    pub fn get(&self, path: &Path) -> Option<PendingEdit> {
        self.edits.lock().unwrap().get(path).cloned()
    }

    pub fn hunks(&self, path: &Path) -> Vec<Hunk> {
        match self.get(path) {
            Some(edit) => diff_hunks(edit.base.as_deref().unwrap_or(""), &edit.proposed),
            None => Vec::new(),
        }
    }

    /// Writes the whole proposal and returns the text now on disk.
    pub fn accept_file(&self, path: &Path) -> io::Result<String> {
        self.resolve(path, |edit| Some(edit.proposed.clone()))
    }

    /// Drops the proposal; disk is untouched.
    pub fn reject_file(&self, path: &Path) -> bool {
        self.edits.lock().unwrap().remove(path).is_some()
    }

    /// Applies only the hunks whose indices are in `accepted` (the rest keep the base text),
    /// writes the result and returns it. A new file is not created when nothing is accepted.
    pub fn accept_hunks(&self, path: &Path, accepted: &[usize]) -> io::Result<String> {
        self.resolve(path, |edit| {
            let base = edit.base.as_deref().unwrap_or("");
            if edit.base.is_none() && accepted.is_empty() {
                return None;
            }
            Some(apply_hunks(base, &edit.proposed, accepted))
        })
    }

    fn resolve(
        &self,
        path: &Path,
        decide: impl FnOnce(&PendingEdit) -> Option<String>,
    ) -> io::Result<String> {
        let mut edits = self.edits.lock().unwrap();
        let edit = edits.get(path).ok_or_else(|| not_pending(path))?;
        let current = match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if current != edit.base {
            return Err(conflict(path));
        }
        let result = decide(edit);
        if let Some(text) = &result
            && Some(text) != edit.base.as_ref()
        {
            write_atomic(path, text)?;
        }
        let final_text = result.or_else(|| edit.base.clone()).unwrap_or_default();
        edits.remove(path);
        Ok(final_text)
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// Rebuilds the file from `base`, taking the proposed lines for accepted hunks.
pub fn apply_hunks(base: &str, proposed: &str, accepted: &[usize]) -> String {
    let a = lines(base);
    let b = lines(proposed);
    let mut out = String::with_capacity(proposed.len().max(base.len()));
    let mut at = 0;
    for (index, hunk) in diff_lines(&a, &b).into_iter().enumerate() {
        for line in &a[at..hunk.base.start] {
            out.push_str(line);
        }
        if accepted.contains(&index) {
            for line in &b[hunk.proposed.clone()] {
                out.push_str(line);
            }
        } else {
            for line in &a[hunk.base.clone()] {
                out.push_str(line);
            }
        }
        at = hunk.base.end;
    }
    for line in &a[at..] {
        out.push_str(line);
    }
    out
}

pub fn diff_hunks(base: &str, proposed: &str) -> Vec<Hunk> {
    diff_lines(&lines(base), &lines(proposed))
}

/// Edit distance beyond which the middle is reported as one replaced block (keeps the trace
/// under ~8 MB).
const MAX_EDITS: usize = 1000;

/// Myers' O((N+M)·D) line diff after trimming the common prefix and suffix.
fn diff_lines(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    if a_mid.is_empty() && b_mid.is_empty() {
        return Vec::new();
    }
    let mut hunks = match myers(a_mid, b_mid) {
        Some(edits) => group(&edits),
        None => vec![Hunk {
            base: 0..a_mid.len(),
            proposed: 0..b_mid.len(),
        }],
    };
    for hunk in &mut hunks {
        hunk.base = hunk.base.start + prefix..hunk.base.end + prefix;
        hunk.proposed = hunk.proposed.start + prefix..hunk.proposed.end + prefix;
    }
    hunks
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Equal,
    Delete,
    Insert,
}

fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Edit>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // trace[d] = v[k] for k in -d..=d before step d.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=max.min(MAX_EDITS) as isize {
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let idx = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Edit> {
    let mut edits = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let get = |k: isize| v[(k + d) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && get(k - 1) < get(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = if d == 0 { 0 } else { get(prev_k) };
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            edits.push(Edit::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            edits.push(if x == prev_x {
                Edit::Insert
            } else {
                Edit::Delete
            });
            x = prev_x;
            y = prev_y;
        }
    }
    edits.reverse();
    edits
}

fn group(edits: &[Edit]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut current: Option<Hunk> = None;
    for edit in edits {
        match edit {
            Edit::Equal => {
                if let Some(h) = current.take() {
                    hunks.push(h);
                }
                i += 1;
                j += 1;
            }
            Edit::Delete => {
                let h = current.get_or_insert(Hunk {
                    base: i..i,
                    proposed: j..j,
                });
                i += 1;
                h.base.end = i;
            }
            Edit::Insert => {
                let h = current.get_or_insert(Hunk {
                    base: i..i,
                    proposed: j..j,
                });
                j += 1;
                h.proposed.end = j;
            }
        }
    }
    if let Some(h) = current {
        hunks.push(h);
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunks_and_partial_application() {
        let base = "a\nb\nc\nd\ne\n";
        let proposed = "a\nB\nc\nd\ne\nf\n";
        let hunks = diff_hunks(base, proposed);
        assert_eq!(
            hunks,
            vec![
                Hunk {
                    base: 1..2,
                    proposed: 1..2
                },
                Hunk {
                    base: 5..5,
                    proposed: 5..6
                }
            ]
        );
        assert_eq!(apply_hunks(base, proposed, &[0, 1]), proposed);
        assert_eq!(apply_hunks(base, proposed, &[]), base);
        assert_eq!(apply_hunks(base, proposed, &[0]), "a\nB\nc\nd\ne\n");
        assert_eq!(apply_hunks(base, proposed, &[1]), "a\nb\nc\nd\ne\nf\n");
    }

    #[test]
    fn diff_matches_random_edits() {
        // Applying every hunk must reproduce the target exactly, for many shapes.
        let mut seed = 42u64;
        let mut rand = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 33) as usize
        };
        for _ in 0..300 {
            let a: Vec<String> = (0..rand() % 30)
                .map(|_| format!("{}\n", rand() % 5))
                .collect();
            let b: Vec<String> = (0..rand() % 30)
                .map(|_| format!("{}\n", rand() % 5))
                .collect();
            let (a, b) = (a.concat(), b.concat());
            let hunks = diff_hunks(&a, &b);
            let all: Vec<usize> = (0..hunks.len()).collect();
            assert_eq!(apply_hunks(&a, &b, &all), b);
            assert_eq!(apply_hunks(&a, &b, &[]), a);
        }
    }

    #[test]
    fn missing_trailing_newline_is_a_change() {
        let hunks = diff_hunks("x\ny", "x\ny\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(apply_hunks("x\ny", "x\ny\n", &[0]), "x\ny\n");
    }
}
