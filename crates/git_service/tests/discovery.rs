//! Repository discovery skips ignored and cache directories and invalid `.git` markers.
mod common;

use std::{fs, path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};
use workspace_editor_git::{Discovery, GitService};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-C"])
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
}

fn repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q"]);
}

#[test]
fn discovery_respects_ignores_caches_depth_and_git_markers() {
    common::hermetic();
    let root = std::env::temp_dir().join(format!("zj-discovery-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(&root).unwrap();
    repo(&root.join("app"));
    fs::write(root.join("app/.gitignore"), "build/\nvendored/\n").unwrap();
    // Inside an ignored directory: not walked.
    repo(&root.join("app/build/deps/checkout"));
    // An ignored directory that is itself a repository is still found.
    repo(&root.join("app/vendored"));
    // Cache and dependency directories are never walked.
    repo(&root.join("app/node_modules/pkg"));
    fs::create_dir_all(root.join("app/.uv-cache/sdists-v9")).unwrap();
    fs::write(root.join("app/.uv-cache/sdists-v9/.git"), "not a gitfile\n").unwrap();
    // A stray invalid marker elsewhere is skipped silently, and so is what is below it.
    fs::create_dir_all(root.join("tools/stale/inner")).unwrap();
    fs::write(root.join("tools/stale/.git"), "not a gitfile\n").unwrap();
    repo(&root.join("tools/stale/inner/repo"));
    // Depth: four levels below the root are checked, deeper ones are not.
    repo(&root.join("a/b/c/shallow"));
    repo(&root.join("a/b/c/d/e/deep"));

    let service = GitService::new(2, Duration::from_secs(10)).unwrap();
    let mut found = Vec::new();
    service.discover(
        std::slice::from_ref(&root),
        &AtomicBool::new(false),
        |event| match event {
            Discovery::Repository(repo) => found.push(
                repo.worktree
                    .strip_prefix(&root)
                    .unwrap()
                    .display()
                    .to_string(),
            ),
            Discovery::Issue(path, e) => panic!("{}: {e}", path.display()),
            _ => {}
        },
    );
    found.sort();
    assert_eq!(found, ["a/b/c/shallow", "app", "app/vendored"]);
    fs::remove_dir_all(root).unwrap();
}
