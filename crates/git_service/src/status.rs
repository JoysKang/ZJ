use std::{ffi::OsString, io, os::unix::ffi::OsStringExt, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Tracked,
    Renamed,
    Untracked,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    pub path: PathBuf,
    pub original_path: Option<PathBuf>,
    pub index: u8,
    pub worktree: u8,
    pub kind: ChangeKind,
    pub submodule: [u8; 4],
}

impl Change {
    /// HEAD↔index rename operations need both paths; a local edit after a staged
    /// rename uses the new index path only.
    pub fn paths(&self, side: super::DiffSide) -> Vec<PathBuf> {
        let mut paths = vec![self.path.clone()];
        if (matches!(side, super::DiffSide::Staged) || self.worktree == b'R')
            && let Some(old) = &self.original_path
        {
            paths.push(old.clone());
        }
        paths
    }
    pub fn staged(&self) -> bool {
        self.index != b'.' && self.kind != ChangeKind::Untracked
    }
    pub fn unstaged(&self) -> bool {
        self.worktree != b'.' || self.kind == ChangeKind::Untracked
    }
    /// A file with no content in the index: untracked, or added with `git add -N` (whose
    /// index entry is empty). Discarding it deletes it; restoring would empty it.
    pub fn new_in_worktree(&self) -> bool {
        self.kind == ChangeKind::Untracked || (self.index == b'.' && self.worktree == b'A')
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    pub branch: Option<String>,
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    /// Filesystem snapshot attached by GitService, never inferred from display strings.
    pub version: u64,
    pub changes: Vec<Change>,
}

fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "无效的 porcelain v2 记录")
}
fn path(bytes: &[u8]) -> io::Result<PathBuf> {
    if bytes.is_empty() {
        return Err(malformed());
    }
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

pub fn parse_status(bytes: &[u8]) -> io::Result<Status> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(malformed());
    }
    let mut status = Status::default();
    let mut records = bytes.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.head ") {
            status.branch = Some(String::from_utf8_lossy(value).into_owned());
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            status.oid = Some(String::from_utf8_lossy(value).into_owned());
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.upstream ") {
            status.upstream = Some(String::from_utf8_lossy(value).into_owned());
            continue;
        }
        if let Some(value) = record.strip_prefix(b"# branch.ab ") {
            let value = std::str::from_utf8(value).map_err(|_| malformed())?;
            let (ahead, behind) = value.split_once(' ').ok_or_else(malformed)?;
            status.ahead = ahead
                .strip_prefix('+')
                .ok_or_else(malformed)?
                .parse()
                .map_err(|_| malformed())?;
            status.behind = behind
                .strip_prefix('-')
                .ok_or_else(malformed)?
                .parse()
                .map_err(|_| malformed())?;
            continue;
        }
        if record.starts_with(b"# ") || record.starts_with(b"! ") {
            continue;
        }
        if let Some(value) = record.strip_prefix(b"? ") {
            status.changes.push(Change {
                path: path(value)?,
                original_path: None,
                index: b'.',
                worktree: b'?',
                kind: ChangeKind::Untracked,
                submodule: *b"N...",
            });
            continue;
        }
        let (count, kind) = match record[0] {
            b'1' => (9, ChangeKind::Tracked),
            b'2' => (10, ChangeKind::Renamed),
            b'u' => (11, ChangeKind::Conflict),
            _ => return Err(malformed()),
        };
        let fields: Vec<_> = record.splitn(count, |b| *b == b' ').collect();
        if fields.len() != count
            || fields[1].len() != 2
            || fields[2].len() != 4
            || fields[..count - 1].iter().any(|f| f.is_empty())
        {
            return Err(malformed());
        }
        if !fields[1].iter().all(|b| b".MADRCUT".contains(b)) {
            return Err(malformed());
        }
        let original_path = if kind == ChangeKind::Renamed {
            Some(path(records.next().ok_or_else(malformed)?)?)
        } else {
            None
        };
        status.changes.push(Change {
            path: path(fields[count - 1])?,
            original_path,
            index: fields[1][0],
            worktree: fields[1][1],
            kind,
            submodule: fields[2].try_into().unwrap(),
        });
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    #[test]
    fn preserves_raw_paths_and_dual_state() {
        let raw = b"# branch.head main\0# branch.oid (initial)\0\
1 MM N... 100644 100644 100644 abc def src/a b.rs\0\
2 R. N... 100644 100644 100644 abc def R100 new\nname\0old name\0\
? bad\xff\0\
u UU N... 100644 100644 100644 100644 abc def ghi conflict\0";
        let s = parse_status(raw).unwrap();
        assert_eq!(s.changes.len(), 4);
        assert!(s.changes[0].staged() && s.changes[0].unstaged());
        assert_eq!(
            s.changes[1]
                .original_path
                .as_ref()
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"old name"
        );
        assert_eq!(s.changes[2].path.as_os_str().as_bytes(), b"bad\xff");
        assert_eq!(s.changes[3].kind, ChangeKind::Conflict);
    }
    #[test]
    fn malformed_output_never_becomes_clean() {
        for input in [
            b"? truncated".as_slice(),
            b"2 R.\0",
            b"1 ZZ N... a b c d e f\0",
            b"x ignored\0",
        ] {
            assert!(parse_status(input).is_err());
        }
        assert!(
            parse_status(b"# branch.head (detached)\0")
                .unwrap()
                .changes
                .is_empty()
        );
    }
}
