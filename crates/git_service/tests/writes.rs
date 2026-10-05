use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{
    DiffSide, GitService, Operation, Request, WriteOperation, WriteRequest,
};

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

#[test]
fn outgoing_lists_the_commits_the_upstream_lacks() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(1, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "pushed"]);
    let repo = service.identify(&root, &cancel).unwrap();
    // No upstream: Git's error is shown, not an empty list.
    assert!(service.outgoing(&repo, 10, &cancel).is_err());
    let bare = fixture.0.join("remote.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);
    assert_eq!(service.outgoing(&repo, 10, &cancel).unwrap(), Vec::new());
    for subject in ["one", "two 中文", "three"] {
        git(&root, &["commit", "--allow-empty", "-m", subject]);
    }
    let commits = service.outgoing(&repo, 10, &cancel).unwrap();
    let subjects: Vec<_> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["three", "two 中文", "one"]);
    assert_eq!(
        commits[0].short,
        git(&root, &["rev-parse", "--short", "HEAD"]).trim()
    );
    assert_eq!(
        commits[0].time.to_string(),
        git(&root, &["log", "-1", "--format=%at"]).trim()
    );
    assert_eq!(service.outgoing(&repo, 2, &cancel).unwrap().len(), 2);
}

fn configure(repo: &Path) {
    for (key, value) in [
        ("user.name", "Other"),
        ("user.email", "other@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
    ] {
        git(repo, &["config", key, value]);
    }
}

#[test]
fn fetch_pull_sync_checkout_and_create_branch() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(20)).unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    // No upstream: nothing is pulled or pushed.
    for operation in [WriteOperation::Pull, WriteOperation::Sync] {
        let error = write(&service, &root, operation).unwrap_err().to_string();
        assert!(error.contains("尚未配置上游"), "{error}");
    }
    let bare = fixture.0.join("remote.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);
    // The user's global pull settings must not decide the test.
    git(&root, &["config", "pull.rebase", "false"]);
    let other = fixture.0.join("other");
    git(
        &fixture.0,
        &["clone", bare.to_str().unwrap(), other.to_str().unwrap()],
    );
    configure(&other);
    git(&other, &["commit", "--allow-empty", "-m", "remote one"]);
    git(&other, &["push", "origin", "main"]);
    let remote_one = git(&other, &["rev-parse", "HEAD"]).trim().to_string();

    // Fetch moves the remote-tracking branch only.
    write(&service, &root, WriteOperation::Fetch).unwrap();
    assert_eq!(git(&root, &["rev-parse", "origin/main"]).trim(), remote_one);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).trim(), base);
    write(&service, &root, WriteOperation::Pull).unwrap();
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).trim(), remote_one);

    // Sync on diverged branches: pull (merge), then push the result.
    git(&other, &["commit", "--allow-empty", "-m", "remote two"]);
    git(&other, &["push", "origin", "main"]);
    fs::write(root.join("src/main.rs"), "local\n").unwrap();
    git(&root, &["commit", "-am", "local"]);
    write(&service, &root, WriteOperation::Sync).unwrap();
    let subjects = git(&bare, &["log", "main", "--format=%s"]);
    assert!(subjects.contains("remote two") && subjects.contains("local"));
    assert_eq!(
        git(&bare, &["rev-parse", "main"]),
        git(&root, &["rev-parse", "HEAD"])
    );

    // Checking out a remote-tracking branch creates the tracking local branch.
    git(&other, &["switch", "-c", "feature"]);
    git(&other, &["commit", "--allow-empty", "-m", "feature work"]);
    git(&other, &["push", "origin", "feature"]);
    write(&service, &root, WriteOperation::Fetch).unwrap();
    let checkout = |branch: &str, remote: bool| WriteOperation::Checkout {
        branch: branch.into(),
        remote,
    };
    write(&service, &root, checkout("origin/feature", true)).unwrap();
    assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "feature");
    assert_eq!(
        git(&root, &["rev-parse", "--abbrev-ref", "@{upstream}"]).trim(),
        "origin/feature"
    );
    write(&service, &root, checkout("main", false)).unwrap();
    assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "main");
    for (branch, remote) in [("missing", false), ("origin/missing", true), ("-f", false)] {
        assert!(write(&service, &root, checkout(branch, remote)).is_err());
    }
    assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "main");

    // New branches: at HEAD, at a commit, never over an existing one or with a bad name.
    let create = |name: &str, start: Option<&str>| WriteOperation::CreateBranch {
        name: name.into(),
        start: start.map(str::to_string),
    };
    write(&service, &root, create("topic", None)).unwrap();
    assert_eq!(git(&root, &["branch", "--show-current"]).trim(), "topic");
    let error = write(&service, &root, create("topic", None))
        .unwrap_err()
        .to_string();
    assert!(error.contains("已存在"), "{error}");
    for name in ["a..b", "with space", "-x", ""] {
        assert!(
            write(&service, &root, create(name, None)).is_err(),
            "{name}"
        );
    }
    write(&service, &root, create("old", Some(&base))).unwrap();
    assert_eq!(git(&root, &["rev-parse", "HEAD"]).trim(), base);
    assert!(write(&service, &root, create("bad-start", Some("HEAD"))).is_err());
}

