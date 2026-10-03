//! Parser for `git log -z --format=%h%x00%at%x00%s`.
use std::io;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    /// Abbreviated object name.
    pub short: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub subject: String,
}

/// Every commit is three NUL-separated fields ending with a NUL; a subject may be empty.
pub fn parse_log(bytes: &[u8]) -> io::Result<Vec<Commit>> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "无效的 git log 输出");
    let Some(body) = bytes.strip_suffix(b"\0") else {
        return if bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Err(malformed())
        };
    };
    let fields: Vec<&[u8]> = body.split(|b| *b == 0).collect();
    let (commits, rest) = fields.as_chunks::<3>();
    if !rest.is_empty() {
        return Err(malformed());
    }
    commits
        .iter()
        .map(|commit| {
            let short = std::str::from_utf8(commit[0]).map_err(|_| malformed())?;
            if short.is_empty() || !short.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(malformed());
            }
            let time = std::str::from_utf8(commit[1])
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(malformed)?;
            Ok(Commit {
                short: short.to_string(),
                time,
                subject: String::from_utf8_lossy(commit[2]).into_owned(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_with_empty_and_non_utf8_subjects() {
        let raw = b"141a0b8\x001790994928\x00third \xe4\xb8\xad\x00\
                    5a61849\x001790994927\x00\x00\
                    6248dc0\x001790994926\x00bad \xff\x00";
        let commits = parse_log(raw).unwrap();
        assert_eq!(
            commits,
            vec![
                Commit {
                    short: "141a0b8".into(),
                    time: 1_790_994_928,
                    subject: "third 中".into(),
                },
                Commit {
                    short: "5a61849".into(),
                    time: 1_790_994_927,
                    subject: String::new(),
                },
                Commit {
                    short: "6248dc0".into(),
                    time: 1_790_994_926,
                    subject: "bad \u{fffd}".into(),
                },
            ]
        );
        assert_eq!(parse_log(b"").unwrap(), Vec::new());
    }

    #[test]
    fn truncated_or_garbled_output_is_an_error() {
        assert!(parse_log(b"141a0b8\x001790994928\x00subject").is_err());
        assert!(parse_log(b"141a0b8\x001790994928\x00").is_err());
        assert!(parse_log(b"zzz\x001790994928\x00s\x00").is_err());
        assert!(parse_log(b"141a0b8\x00soon\x00s\x00").is_err());
    }
}
