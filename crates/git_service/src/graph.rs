//! Parsers for the commit graph (`git log`), a commit's details (`git show`) and its files
//! (`git diff --name-status`).
use std::{ffi::OsString, io, os::unix::ffi::OsStringExt, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefKind {
    /// A detached HEAD.
    Head,
    Branch,
    Remote,
    Tag,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphRef {
    pub kind: RefKind,
    /// `main`, `origin/main`, `v1.0` (empty for a detached HEAD).
    pub name: String,
    /// The checked-out branch (`HEAD -> main`).
    pub head: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, Unix seconds.
    pub time: i64,
    pub refs: Vec<GraphRef>,
    pub subject: String,
}

/// `--format` of [`parse_graph`] (with `-z --decorate=full`).
pub(crate) const GRAPH_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%D%x00%s";

fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "无效的 git log 输出")
}

fn text(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

fn time(field: &[u8]) -> io::Result<i64> {
    std::str::from_utf8(field)
        .ok()
        .and_then(|t| t.parse().ok())
        .ok_or_else(malformed)
}

pub(crate) fn is_hash(text: &str) -> bool {
    (4..=64).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `%D` with full names: `HEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main`.
/// Ref names cannot contain spaces, so `, ` separates them.
fn parse_refs(field: &[u8]) -> Vec<GraphRef> {
    let field = String::from_utf8_lossy(field);
    let mut refs = Vec::new();
    for item in field.split(", ").filter(|item| !item.is_empty()) {
        let (head, item) = match item.strip_prefix("HEAD -> ") {
            Some(rest) => (true, rest),
            None => (false, item),
        };
        let (kind, name) = if item == "HEAD" {
            (RefKind::Head, "")
        } else if let Some(name) = item.strip_prefix("tag: refs/tags/") {
            (RefKind::Tag, name)
        } else if let Some(name) = item.strip_prefix("refs/heads/") {
            (RefKind::Branch, name)
        } else if let Some(name) = item.strip_prefix("refs/remotes/") {
            if name.ends_with("/HEAD") {
                continue;
            }
            (RefKind::Remote, name)
        } else {
            continue;
        };
        refs.push(GraphRef {
            kind,
            name: name.to_string(),
            head,
        });
    }
    refs
}

/// Seven NUL-separated fields per commit, each commit ending with a NUL.
pub fn parse_graph(bytes: &[u8]) -> io::Result<Vec<GraphCommit>> {
    let Some(body) = bytes.strip_suffix(b"\0") else {
        return if bytes.is_empty() {
            Ok(Vec::new())
        } else {
            Err(malformed())
        };
    };
    let fields: Vec<&[u8]> = body.split(|b| *b == 0).collect();
    let (commits, rest) = fields.as_chunks::<7>();
    if !rest.is_empty() {
        return Err(malformed());
    }
    commits
        .iter()
        .map(|[hash, parents, author, email, at, refs, subject]| {
            let hash = text(hash);
            let parents: Vec<String> = text(parents)
                .split(' ')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            if !is_hash(&hash) || !parents.iter().all(|p| is_hash(p)) {
                return Err(malformed());
            }
            Ok(GraphCommit {
                hash,
                parents,
                author: text(author),
                email: text(email),
                time: time(at)?,
                refs: parse_refs(refs),
                subject: text(subject),
            })
        })
        .collect()
}

/// A tag's own message; lightweight tags never borrow the target's commit message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagDetails {
    pub name: String,
    pub oid: String,
    pub annotated: bool,
    pub tagger: String,
    pub tagger_email: String,
    pub tagger_date: String,
    /// Complete contents, including subject and any signature text (not verified).
    pub message: String,
    pub lossy: bool,
}

const TAG_LIMIT: usize = 256 * 1024;
const TAG_FIELDS: &str = "%(refname)%00%(objectname)%00%(objecttype)%00%(objectsize)%00%(taggername)%00%(taggeremail)%00%(taggerdate:iso-strict)%00";

impl crate::GitService {
    /// Reads one exact local tag, without fetching objects or invoking signature tools.
    pub fn tag_details(
        &self,
        repo: &workspace_editor_core::Repository,
        name: &str,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> io::Result<TagDetails> {
        let reference = format!("refs/tags/{name}");
        if name.is_empty() || name.contains('\0') {
            return Err(crate::error("无效的标签名"));
        }
        self.run(
            &repo.worktree,
            &["check-ref-format".into(), reference.clone().into()],
            cancel,
        )?;
        let query = |contents: bool| {
            self.run_bounded(
                &repo.worktree,
                &[
                    "--no-pager".into(),
                    "--no-lazy-fetch".into(),
                    "--no-replace-objects".into(),
                    "for-each-ref".into(),
                    "--count=1".into(),
                    format!(
                        "--format={TAG_FIELDS}{}",
                        if contents { "%(contents)" } else { "" }
                    )
                    .into(),
                    "--".into(),
                    reference.clone().into(),
                ],
                cancel,
                false,
                None,
                TAG_LIMIT + 4096,
            )
        };
        let metadata = query(false)?;
        let fields = tag_fields(&metadata, &reference)?;
        let oid = text(fields[1]);
        if !matches!(oid.len(), 40 | 64) || !is_hash(&oid) {
            return Err(crate::error("无效的标签对象"));
        }
        let annotated = fields[2] == b"tag";
        let mut details = TagDetails {
            name: name.into(),
            oid,
            annotated,
            tagger: text(fields[4]),
            tagger_email: text(fields[5]),
            tagger_date: text(fields[6]),
            message: String::new(),
            lossy: false,
        };
        if annotated {
            let size = std::str::from_utf8(fields[3])
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or_else(|| crate::error("无效的标签大小"))?;
            if size > TAG_LIMIT {
                return Err(crate::error("标签对象超过 256 KiB；无法完整显示说明"));
            }
            let contents = query(true)?;
            let fields = tag_fields(&contents, &reference)?;
            if fields[1] != details.oid.as_bytes() || fields[2] != b"tag" {
                return Err(crate::error("标签在读取期间已变化，请重新查看"));
            }
            if fields[7].len() > TAG_LIMIT {
                return Err(crate::error("标签说明超过 256 KiB；无法完整显示"));
            }
            details.lossy = std::str::from_utf8(fields[7]).is_err();
            details.message = text(fields[7]);
        }
        Ok(details)
    }
}

fn tag_fields<'a>(bytes: &'a [u8], reference: &str) -> io::Result<Vec<&'a [u8]>> {
    let fields: Vec<_> = bytes
        .strip_suffix(b"\n")
        .unwrap_or(bytes)
        .splitn(8, |b| *b == 0)
        .collect();
    if fields.len() != 8 || fields[0] != reference.as_bytes() {
        return Err(crate::error("标签不存在或已删除，请刷新 Git 图"));
    }
    Ok(fields)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitFile {
    /// `A`, `M`, `D`, `R`, `C`, `T`.
    pub status: char,
    pub path: PathBuf,
    /// The source of a rename or copy.
    pub original_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitDetails {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    /// The whole message.
    pub message: String,
    /// Changes against the first parent (everything for a root commit).
    pub files: Vec<CommitFile>,
}

pub(crate) const DETAILS_FORMAT: &str =
    "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B";

/// `git show -s` output; the message is the last field and may contain anything but NUL.
pub(crate) fn parse_details(bytes: &[u8]) -> io::Result<CommitDetails> {
    let fields: Vec<&[u8]> = bytes.splitn(9, |b| *b == 0).collect();
    let [
        hash,
        parents,
        author,
        email,
        at,
        committer,
        committer_email,
        ct,
        message,
    ] = fields.as_slice()
    else {
        return Err(malformed());
    };
    let hash = text(hash);
    if !is_hash(&hash) {
        return Err(malformed());
    }
    Ok(CommitDetails {
        hash,
        parents: text(parents)
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
        author: text(author),
        email: text(email),
        author_time: time(at)?,
        committer: text(committer),
        committer_email: text(committer_email),
        commit_time: time(ct)?,
        message: text(message).trim_end().to_string(),
        files: Vec::new(),
    })
}

/// `git diff --name-status -z`: `M\0path\0`, or `R100\0old\0new\0` for renames and copies.
pub fn parse_name_status(bytes: &[u8]) -> io::Result<Vec<CommitFile>> {
    let mut fields = bytes.split(|b| *b == 0).filter(|f| !f.is_empty());
    let path = |field: Option<&[u8]>| {
        field
            .map(|f| PathBuf::from(OsString::from_vec(f.to_vec())))
            .ok_or_else(malformed)
    };
    let mut files = Vec::new();
    while let Some(code) = fields.next() {
        let status = *code.first().ok_or_else(malformed)? as char;
        if !status.is_ascii_uppercase() {
            return Err(malformed());
        }
        let (path, original_path) = if matches!(status, 'R' | 'C') {
            let original = path(fields.next())?;
            (path(fields.next())?, Some(original))
        } else {
            (path(fields.next())?, None)
        };
        files.push(CommitFile {
            status,
            path,
            original_path,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "94265da7d87c66a16829018bc6f3b7217e992dc6";
    const B: &str = "807157d2d71753348cc74c6bcf1111115aacf68a";

    #[test]
    fn commits_parents_and_decorations() {
        let raw = format!(
            "{A}\0{B}\0Ann\0ann@example.invalid\x001790996063\0HEAD -> refs/heads/feat, \
             refs/remotes/origin/feat, refs/remotes/origin/HEAD, refs/stash\0two\0\
             {B}\0\0Bob\0bob@example.invalid\x001790996060\0tag: refs/tags/v1, refs/heads/main\0\0"
        );
        let commits = parse_graph(raw.as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, [B]);
        assert_eq!(
            commits[0].refs,
            [
                GraphRef {
                    kind: RefKind::Branch,
                    name: "feat".into(),
                    head: true
                },
                GraphRef {
                    kind: RefKind::Remote,
                    name: "origin/feat".into(),
                    head: false
                },
            ]
        );
        assert!(commits[1].parents.is_empty());
        assert_eq!(commits[1].subject, "");
        assert_eq!(
            commits[1].refs.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [RefKind::Tag, RefKind::Branch]
        );
        assert_eq!(parse_refs(b"HEAD")[0].kind, RefKind::Head);
        assert!(parse_graph(b"").unwrap().is_empty());
        assert!(parse_graph(format!("{A}\0\0a\0e\x001\0\0").as_bytes()).is_err());
        assert!(parse_graph(b"zz\0\0a\0e\x001\0\0s\0").is_err());
    }

    #[test]
    fn tag_details_read_only_fixture() {
        use std::{
            fs,
            process::Command,
            sync::atomic::AtomicBool,
            time::{Duration, SystemTime, UNIX_EPOCH},
        };
        let root = std::env::temp_dir().join(format!(
            "zj-tag-details-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "user.name=Tag Fixture",
                    "-c",
                    "user.email=tag@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "tag.gpgSign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "提交正文不是标签说明",
        ]);
        let service = crate::GitService::new(1, Duration::from_secs(5)).unwrap();
        let cancel = AtomicBool::new(false);
        let repo = service.identify(&root, &cancel).unwrap();
        for (name, message) in [
            ("multi", "发布说明\n\n中文第二段\n第三行"),
            ("single", "只有一个段落"),
            (
                "signature",
                "发布摘要\n\n-----BEGIN PGP SIGNATURE-----\nfixture only\n-----END PGP SIGNATURE-----",
            ),
        ] {
            git(&["tag", "-a", name, "-m", message]);
            let details = service.tag_details(&repo, name, &cancel).unwrap();
            assert!(details.annotated);
            assert_eq!(details.message, format!("{message}\n"));
            assert_eq!(details.tagger, "Tag Fixture");
            assert!(!details.tagger_date.is_empty());
        }
        git(&["tag", "light"]);
        let details = service.tag_details(&repo, "light", &cancel).unwrap();
        assert!(!details.annotated);
        assert!(details.message.is_empty());
        git(&["tag", "-d", "single"]);
        git(&["tag", "single/child"]);
        assert!(
            service
                .tag_details(&repo, "single", &cancel)
                .unwrap_err()
                .to_string()
                .contains("不存在")
        );
        for invalid in ["../multi", "multi*", "multi^{commit}", "multi\0"] {
            assert!(service.tag_details(&repo, invalid, &cancel).is_err());
        }
        let message_file = root.join("message");
        fs::write(&message_file, "x".repeat(TAG_LIMIT + 1)).unwrap();
        git(&["tag", "-a", "large", "-F", message_file.to_str().unwrap()]);
        assert!(
            service
                .tag_details(&repo, "large", &cancel)
                .unwrap_err()
                .to_string()
                .contains("256 KiB")
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            service
                .tag_details(&repo, "multi", &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        fs::remove_dir_all(root.join(".git")).unwrap();
        assert!(service.tag_details(&repo, "multi", &cancel).is_err());
    }

    #[test]
    fn details_and_files() {
        let raw = format!(
            "{A}\0{B}\0Ann\0ann@example.invalid\x001790996063\0Cy\0cy@example.invalid\x00\
             1790996099\0subject\n\nbody\0with nul? no\n\n"
        );
        let details = parse_details(raw.as_bytes()).unwrap();
        assert_eq!(details.parents, [B]);
        assert_eq!(details.committer, "Cy");
        assert_eq!(details.commit_time, 1_790_996_099);
        assert_eq!(details.message, "subject\n\nbody\0with nul? no");
        let files = parse_name_status(b"M\0src/a.rs\0R087\0old.rs\0new.rs\0A\0x y\0").unwrap();
        assert_eq!(files[0].status, 'M');
        assert_eq!(files[1].path, PathBuf::from("new.rs"));
        assert_eq!(files[1].original_path, Some(PathBuf::from("old.rs")));
        assert_eq!(files[2].path, PathBuf::from("x y"));
        assert!(parse_name_status(b"M\0").is_err());
        assert!(parse_name_status(b"").unwrap().is_empty());
    }
}
