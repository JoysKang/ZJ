//! Stashes (`git stash list`) and the blame of one line (`git blame --porcelain`).
use std::io;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stash {
    /// `n` of `stash@{n}`.
    pub index: usize,
    /// The stash commit; a write checks `stash@{n}` still names it.
    pub oid: String,
    /// Unix seconds.
    pub time: i64,
    /// `On main: message` or `WIP on main: abc1234 subject`.
    pub message: String,
}

/// `--format` of [`parse_stashes`]: four NUL-terminated fields per entry.
pub(crate) const STASH_FORMAT: &str = "--format=%gd%x00%H%x00%ct%x00%gs%x00";

pub fn parse_stashes(bytes: &[u8]) -> io::Result<Vec<Stash>> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "无效的 stash 列表");
    let fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let (records, rest) = fields.as_chunks::<4>();
    if !matches!(rest, [] | [b""] | [b"\n"]) {
        return Err(malformed());
    }
    let text = |field: &[u8]| String::from_utf8_lossy(field).into_owned();
    records
        .iter()
        .map(|[name, oid, time, message]| {
            let name = name.strip_prefix(b"\n").unwrap_or(name);
            let index = std::str::from_utf8(name)
                .ok()
                .and_then(|n| n.strip_prefix("stash@{")?.strip_suffix('}')?.parse().ok())
                .ok_or_else(malformed)?;
            Ok(Stash {
                index,
                oid: text(oid),
                time: std::str::from_utf8(time)
                    .ok()
                    .and_then(|t| t.parse().ok())
                    .ok_or_else(malformed)?,
                message: text(message),
            })
        })
        .collect()
}

/// Who last changed a line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Blame {
    pub commit: String,
    pub author: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub summary: String,
    /// The line is not committed yet (edited in the buffer or the worktree).
    pub uncommitted: bool,
}

/// The first entry of `git blame --porcelain` output.
pub fn parse_blame(bytes: &[u8]) -> io::Result<Blame> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "无效的 blame 输出");
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let commit = lines
        .next()
        .and_then(|line| line.split(' ').next())
        .filter(|hash| hash.len() >= 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(malformed)?
        .to_string();
    let (mut author, mut time, mut summary) = (String::new(), 0, String::new());
    for line in lines {
        if line.starts_with('\t') {
            break;
        } else if let Some(value) = line.strip_prefix("author ") {
            author = value.to_string();
        } else if let Some(value) = line.strip_prefix("author-time ") {
            time = value.parse().map_err(|_| malformed())?;
        } else if let Some(value) = line.strip_prefix("summary ") {
            summary = value.to_string();
        }
    }
    Ok(Blame {
        uncommitted: commit.bytes().all(|b| b == b'0'),
        commit,
        author,
        time,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stash_lists_and_rejects_garbage() {
        let out = b"stash@{0}\0aaaa\x001700000000\0On main: wip\0\nstash@{1}\0bbbb\x001600000000\0WIP on main: 1234567 fix\0\n";
        let stashes = parse_stashes(out).unwrap();
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[1].index, 1);
        assert_eq!(stashes[1].oid, "bbbb");
        assert_eq!(stashes[0].message, "On main: wip");
        assert!(parse_stashes(b"").unwrap().is_empty());
        assert!(parse_stashes(b"stash@{x}\0a\x001\0m\0").is_err());
    }

    #[test]
    fn parses_blame_porcelain() {
        let hash = "1".repeat(40);
        let out = format!(
            "{hash} 3 3 1\nauthor 张三\nauthor-mail <z@example.com>\nauthor-time 1700000000\nauthor-tz +0800\nsummary 修复 bug\nfilename a.rs\n\tlet x = 1;\n"
        );
        let blame = parse_blame(out.as_bytes()).unwrap();
        assert_eq!(blame.author, "张三");
        assert_eq!(blame.time, 1_700_000_000);
        assert_eq!(blame.summary, "修复 bug");
        assert!(!blame.uncommitted);
        let zero = format!(
            "{} 1 1 1\nauthor Not Committed Yet\nauthor-time 1\nsummary x\n\tx\n",
            "0".repeat(40)
        );
        assert!(parse_blame(zero.as_bytes()).unwrap().uncommitted);
        assert!(parse_blame(b"fatal: no such path").is_err());
    }
}
