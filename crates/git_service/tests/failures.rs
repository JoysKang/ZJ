//! Git failures must not read as "absent". Runs alone in its own process: it puts a wrapper
//! around `git` first on PATH that fails every `rev-parse --verify` the way a broken
//! repository or a timed-out query would.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{GitService, GraphScope, WriteOperation, WriteRequest};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(real: &Path, root: &Path, args: &[&str]) -> String {
    let result = Command::new(real)
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
fn failed_verify_is_an_error_not_a_missing_ref() {
    let path = std::env::temp_dir().join(format!(
        "zj-failures-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&path).unwrap();
    let fixture = Fixture(path);
    let which = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = PathBuf::from(String::from_utf8(which.stdout).unwrap().trim());
    assert!(real.is_absolute(), "git not found on PATH");
    let bin = fixture.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\ncase \" $* \" in *\" --verify \"*) echo 'fatal: injected failure' >&2; exit 128;; esac\nexec '{}' \"$@\"\n",
            real.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&path));
    // SAFETY: the only test in this binary; no other thread reads the environment yet.
    unsafe {
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }

    let root = fixture.0.join("repo");
    fs::create_dir(&root).unwrap();
    git(&real, &root, &["init", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
    ] {
        git(&real, &root, &["config", key, value]);
    }
    fs::write(root.join("file.txt"), "one\n").unwrap();
    git(&real, &root, &["add", "-A"]);
    git(&real, &root, &["commit", "-m", "one"]);
    git(&real, &root, &["tag", "v1"]);
    let head = git(&real, &root, &["rev-parse", "HEAD"]).trim().to_owned();

    let service = GitService::new(1, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&root, &cancel).unwrap();
    // A detached HEAD is only listed through HEAD itself.
    git(&real, &root, &["switch", "--detach", "HEAD"]);
    let graph = service.graph(&repo, &GraphScope::Local, 0, 10, &cancel);
    assert!(
        graph
            .as_ref()
            .is_err_and(|e| e.to_string().contains("injected")),
        "{graph:?}"
    );
    git(&real, &root, &["switch", "main"]);

    let write = |operation| {
        let expected = service.status(&repo, 1, &cancel).unwrap();
        service
            .write(
                &WriteRequest {
                    repo: repo.clone(),
                    generation: 2,
                    expected: expected.into(),
                    operation,
                },
                &cancel,
            )
            .map(|_| ())
            .unwrap_err()
            .to_string()
    };
    for operation in [
        WriteOperation::CreateTag {
            name: "v2".into(),
            commit: head,
            message: None,
            push: false,
        },
        WriteOperation::PushTag { name: "v1".into() },
        WriteOperation::DeleteTag {
            name: "v1".into(),
            remote: false,
        },
    ] {
        let error = write(operation);
        assert!(error.contains("injected"), "{error}");
    }
    assert_eq!(git(&real, &root, &["tag"]), "v1\n");
}
