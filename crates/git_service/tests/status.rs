mod common;

use std::{fs, path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};
use workspace_editor_git::GitService;

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

#[test]
fn a_configured_upstream_without_its_tracking_ref_is_reported_as_configured() {
    common::hermetic();
    let base = std::env::temp_dir().join(format!(
        "zj-status-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let remote = base.join("remote.git");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare"]);
    git(&repo, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/hooks"),
    ] {
        git(&repo, &["config", key, value]);
    }
    fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "pushed"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["checkout", "-q", "-b", "perf/x"]);
    fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "perf"]);
    git(&repo, &["push", "-q", "-u", "origin", "perf/x"]);

    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    let cancel = AtomicBool::new(false);
    let identified = service.identify(&repo, &cancel).unwrap();
    let status = service.status(&identified, 1, &cancel).unwrap();
    assert!(status.upstream.is_some());
    assert!(!status.upstream_configured);

    // A fetch refspec that only covers the default branch cannot store this branch's tracking
    // ref, so `git status` stops reporting `branch.upstream` even though it is configured.
    git(
        &repo,
        &[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/perf/x"]);
    let status = service.status(&identified, 1, &cancel).unwrap();
    assert!(status.upstream.is_none());
    assert!(status.upstream_configured);

    // A branch with no upstream configuration at all stays unpublished.
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    let status = service.status(&identified, 1, &cancel).unwrap();
    assert!(status.upstream.is_none());
    assert!(!status.upstream_configured);
    fs::remove_dir_all(&base).unwrap();
}
