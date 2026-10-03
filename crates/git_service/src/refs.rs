//! Parser for the branch listing (`git for-each-ref` over `refs/heads` and `refs/remotes`).
use std::io;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Branch {
    /// `main`, or `origin/main` for a remote-tracking branch.
    pub name: String,
    pub remote: bool,
    /// The checked-out branch.
    pub head: bool,
    pub short: String,
    /// `origin/main` for a local branch with an upstream.
    pub upstream: Option<String>,
    /// Committer time of the tip, Unix seconds.
    pub time: i64,
    pub subject: String,
}

/// `--format` of [`parse_branches`]: seven NUL-terminated fields per ref.
pub(crate) const BRANCH_FORMAT: &str = "--format=%(refname)%00%(HEAD)%00%(symref)%00\
    %(objectname:short)%00%(upstream:short)%00%(committerdate:unix)%00%(contents:subject)%00";

/// for-each-ref ends every record with a newline after the last NUL. Symbolic refs
/// (`origin/HEAD`) are left out.
pub fn parse_branches(bytes: &[u8]) -> io::Result<Vec<Branch>> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "无效的分支列表");
    let fields: Vec<&[u8]> = bytes.split(|b| *b == 0).collect();
    let (records, rest) = fields.as_chunks::<7>();
    if !matches!(rest, [] | [b""] | [b"\n"]) {
        return Err(malformed());
    }
    let text = |field: &[u8]| String::from_utf8_lossy(field).into_owned();
    let mut branches = Vec::new();
    for [refname, head, symref, short, upstream, time, subject] in records {
        let refname = refname.strip_prefix(b"\n").unwrap_or(refname);
        if !symref.is_empty() {
            continue;
        }
        let (name, remote) = if let Some(name) = refname.strip_prefix(b"refs/heads/") {
            (name, false)
        } else if let Some(name) = refname.strip_prefix(b"refs/remotes/") {
            (name, true)
        } else {
            continue;
        };
        branches.push(Branch {
            name: text(name),
            remote,
            head: *head == b"*",
            short: text(short),
            upstream: (!upstream.is_empty()).then(|| text(upstream)),
            time: std::str::from_utf8(time)
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(malformed)?,
            subject: text(subject),
        });
    }
    Ok(branches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_remote_and_symbolic_refs() {
        let raw = b"refs/heads/feat\0*\0\x0094265da\0origin/feat\x001790996063\0two\0\n\
                    refs/heads/main\0 \0\x00807157d\0\x001790996060\0one \xff\0\n\
                    refs/remotes/origin/HEAD\0 \0refs/remotes/origin/main\0807157d\0\x001790996060\0one\0\n\
                    refs/remotes/origin/main\0 \0\x00807157d\0\x001790996060\0one\0\n";
        let branches = parse_branches(raw).unwrap();
        let names: Vec<_> = branches
            .iter()
            .map(|b| (b.name.as_str(), b.remote, b.head, b.upstream.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("feat", false, true, Some("origin/feat")),
                ("main", false, false, None),
                ("origin/main", true, false, None),
            ]
        );
        assert_eq!(branches[1].subject, "one \u{fffd}");
        assert_eq!(branches[0].time, 1_790_996_063);
        assert_eq!(parse_branches(b"").unwrap(), Vec::new());
        assert!(parse_branches(b"refs/heads/x\0*\0").is_err());
    }
}