#[test]
fn branches_graph_details_and_commit_diff() {
    use workspace_editor_git::{GraphScope, Operation, RefKind, Request};
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    // An unborn branch has no commits, and that is not an error.
    assert!(
        service
            .graph(&repo, &GraphScope::All, 0, 10, &cancel)
            .unwrap()
            .is_empty()
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "root"]);
    let root_commit = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    git(&root, &["mv", "src/main.rs", "src/lib.rs"]);
    git(&root, &["commit", "-m", "rename"]);
    let rename = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    git(&root, &["switch", "-c", "feature"]);
    git(&root, &["commit", "--allow-empty", "-m", "feature work"]);
    git(&root, &["switch", "main"]);
    git(&root, &["commit", "--allow-empty", "-m", "main work"]);
    git(
        &root,
        &["merge", "--no-ff", "-m", "merge feature", "feature"],
    );
    git(&root, &["tag", "v1"]);

    let branches = service.branches(&repo, &cancel).unwrap();
    let names: Vec<_> = branches
        .iter()
        .map(|b| (b.name.as_str(), b.head, b.remote))
        .collect();
    assert!(names.contains(&("main", true, false)));
    assert!(names.contains(&("feature", false, false)));

    let all = service
        .graph(&repo, &GraphScope::All, 0, 100, &cancel)
        .unwrap();
    let subjects: Vec<_> = all.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects.len(), 5);
    assert_eq!(subjects[0], "merge feature");
    assert_eq!(*subjects.last().unwrap(), "root");
    assert_eq!(all[0].parents.len(), 2);
    assert!(
        all[0]
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Branch && r.name == "main" && r.head)
    );
    assert!(
        all[0]
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Tag && r.name == "v1")
    );
    // Every parent is listed after its child.
    for (i, commit) in all.iter().enumerate() {
        for parent in &commit.parents {
            assert!(all[i + 1..].iter().any(|c| &c.hash == parent));
        }
    }
    let page = service
        .graph(&repo, &GraphScope::All, 1, 2, &cancel)
        .unwrap();
    assert_eq!(page, all[1..3]);
    let feature = service
        .graph(
            &repo,
            &GraphScope::Branch("refs/heads/feature".into()),
            0,
            100,
            &cancel,
        )
        .unwrap();
    let subjects: Vec<_> = feature.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects, ["feature work", "rename", "root"]);
    for scope in ["main", "--all", "refs/tags/v1"] {
        assert!(
            service
                .graph(&repo, &GraphScope::Branch(scope.into()), 0, 10, &cancel)
                .is_err()
        );
    }

    let details = service.commit_details(&repo, &rename, &cancel).unwrap();
    assert_eq!(details.message, "rename");
    assert_eq!(details.parents, std::slice::from_ref(&root_commit));
    assert_eq!(details.files.len(), 1);
    assert_eq!(details.files[0].status, 'R');
    assert_eq!(details.files[0].path, PathBuf::from("src/lib.rs"));
    assert_eq!(
        details.files[0].original_path,
        Some(PathBuf::from("src/main.rs"))
    );
    let first = service
        .commit_details(&repo, &root_commit, &cancel)
        .unwrap();
    assert!(first.parents.is_empty());
    assert_eq!(first.files[0].status, 'A');
    assert!(service.commit_details(&repo, "HEAD", &cancel).is_err());

    let diff = |commit: &str, parent: Option<&str>, path: &str| {
        service.execute(
            &Request {
                repo: repo.clone(),
                generation: 1,
                operation: Operation::CommitDiff {
                    commit: commit.into(),
                    parent: parent.map(str::to_string),
                    path: path.into(),
                    original_path: None,
                },
            },
            &cancel,
        )
    };
    let added = diff(&root_commit, None, "src/main.rs").unwrap().output;
    let added = String::from_utf8(added).unwrap();
    assert!(added.contains("new file mode") && added.contains("+original"));
    fs::write(root.join("src/lib.rs"), "changed\n").unwrap();
    git(&root, &["commit", "-am", "edit"]);
    let edit = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let head_parent = git(&root, &["rev-parse", "HEAD^"]).trim().to_string();
    let changed = diff(&edit, Some(&head_parent), "src/lib.rs")
        .unwrap()
        .output;
    let changed = String::from_utf8(changed).unwrap();
    assert!(changed.contains("-original") && changed.contains("+changed"));
    assert!(diff("HEAD", None, "src/lib.rs").is_err());
    assert!(diff(&edit, Some(&head_parent), "../escape").is_err());
}

