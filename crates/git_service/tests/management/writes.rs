use super::*;
use workspace_editor_git::{InProgress, parse_lfs_pointer, push_result_summary, redact_git_output};
fn commit(root: &Path) {
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "fixture"]);
}
fn reply(service: &GitService, root: &Path, operation: WriteOperation) -> std::io::Result<String> {
    let cancel = AtomicBool::new(false);
    let repo = service.identify(root, &cancel)?;
    let expected = service.status(&repo, 1, &cancel)?.into();
    service
        .write(
            &WriteRequest {
                repo,
                generation: 2,
                expected,
                operation,
            },
            &cancel,
        )
        .map(|r| String::from_utf8(r.output).unwrap())
}
#[test]
fn publish_selects_exact_remote_target_and_reads_upstream() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    commit(&root);
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    for name in ["chosen", "other"] {
        let bare = f.0.join(format!("{name}.git"));
        fs::create_dir(&bare).unwrap();
        git(&bare, &["init", "--bare"]);
        write(
            &svc,
            &root,
            WriteOperation::AddRemote {
                name: name.into(),
                url: bare.to_str().unwrap().into(),
            },
        )
        .unwrap();
    }
    git(&root, &["branch", "unrelated"]);
    git(
        &root,
        &["config", "remote.chosen.push", "refs/heads/*:refs/heads/*"],
    );
    git(&root, &["config", "push.default", "matching"]);
    let publish = || WriteOperation::PublishBranch {
        remote: "chosen".into(),
        target: "published/topic".into(),
        expected_push_url: f.0.join("chosen.git").to_str().unwrap().into(),
    };
    let output = reply(&svc, &root, publish()).unwrap();
    git(
        &root,
        &[
            "config",
            "remote.chosen.pushurl",
            f.0.join("other.git").to_str().unwrap(),
        ],
    );
    assert!(
        reply(&svc, &root, publish())
            .unwrap_err()
            .to_string()
            .contains("推送地址已变化")
    );
    git(&root, &["config", "--unset", "remote.chosen.pushurl"]);
    assert_eq!(push_result_summary(&output), "远程引用已更新");
    assert!(output.contains("refs/heads/published/topic"));
    assert!(output.contains("上游配置已回读"));
    write(
        &svc,
        &root,
        WriteOperation::SetUpstream {
            branch: "main".into(),
            upstream: "refs/remotes/chosen/published/topic".into(),
        },
    )
    .unwrap();
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::SetUpstream {
                branch: "main".into(),
                upstream: "refs/remotes/chosen/missing".into()
            }
        )
        .is_err()
    );
    assert_eq!(
        git(&root, &["config", "branch.main.remote"]).trim(),
        "chosen"
    );
    assert_eq!(
        git(&root, &["config", "branch.main.merge"]).trim(),
        "refs/heads/published/topic"
    );
    assert_eq!(
        git(
            &f.0.join("chosen.git"),
            &["for-each-ref", "--format=%(refname)"]
        )
        .trim(),
        "refs/heads/published/topic"
    );
    assert!(git(&f.0.join("other.git"), &["for-each-ref"]).is_empty());
    assert_eq!(
        push_result_summary(&reply(&svc, &root, publish()).unwrap()),
        "远程无变化"
    );
    git(
        &root,
        &["update-ref", "-d", "refs/remotes/chosen/published/topic"],
    );
    reply(&svc, &root, publish()).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = svc.identify(&root, &cancel).unwrap();
    assert_eq!(
        svc.status(&repo, 1, &cancel).unwrap().upstream.as_deref(),
        Some("chosen/published/topic")
    );
    // Diverged remote cannot be overwritten by publishing again.
    git(&root, &["commit", "--allow-empty", "-m", "advance"]);
    reply(&svc, &root, publish()).unwrap();
    git(&root, &["reset", "--hard", "HEAD~1"]);
    git(&root, &["commit", "--allow-empty", "-m", "different"]);
    assert!(reply(&svc, &root, publish()).is_err());
    assert_eq!(
        git(
            &f.0.join("chosen.git"),
            &["log", "published/topic", "-1", "--format=%s"]
        )
        .trim(),
        "advance"
    );
}
#[test]
fn remote_edits_branch_guards_and_worktree_management() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    commit(&root);
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    write(
        &svc,
        &root,
        WriteOperation::AddRemote {
            name: "local".into(),
            url: f.0.join("b").to_str().unwrap().into(),
        },
    )
    .unwrap();
    let repo = svc.identify(&root, &cancel).unwrap();
    let remotes = svc.remotes(&repo, &cancel).unwrap();
    let old = remotes[0].fetch_urls[0].clone();
    git(
        &root,
        &[
            "config",
            "remote.local.pushurl",
            f.0.join("a").to_str().unwrap(),
        ],
    );
    write(
        &svc,
        &root,
        WriteOperation::SetRemoteUrl {
            name: "local".into(),
            expected_url: old.clone(),
            url: f.0.to_str().unwrap().into(),
        },
    )
    .unwrap();
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::SetRemoteUrl {
                name: "local".into(),
                expected_url: old,
                url: "nope".into()
            }
        )
        .is_err()
    );
    assert_eq!(
        svc.remotes(&repo, &cancel).unwrap()[0].push_urls,
        [root.to_str().unwrap()]
    );
    git(&root, &["branch", "topic"]);
    let oid = git(&root, &["rev-parse", "topic"]).trim().to_string();
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::RenameBranch {
                name: "topic".into(),
                new_name: "bad".into(),
                oid: "0".repeat(40)
            }
        )
        .is_err()
    );
    write(
        &svc,
        &root,
        WriteOperation::RenameBranch {
            name: "topic".into(),
            new_name: "renamed".into(),
            oid: oid.clone(),
        },
    )
    .unwrap();
    let linked = f.0.join("linked");
    write(
        &svc,
        &root,
        WriteOperation::AddWorktree {
            path: linked.clone(),
            branch: "renamed".into(),
        },
    )
    .unwrap();
    let trees = svc.worktrees(&repo, &cancel).unwrap();
    assert_eq!(trees.len(), 2);
    let tree = trees
        .into_iter()
        .find(|t| t.path == linked.canonicalize().unwrap())
        .unwrap();
    fs::write(linked.join("keep.txt"), "keep").unwrap();
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::RemoveWorktree {
                path: tree.path.clone(),
                head: tree.head.clone()
            }
        )
        .is_err()
    );
    assert!(linked.join("keep.txt").exists());
    fs::remove_file(linked.join("keep.txt")).unwrap();
    write(
        &svc,
        &root,
        WriteOperation::RemoveWorktree {
            path: tree.path,
            head: tree.head,
        },
    )
    .unwrap();
    assert!(!linked.exists());
    write(
        &svc,
        &root,
        WriteOperation::DeleteBranch {
            name: "renamed".into(),
            oid,
        },
    )
    .unwrap();
    git(&root, &["switch", "-c", "unmerged"]);
    git(&root, &["commit", "--allow-empty", "-m", "keep"]);
    let oid = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    git(&root, &["switch", "main"]);
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::DeleteBranch {
                name: "unmerged".into(),
                oid
            }
        )
        .is_err()
    );
}
#[test]
fn init_clone_and_operation_recovery_stay_local() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    commit(&root);
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let initialized = svc
        .create_repository(&f.0.join("new"), None, &cancel)
        .unwrap();
    assert!(
        svc.create_repository(&initialized.worktree, None, &cancel)
            .is_err()
    );
    let dangling = f.0.join("dangling");
    std::os::unix::fs::symlink(f.0.join("missing"), &dangling).unwrap();
    assert!(svc.create_repository(&dangling, None, &cancel).is_err());
    assert!(fs::symlink_metadata(&dangling).unwrap().is_symlink());
    assert!(!f.0.join("missing").exists());
    let cloned = svc
        .create_repository(&f.0.join("clone"), Some(root.to_str().unwrap()), &cancel)
        .unwrap();
    assert_eq!(
        git(&cloned.worktree, &["show", "HEAD:src/main.rs"]),
        "original\n"
    );
    git(&root, &["switch", "-c", "topic"]);
    fs::write(root.join("src/main.rs"), "topic\n").unwrap();
    commit(&root);
    git(&root, &["switch", "main"]);
    fs::write(root.join("src/main.rs"), "main\n").unwrap();
    commit(&root);
    let conflict = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["merge", "topic"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    let repo = svc.identify(&root, &cancel).unwrap();
    assert_eq!(
        svc.in_progress(&repo, &cancel).unwrap(),
        Some(InProgress::Merge)
    );
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::ContinueOperation {
                operation: InProgress::Rebase
            }
        )
        .is_err()
    );
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::ContinueOperation {
                operation: InProgress::Merge
            }
        )
        .is_err()
    );
    write(
        &svc,
        &root,
        WriteOperation::AbortOperation {
            operation: InProgress::Merge,
        },
    )
    .unwrap();
    assert_eq!(svc.in_progress(&repo, &cancel).unwrap(), None);
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "main\n"
    );
    let conflict = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["merge", "topic"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    fs::write(root.join("src/main.rs"), "resolved\n").unwrap();
    git(&root, &["add", "."]);
    write(
        &svc,
        &root,
        WriteOperation::ContinueOperation {
            operation: InProgress::Merge,
        },
    )
    .unwrap();
    assert_eq!(svc.in_progress(&repo, &cancel).unwrap(), None);
}
#[test]
fn submodule_clone_uses_the_confirmed_expanded_url() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    let source = f.0.join("b");
    commit(&root);
    commit(&source);
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.to_str().unwrap(),
            "module",
        ],
    );
    commit(&root);
    git(&root, &["submodule", "deinit", "-f", "--", "module"]);
    fs::remove_dir_all(root.join(".git/modules/module")).unwrap();
    git(
        &root,
        &["config", "submodule.module.url", "fixture-sub-alias"],
    );
    git(
        &root,
        &[
            "config",
            &format!("url.{}.insteadOf", source.display()),
            "fixture-sub-alias",
        ],
    );
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = svc.identify(&root, &cancel).unwrap();
    let module = svc.submodules(&repo, &cancel).unwrap().remove(0);
    assert_eq!(module.url, source.to_str().unwrap());
    write(
        &svc,
        &root,
        WriteOperation::UpdateSubmodule {
            path: module.path,
            expected_url: module.url,
        },
    )
    .unwrap();
    assert!(root.join("module/src/main.rs").is_file());
}

