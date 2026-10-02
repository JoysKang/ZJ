use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{GitService, WriteOperation, WriteRequest};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn git(root: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn fixture() -> Fixture {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "zj-writes-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    for name in ["a", "b"] {
        let repo = path.join(name);
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            git(&repo, &["config", key, value]);
        }
        fs::create_dir(repo.join("src")).unwrap();
        fs::write(repo.join("src/main.rs"), "original\n").unwrap();
    }
    Fixture(path)
}
fn write(service: &GitService, root: &Path, operation: WriteOperation) -> std::io::Result<()> {
    let cancel = AtomicBool::new(false);
    let repo = service.identify(root, &cancel)?;
    let expected = service.status(&repo, 1, &cancel)?;
    service
        .write(
            &WriteRequest {
                repo,
                expected: expected.into(),
                operation,
                generation: 2,
            },
            &cancel,
        )
        .map(|_| ())
}
fn paths() -> Vec<PathBuf> {
    vec!["src/main.rs".into()]
}

#[test]
fn disk_stage_unborn_unstage_commit_hook_discard_and_local_push() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    fs::write(root.join("src/main.rs"), "unsaved-on-disk\n").unwrap();
    write(&service, &root, WriteOperation::Unstage { paths: paths() }).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "unsaved-on-disk\n"
    );
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    fs::write(root.join("src/main.rs"), "keep-out-of-commit\n").unwrap();
    let hook = root.join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\necho hook-rejected >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        write(
            &service,
            &root,
            WriteOperation::Commit {
                message: "fixture commit".into(),
                amend: false,
                push: false,
                all: false,
            }
        )
        .err()
        .unwrap()
        .to_string()
        .contains("hook-rejected")
    );
    fs::remove_file(hook).unwrap();
    write(
        &service,
        &root,
        WriteOperation::Commit {
            message: "fixture commit\n\nonly staged".into(),
            amend: false,
            push: false,
            all: false,
        },
    )
    .unwrap();
    assert_eq!(
        git(&root, &["show", "HEAD:src/main.rs"]),
        "unsaved-on-disk\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "keep-out-of-commit\n"
    );
    // Discarding a tracked edit together with an untracked file restores one, deletes the other.
    fs::write(root.join("src/new.rs"), "untracked\n").unwrap();
    let mut both = paths();
    both.push("src/new.rs".into());
    write(&service, &root, WriteOperation::Discard { paths: both }).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "unsaved-on-disk\n"
    );
    assert!(!root.join("src/new.rs").exists());
    fs::write(root.join("only-new.txt"), "untracked\n").unwrap();
    write(
        &service,
        &root,
        WriteOperation::Discard {
            paths: vec!["only-new.txt".into()],
        },
    )
    .unwrap();
    assert!(!root.join("only-new.txt").exists());
    assert!(
        write(&service, &root, WriteOperation::Push)
            .err()
            .unwrap()
            .to_string()
            .contains("尚未配置上游")
    );
    let bare = fixture.0.join("remote.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);
    fs::write(root.join("src/main.rs"), "pushed\n").unwrap();
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    write(
        &service,
        &root,
        WriteOperation::Commit {
            message: "push fixture".into(),
            amend: false,
            push: false,
            all: false,
        },
    )
    .unwrap();
    // User config must not broaden the explicit current-branch push.
    git(&root, &["config", "push.default", "matching"]);
    git(&root, &["config", "push.followTags", "true"]);
    write(&service, &root, WriteOperation::Push).unwrap();
    assert_eq!(git(&bare, &["show", "main:src/main.rs"]), "pushed\n");
    let other = fixture.0.join("b");
    assert!(
        git(&other, &["ls-files"]).is_empty(),
        "same relative path in other repository must remain untouched"
    );
    // Remote divergence is rejected rather than forced.
    git(&root, &["reset", "--soft", "HEAD~1"]);
    write(
        &service,
        &root,
        WriteOperation::Commit {
            message: "divergent fixture".into(),
            amend: false,
            push: false,
            all: false,
        },
    )
    .unwrap();
    assert!(write(&service, &root, WriteOperation::Push).is_err());
    assert_eq!(
        git(&bare, &["log", "main", "-1", "--format=%s"]).trim(),
        "push fixture"
    );
}