#[test]
fn tags_are_created_pushed_and_deleted() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(20)).unwrap();
    // The user's global tag signing must not decide the test.
    git(&root, &["config", "tag.gpgSign", "false"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    let base = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    let create = |name: &str, message: Option<&str>, push: bool| WriteOperation::CreateTag {
        name: name.into(),
        commit: base.clone(),
        message: message.map(str::to_string),
        push,
    };
    let tags = |repo: &Path| {
        git(repo, &["tag", "--list"])
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    // Without a remote, a tag to push is not created at all.
    let error = write(&service, &root, create("early", None, true))
        .unwrap_err()
        .to_string();
    assert!(error.contains("没有配置远程"), "{error}");
    assert!(tags(&root).is_empty());

    let bare = fixture.0.join("remote.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);

    // Lightweight without a message, annotated with one; the annotated one is pushed.
    write(&service, &root, create("v1", None, false)).unwrap();
    assert_eq!(git(&root, &["cat-file", "-t", "v1"]).trim(), "commit");
    write(&service, &root, create("v2", Some("第二版\n\n说明"), true)).unwrap();
    assert_eq!(git(&root, &["cat-file", "-t", "v2"]).trim(), "tag");
    assert_eq!(
        git(&root, &["tag", "-l", "--format=%(contents)", "v2"]).trim(),
        "第二版\n\n说明"
    );
    assert_eq!(git(&root, &["rev-parse", "v2^{commit}"]).trim(), base);
    assert_eq!(tags(&bare), ["v2"]);

    // Never over an existing tag, with a bad name, an empty message or a symbolic commit.
    let error = write(&service, &root, create("v1", None, false))
        .unwrap_err()
        .to_string();
    assert!(error.contains("已存在"), "{error}");
    for name in ["a..b", "with space", "-x", "", "x.lock"] {
        assert!(
            write(&service, &root, create(name, None, false)).is_err(),
            "{name}"
        );
    }
    assert!(write(&service, &root, create("v3", Some("  "), false)).is_err());
    let symbolic = WriteOperation::CreateTag {
        name: "v3".into(),
        commit: "HEAD".into(),
        message: None,
        push: false,
    };
    assert!(write(&service, &root, symbolic).is_err());
    assert_eq!(tags(&root), ["v1", "v2"]);

    // Pushing one tag publishes only it.
    write(
        &service,
        &root,
        WriteOperation::PushTag { name: "v1".into() },
    )
    .unwrap();
    assert_eq!(tags(&bare), ["v1", "v2"]);
    let missing = WriteOperation::PushTag {
        name: "missing".into(),
    };
    assert!(write(&service, &root, missing).is_err());

    // Deleting: on the remote too, or only locally.
    let delete = |name: &str, remote: bool| WriteOperation::DeleteTag {
        name: name.into(),
        remote,
    };
    write(&service, &root, delete("v1", true)).unwrap();
    assert_eq!(tags(&root), ["v2"]);
    assert_eq!(tags(&bare), ["v2"]);
    write(&service, &root, delete("v2", false)).unwrap();
    assert!(tags(&root).is_empty());
    assert_eq!(tags(&bare), ["v2"]);
    assert!(write(&service, &root, delete("v2", false)).is_err());
}

#[test]
fn patches_keep_a_b_prefixes_whatever_the_user_config_says() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    fs::write(root.join("src/main.rs"), "changed\n").unwrap();
    fs::write(root.join("new.txt"), "new\n").unwrap();
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    for (key, value) in [("diff.noprefix", "true"), ("diff.mnemonicPrefix", "true")] {
        git(&root, &["config", key, value]);
        for operation in [
            Operation::Diff {
                side: DiffSide::Worktree,
                path: "src/main.rs".into(),
                original_path: None,
            },
            Operation::UntrackedDiff {
                path: "new.txt".into(),
            },
        ] {
            let reply = service
                .execute(
                    &Request {
                        repo: repo.clone(),
                        generation: 1,
                        operation,
                    },
                    &cancel,
                )
                .unwrap();
            let patch = String::from_utf8(reply.output).unwrap();
            assert!(patch.contains("+++ b/"), "{key}: {patch}");
        }
        git(&root, &["config", "--unset", key]);
    }
}

#[test]
fn a_cancelled_write_asks_git_to_stop_before_killing_it() {
    let fixture = fixture();
    let root = fixture.0.join("a");
    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    write(&service, &root, WriteOperation::Stage { paths: paths() }).unwrap();
    // SIGTERM lets Git and its hooks clean up (Git removes index.lock on it, not on SIGKILL).
    let marks = fixture.0.join("marks");
    fs::create_dir(&marks).unwrap();
    let hook = root.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\ntrap 'touch {0}/term; exit 1' TERM\ntouch {0}/started\nsleep 30 &\nwait\n",
            marks.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    let expected = service.status(&repo, 1, &cancel).unwrap();
    let request = WriteRequest {
        repo,
        expected: expected.into(),
        operation: WriteOperation::Commit {
            message: "never".into(),
            amend: false,
            push: false,
            all: false,
        },
        generation: 2,
    };
    let started = std::time::Instant::now();
    let result = std::thread::scope(|scope| {
        let commit = scope.spawn(|| service.write(&request, &cancel).map(|_| ()));
        while !marks.join("started").exists() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        commit.join().unwrap()
    });
    assert!(marks.join("started").exists(), "the hook never ran");
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(marks.join("term").exists());
    assert!(!root.join(".git/index.lock").exists());
}
