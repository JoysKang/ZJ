//! One test process so its Git wrapper cannot affect other tests' PATH.
mod common;

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::AtomicBool, mpsc},
    time::{Duration, Instant},
};
use workspace_editor_git::{GitService, WriteOperation, WriteRequest};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn linked_worktrees_cannot_drop_a_different_stash_after_validation() {
    common::hermetic();
    let fixture =
        Fixture(std::env::temp_dir().join(format!("zj-stash-lock-{}", std::process::id())));
    fs::create_dir_all(&fixture.0).unwrap();
    let root = fixture.0.join("repo");
    let other = fixture.0.join("linked");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        git(&root, &["config", key, value]);
    }
    fs::write(root.join("file"), "base\n").unwrap();
    git(&root, &["add", "file"]);
    git(&root, &["commit", "-m", "base"]);
    for text in ["older\n", "newer\n"] {
        fs::write(root.join("file"), text).unwrap();
        git(&root, &["stash", "push", "-m", text.trim()]);
    }
    git(
        &root,
        &["worktree", "add", "--detach", other.to_str().unwrap()],
    );
    let service = GitService::new(4, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let a = service.identify(&root, &cancel).unwrap();
    let b = service.identify(&other, &cancel).unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(a.common_dir, b.common_dir);
    assert_eq!(a.common_dir, fs::canonicalize(root.join(".git")).unwrap());
    let stashes = service.stashes(&a, &cancel).unwrap();
    let requests: Vec<_> = [a.clone(), b]
        .into_iter()
        .map(|repo| WriteRequest {
            expected: service.status(&repo, 1, &cancel).unwrap().into(),
            repo,
            generation: 1,
            operation: WriteOperation::StashDrop {
                index: 0,
                oid: stashes[0].oid.clone(),
            },
        })
        .collect();

    let real = Command::new("which").arg("git").output().unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let entered = fixture.0.join("entered");
    let release = fixture.0.join("release");
    let bin = fixture.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    // Pause the first worktree after ZJ verified the oid, before Git resolves stash@{0}.
    fs::write(&wrapper, format!(
        "#!/bin/sh\ncase \" $* \" in *\" -C \"{}\" stash drop \"*)\ntouch {}\nwhile [ ! -e {} ]; do sleep 0.01; done;;\nesac\nexec {} \"$@\"\n",
        quote(&a.worktree), quote(&entered), quote(&release), quote(Path::new(&real))
    )).unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&original_path));
    // SAFETY: the only test in this binary; no worker has started yet.
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    }
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| service.write(&requests[0], &cancel));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !entered.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            entered.exists(),
            "first stash delete did not reach the Git wrapper"
        );
        let (done, result) = mpsc::channel();
        let (service, request, cancel) = (&service, &requests[1], &cancel);
        let second = scope.spawn(move || {
            done.send(service.write(request, cancel)).unwrap();
        });
        // Before the fix, the other worktree can finish deleting the same index here.
        let early = result.recv_timeout(Duration::from_secs(1)).ok();
        fs::write(&release, "go").unwrap();
        let first = first.join().unwrap();
        second.join().unwrap();
        (first, early.unwrap_or_else(|| result.recv().unwrap()))
    });
    assert!(first.is_ok(), "{:?}", first.as_ref().err());
    assert!(
        second.is_err(),
        "both worktrees deleted stash@{{0}}, including an unselected stash"
    );
    let left = service.stashes(&a, &cancel).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].oid, stashes[1].oid);
}
