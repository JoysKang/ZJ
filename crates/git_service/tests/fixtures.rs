use std::{
    ffi::OsString,
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{ChangeKind, DiffSide, Discovery, GitService, Operation, Request};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn real_git_identities_states_diffs_and_cancellation() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("zj-test-{}-{stamp}", std::process::id()));
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/fixtures.py");
    assert!(
        Command::new("python3")
            .arg(script)
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success()
    );
    let fixture = Fixture(path);
    let roots: Vec<_> = (1..=3)
        .map(|n| fixture.0.join(format!("workspace-{n}")))
        .collect();
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let mut found = Vec::new();
    let mut overlapping = roots.clone();
    overlapping.push(roots[0].join("repo-00"));
    let alias = fixture.0.join("alias");
    std::os::unix::fs::symlink(&roots[0], &alias).unwrap();
    overlapping.push(alias);
    service.discover(&overlapping, &cancel, |event| match event {
        Discovery::Repository(repo) => found.push(repo),
        Discovery::Issue(_, e) => panic!("{e}"),
        _ => {}
    });
    assert_eq!(
        found.len(),
        36,
        "F fixture, overlaps and symlinks must not duplicate worktrees"
    );
    let parent = service
        .identify(&roots[0].join("repo-08"), &cancel)
        .unwrap();
    let linked1 = service.identify(&roots[1].join("linked"), &cancel).unwrap();
    let linked2 = service.identify(&roots[2].join("linked"), &cancel).unwrap();
    assert_ne!(parent.id, linked1.id);
    assert_ne!(linked1.id, linked2.id);
    assert_eq!(parent.common_dir, linked1.common_dir);
    assert_eq!(linked1.common_dir, linked2.common_dir);
    assert_eq!(
        service
            .identify(&roots[0].join("repo-08/src"), &cancel)
            .unwrap()
            .id,
        parent.id
    );
    assert!(roots[0].join("repo-09/.git").is_file());
    let repo = service
        .identify(&roots[0].join("repo-01"), &cancel)
        .unwrap();
    let s = service.status(&repo, 3, &cancel).unwrap();
    assert_eq!(s.changes.len(), 1);
    assert!(s.changes[0].staged() && s.changes[0].unstaged());
    let request = |side| Request {
        repo: repo.clone(),
        generation: 3,
        operation: Operation::Diff {
            side,
            path: "src/main.rs".into(),
            original_path: None,
        },
    };
    let staged = service
        .execute(&request(DiffSide::Staged), &cancel)
        .unwrap();
    let unstaged = service
        .execute(&request(DiffSide::Worktree), &cancel)
        .unwrap();
    assert_eq!(staged.generation, 3);
    assert_eq!(staged.repo, repo.id);
    let staged = String::from_utf8(staged.output).unwrap();
    let unstaged = String::from_utf8(unstaged.output).unwrap();
    assert!(staged.contains("+fn main() { println!(\"已暂存\"); }"));
    assert!(unstaged.contains("+fn main() { println!(\"再次修改\"); }"));
    let invalid = Request {
        repo: repo.clone(),
        generation: 4,
        operation: Operation::Diff {
            side: DiffSide::Staged,
            path: "../other".into(),
            original_path: None,
        },
    };
    assert!(service.execute(&invalid, &cancel).is_err());
    let renamed = service
        .identify(&roots[0].join("repo-02"), &cancel)
        .unwrap();
    let rename_status = service.status(&renamed, 4, &cancel).unwrap();
    let change = &rename_status.changes[0];
    let rename = service
        .execute(
            &Request {
                repo: renamed,
                generation: 4,
                operation: Operation::Diff {
                    side: DiffSide::Staged,
                    path: change.path.clone(),
                    original_path: change.original_path.clone(),
                },
            },
            &cancel,
        )
        .unwrap();
    let rename = String::from_utf8(rename.output).unwrap();
    assert!(
        rename.contains("rename from src/main.rs")
            && rename.contains("rename to src/renamed file.rs")
    );
    for (number, kind) in [
        (2, ChangeKind::Renamed),
        (3, ChangeKind::Untracked),
        (5, ChangeKind::Conflict),
    ] {
        let repo = service
            .identify(&roots[0].join(format!("repo-{number:02}")), &cancel)
            .unwrap();
        assert_eq!(
            service.status(&repo, 1, &cancel).unwrap().changes[0].kind,
            kind
        );
    }
    let untracked = service
        .identify(&roots[0].join("repo-03"), &cancel)
        .unwrap();
    let change = service
        .status(&untracked, 5, &cancel)
        .unwrap()
        .changes
        .remove(0);
    let added = service
        .execute(
            &Request {
                repo: untracked.clone(),
                generation: 5,
                operation: Operation::UntrackedDiff {
                    path: change.path.clone(),
                },
            },
            &cancel,
        )
        .unwrap();
    assert!(
        String::from_utf8(added.output).unwrap().contains("+未跟踪"),
        "untracked file must have an addition patch"
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&untracked.worktree)
            .args(["--literal-pathspecs", "add", "--"])
            .arg(&change.path)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        service
            .execute(
                &Request {
                    repo: untracked.clone(),
                    generation: 6,
                    operation: Operation::UntrackedDiff {
                        path: change.path.clone()
                    },
                },
                &cancel
            )
            .unwrap()
            .output
            .is_empty(),
        "a newly staged file must compare against the index"
    );
    for path in ["../outside", "/etc/hosts", "missing-file"] {
        assert!(
            service
                .execute(
                    &Request {
                        repo: untracked.clone(),
                        generation: 5,
                        operation: Operation::UntrackedDiff { path: path.into() },
                    },
                    &cancel
                )
                .is_err(),
            "invalid/missing path must be an error: {path}"
        );
    }
    for (number, expected) in [(4, "-fn main()"), (5, "diff --cc"), (7, "Binary files")] {
        let repo = service
            .identify(&roots[0].join(format!("repo-{number:02}")), &cancel)
            .unwrap();
        let change = service.status(&repo, 5, &cancel).unwrap().changes.remove(0);
        let reply = service
            .execute(
                &Request {
                    repo,
                    generation: 5,
                    operation: Operation::Diff {
                        side: DiffSide::Worktree,
                        path: change.path,
                        original_path: change.original_path,
                    },
                },
                &cancel,
            )
            .unwrap();
        assert!(String::from_utf8(reply.output).unwrap().contains(expected));
    }
    let unborn = service
        .identify(&roots[0].join("repo-06"), &cancel)
        .unwrap();
    assert_eq!(
        service.status(&unborn, 1, &cancel).unwrap().oid.as_deref(),
        Some("(initial)")
    );
    // APFS rejects invalid UTF-8 filenames (EILSEQ). Raw bytes are covered by the parser test.
    let raw = OsString::from("raw 空格\nname");
    fs::write(repo.worktree.join(&raw), b"sample").unwrap();
    assert!(
        service
            .status(&repo, 1, &cancel)
            .unwrap()
            .changes
            .iter()
            .any(|c| c.path.as_os_str() == raw)
    );
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        service.status(&repo, 4, &cancel).unwrap_err().kind(),
        std::io::ErrorKind::Interrupted
    );
    let timeout = GitService::new(1, Duration::ZERO).unwrap();
    assert_eq!(
        timeout
            .identify(&repo.worktree, &AtomicBool::new(false))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(GitService::new(0, Duration::from_secs(1)).is_err());
}