#[test]
fn submodule_uninitialized_update_and_lfs_boundaries() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    commit(&root);
    let sub = f.0.join("b");
    commit(&sub);
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub.to_str().unwrap(),
            "module",
        ],
    );
    commit(&root);
    git(&root, &["submodule", "deinit", "-f", "--", "module"]);
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = svc.identify(&root, &cancel).unwrap();
    assert!(!svc.submodules(&repo, &cancel).unwrap()[0].initialized);
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::UpdateSubmodule {
                path: "module".into(),
                expected_url: "different-source".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("来源地址已变化")
    );
    // The object store already exists: initialization needs no connection or protocol override.
    write(
        &svc,
        &root,
        WriteOperation::UpdateSubmodule {
            path: "module".into(),
            expected_url: sub.to_str().unwrap().into(),
        },
    )
    .unwrap();
    assert!(svc.submodules(&repo, &cancel).unwrap()[0].initialized);
    let module = root.join("module");
    let changed_url = f.0.join("changed-source.git");
    git(
        &module,
        &["remote", "set-url", "origin", changed_url.to_str().unwrap()],
    );
    assert_eq!(
        svc.submodules(&repo, &cancel).unwrap()[0].url,
        changed_url.to_str().unwrap()
    );
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::UpdateSubmodule {
                path: "module".into(),
                expected_url: sub.to_str().unwrap().into(),
            }
        )
        .unwrap_err()
        .to_string()
        .contains("来源地址已变化")
    );
    git(
        &module,
        &[
            "config",
            &format!("url.{}.insteadOf", sub.display()),
            "fixture-sub",
        ],
    );
    git(&module, &["remote", "set-url", "origin", "fixture-sub"]);
    assert_eq!(
        svc.submodules(&repo, &cancel).unwrap()[0].url,
        sub.to_str().unwrap()
    );
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::UpdateSubmodule {
                path: "src/main.rs".into(),
                expected_url: String::new(),
            }
        )
        .is_err()
    );
    let pointer = format!(
        "version https://git-lfs.github.com/spec/v1\noid sha256:{}\nsize 123\n",
        "a".repeat(64)
    );
    assert_eq!(parse_lfs_pointer(pointer.as_bytes()).unwrap().size, 123);
    assert!(parse_lfs_pointer(pointer.replace("sha256:", "sha256:../").as_bytes()).is_none());
    assert_eq!(
        redact_git_output("To https://user:secret@example.invalid/repo?token=private\n"),
        "To https://[redacted]@example.invalid/repo[redacted]\n"
    );
    assert_eq!(
        redact_git_output("https://user:raw?fragment@example.invalid/repo"),
        "https://[redacted]@example.invalid/repo"
    );
    let multiple = redact_git_output(
        "https://one:secret1@example.invalid/a、https://two:secret2@example.invalid/b?token=private",
    );
    assert!(
        !multiple.contains("one:") && !multiple.contains("two:") && !multiple.contains("private")
    );
    assert_eq!(
        push_result_summary("!\tHEAD:refs/heads/main\t[rejected]\n"),
        "推送失败或部分完成"
    );
}
#[test]
fn rebase_and_cherry_pick_can_abort_and_continue_after_conflicts() {
    common::hermetic();
    for operation in [InProgress::Rebase, InProgress::CherryPick] {
        let f = fixture();
        let root = f.0.join("a");
        commit(&root);
        git(&root, &["switch", "-c", "topic"]);
        fs::write(root.join("src/main.rs"), "topic\n").unwrap();
        commit(&root);
        git(&root, &["switch", "main"]);
        fs::write(root.join("src/main.rs"), "main\n").unwrap();
        commit(&root);
        let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
        let cancel = AtomicBool::new(false);
        let repo = svc.identify(&root, &cancel).unwrap();
        for abort in [true, false] {
            let conflict = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([operation.label(), "topic"])
                .output()
                .unwrap();
            assert!(!conflict.status.success());
            assert_eq!(svc.in_progress(&repo, &cancel).unwrap(), Some(operation));
            if abort {
                write(&svc, &root, WriteOperation::AbortOperation { operation }).unwrap();
                assert_eq!(
                    fs::read_to_string(root.join("src/main.rs")).unwrap(),
                    "main\n"
                );
            } else {
                fs::write(root.join("src/main.rs"), "resolved\n").unwrap();
                git(&root, &["add", "."]);
                write(&svc, &root, WriteOperation::ContinueOperation { operation }).unwrap();
                assert_eq!(
                    fs::read_to_string(root.join("src/main.rs")).unwrap(),
                    "resolved\n"
                );
            }
            assert_eq!(svc.in_progress(&repo, &cancel).unwrap(), None);
        }
    }
}
#[test]
fn external_patch_checks_exact_file_under_write_lock() {
    common::hermetic();
    let f = fixture();
    let root = f.0.join("a");
    commit(&root);
    let svc = GitService::new(2, Duration::from_secs(10)).unwrap();
    let patch = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-original\n+patched\n";
    let new_file = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+new\n";
    write(
        &svc,
        &root,
        WriteOperation::ApplyExternalPatch {
            path: "new.txt".into(),
            patch: new_file.into(),
        },
    )
    .unwrap();
    assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(), "new\n");
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::ApplyExternalPatch {
                path: "link.txt".into(),
                patch: new_file
                    .replace("new.txt", "link.txt")
                    .replace("100644", "120000")
            }
        )
        .is_err()
    );
    assert!(!root.join("link.txt").exists());
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::ApplyExternalPatch {
                path: "other.rs".into(),
                patch: patch.into()
            }
        )
        .is_err()
    );
    write(
        &svc,
        &root,
        WriteOperation::ApplyExternalPatch {
            path: "src/main.rs".into(),
            patch: patch.into(),
        },
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "patched\n"
    );
    assert!(
        write(
            &svc,
            &root,
            WriteOperation::ApplyExternalPatch {
                path: "src/main.rs".into(),
                patch: format!("{patch}{patch}")
            }
        )
        .is_err()
    );
}