#[test]
fn stale_snapshot_identity_paths_and_rename() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    let expected = service.status(&repo, 1, &cancel).unwrap();
    let mut request = WriteRequest {
        repo: repo.clone(),
        expected: expected.into(),
        generation: 2,
        operation: WriteOperation::Stage { paths: paths() },
    };
    fs::write(root.join("src/main.rs"), "changed after snapshot\n").unwrap();
    assert!(
        service
            .write(&request, &cancel)
            .err()
            .unwrap()
            .to_string()
            .contains("已变化")
    );
    request.expected = service.status(&repo, 1, &cancel).unwrap().into();
    request.operation = WriteOperation::Stage {
        paths: vec!["../b/src/main.rs".into()],
    };
    assert!(service.write(&request, &cancel).is_err());
    request.operation = WriteOperation::Stage { paths: paths() };
    request.repo.worktree = fixture.0.join("b");
    assert!(
        service
            .write(&request, &cancel)
            .err()
            .unwrap()
            .to_string()
            .contains("身份")
    );
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    write(
        &service,
        &root,
        WriteOperation::Commit {
            message: "base".into(),
            amend: false,
            push: false,
            all: false,
        },
    )
    .unwrap();
    git(&root, &["mv", "src/main.rs", "src/renamed.rs"]);
    let rename = vec!["src/main.rs".into(), "src/renamed.rs".into()];
    write(
        &service,
        &root,
        WriteOperation::Unstage {
            paths: rename.clone(),
        },
    )
    .unwrap();
    write(&service, &root, WriteOperation::Stage { paths: rename }).unwrap();
    assert!(git(&root, &["diff", "--cached", "--name-status"]).starts_with("R100"));
    fs::write(root.join("src/renamed.rs"), "local edit after rename\n").unwrap();
    let changed = service.status(&repo, 1, &cancel).unwrap();
    let change = &changed.changes[0];
    assert_eq!((change.index, change.worktree), (b'R', b'M'));
    let local_paths = change.paths(workspace_editor_git::DiffSide::Worktree);
    assert_eq!(local_paths, vec![PathBuf::from("src/renamed.rs")]);
    write(
        &service,
        &root,
        WriteOperation::Discard {
            paths: local_paths.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("src/renamed.rs")).unwrap(),
        "changed after snapshot\n"
    );
    fs::write(root.join("src/renamed.rs"), "stage after rename\n").unwrap();
    write(
        &service,
        &root,
        WriteOperation::Stage { paths: local_paths },
    )
    .unwrap();
    assert_eq!(
        git(&root, &["show", ":src/renamed.rs"]),
        "stage after rename\n"
    );
}

#[test]
fn check_ignore_batches_directories_and_files() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    let ignored = service
        .check_ignore(
            &repo,
            &[
                "target/".into(),
                "src/".into(),
                "src/main.rs".into(),
                "src/debug.log".into(),
            ],
            &cancel,
        )
        .unwrap();
    assert_eq!(
        ignored,
        vec![PathBuf::from("target/"), PathBuf::from("src/debug.log")]
    );
    // Nothing ignored is exit status 1, not an error.
    assert!(
        service
            .check_ignore(&repo, &["src/main.rs".into()], &cancel)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn apply_patch_stages_and_rejects_stale_patches() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    fs::write(root.join("src/main.rs"), "changed\n").unwrap();
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let patch = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,1 @@\n-original\n+changed\n";
    let apply = |patch: &str| {
        write(
            &service,
            &root,
            WriteOperation::ApplyPatch {
                path: "src/main.rs".into(),
                patch: patch.into(),
                cached: true,
            },
        )
    };
    apply(patch).unwrap();
    assert_eq!(git(&root, &["show", ":src/main.rs"]), "changed\n");
    // The index no longer has "original": the same patch must fail, not half-apply.
    fs::write(root.join("src/main.rs"), "again\n").unwrap();
    assert!(apply(patch).is_err());
    assert_eq!(git(&root, &["show", ":src/main.rs"]), "changed\n");
}

#[test]
fn amend_keeps_or_replaces_the_message_and_commit_push_publishes() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let commit = |message: &str, amend: bool, push: bool| WriteOperation::Commit {
        message: message.into(),
        amend,
        push,
        all: false,
    };
    // Nothing to amend before the first commit.
    assert!(write(&service, &root, commit("", true, false)).is_err());
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    write(&service, &root, commit("first", false, false)).unwrap();
    // Amend with an empty message keeps it and folds in what is staged.
    fs::write(root.join("src/main.rs"), "amended\n").unwrap();
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    write(&service, &root, commit("  ", true, false)).unwrap();
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]).trim(), "first");
    assert_eq!(git(&root, &["show", "HEAD:src/main.rs"]), "amended\n");
    assert_eq!(git(&root, &["rev-list", "--count", "HEAD"]).trim(), "1");
    // Amend with a message rewrites it, even with nothing staged.
    write(&service, &root, commit("renamed", true, false)).unwrap();
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]).trim(), "renamed");
    // Commit and push: no upstream means nothing is committed.
    fs::write(root.join("src/main.rs"), "pushed\n").unwrap();
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    assert!(
        write(&service, &root, commit("needs upstream", false, true))
            .unwrap_err()
            .to_string()
            .contains("尚未配置上游")
    );
    assert_eq!(git(&root, &["rev-list", "--count", "HEAD"]).trim(), "1");
    let bare = fixture.0.join("remote.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);
    write(&service, &root, commit("second", false, true)).unwrap();
    assert_eq!(
        git(&bare, &["log", "main", "-1", "--format=%s"]).trim(),
        "second"
    );
    assert_eq!(git(&bare, &["show", "main:src/main.rs"]), "pushed\n");
    // Amending and pushing would need a force push.
    assert!(write(&service, &root, commit("x", true, true)).is_err());
    // Smart commit: nothing staged, so everything (including untracked files) is staged first.
    fs::write(root.join("src/main.rs"), "smart\n").unwrap();
    fs::write(root.join("added.txt"), "new\n").unwrap();
    assert!(write(&service, &root, commit("not staged", false, false)).is_err());
    write(
        &service,
        &root,
        WriteOperation::Commit {
            message: "smart".into(),
            amend: false,
            push: false,
            all: true,
        },
    )
    .unwrap();
    assert_eq!(git(&root, &["show", "HEAD:added.txt"]), "new\n");
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
}
