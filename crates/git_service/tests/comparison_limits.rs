mod common;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{CommitSearch, GitService, GraphScope};
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn repository_search_reaches_before_the_first_500_commits_and_blob_reads_are_bounded() {
    common::hermetic();
    let root = std::env::temp_dir().join(format!(
        "zj-query-limits-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let f = Fixture(root);
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&f.0)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&f.0)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for i in 0..510 {
        let message = if i == 0 {
            "ancient searchable commit".to_string()
        } else {
            format!("new commit {i}")
        };
        writeln!(input,"commit refs/heads/main\nmark :{}\nauthor {} <fixture@example.invalid> {} +0000\ncommitter Fixture <fixture@example.invalid> {} +0000\ndata {}\n{}",i+1,if i==0{"Old Author"}else{"New Author"},1_577_836_800+i,1_577_836_800+i,message.len(),message).unwrap();
        if i > 0 {
            writeln!(input, "from :{i}").unwrap();
        }
        if i == 0 {
            writeln!(input, "M 100644 inline file.txt\ndata 5\nbase\n").unwrap();
        }
        writeln!(input).unwrap();
    }
    writeln!(input, "done").unwrap();
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let service = GitService::new(2, Duration::from_secs(5)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&f.0, &cancel).unwrap();
    let visible = service
        .search_commits(
            &repo,
            &GraphScope::All,
            &CommitSearch::default(),
            0,
            500,
            &cancel,
        )
        .unwrap();
    assert_eq!(visible.len(), 500);
    assert!(!visible.iter().any(|c| c.subject.contains("ancient")));
    let query = CommitSearch {
        author: "Old Author".into(),
        since: "2020-01-01".into(),
        until: "2020-01-01".into(),
        message: "ancient".into(),
        path: Some("file.txt".into()),
        ..Default::default()
    };
    let result = service
        .search_commits(&repo, &GraphScope::All, &query, 0, 500, &cancel)
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].subject, "ancient searchable commit");
    for date in ["2100-01-01", "2024-02-30", "--all", "2020-13-01"] {
        let query = CommitSearch {
            since: date.into(),
            ..Default::default()
        };
        assert!(
            service
                .search_commits(&repo, &GraphScope::All, &query, 0, 500, &cancel)
                .is_err()
        );
    }
    // Import an oversized blob without reading or creating a large worktree file.
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&f.0)
        .args(["hash-object", "-w", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&vec![b'x'; 16_000_001])
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let oid = String::from_utf8(output.stdout).unwrap();
    let tree = format!("100644 blob {}\tlarge.bin\n", oid.trim());
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&f.0)
        .arg("mktree")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(tree.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let tree = String::from_utf8(output.stdout).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&f.0)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit-tree",
            tree.trim(),
            "-m",
            "large blob",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let commit = String::from_utf8(output.stdout).unwrap();
    let error = service
        .read_blob(&repo, commit.trim(), "large.bin".as_ref(), &cancel)
        .unwrap_err();
    assert!(error.to_string().contains("输出"), "{error}");
}
