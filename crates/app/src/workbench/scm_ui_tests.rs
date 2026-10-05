//! Source Control in a headless window, on a temporary repository.

use super::super::test_support::{open, settle};
use super::super::*;
// `gpui_kit::*` also exports a `test` macro; `#[gpui_kit::test]` expands to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A clean clone with `ahead` commits its upstream does not have.
fn fixture(name: &str, ahead: usize) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zj-scm-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    let remote = base.join("remote.git");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&remote).unwrap();
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
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "pushed"]);
    git(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    for n in 1..=ahead {
        git(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", &format!("local {n}")],
        );
    }
    std::fs::canonicalize(repo).unwrap()
}

fn rows(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .map(|row| match *row {
                Row::Group(_) => "repo".to_string(),
                Row::Commit(_) => "message".to_string(),
                Row::Heading(_, _, n) => format!("changes {n}"),
                Row::File(..) => "file".to_string(),
                Row::Outgoing(_) => "outgoing".to_string(),
                Row::OutgoingCommit(g, i) => {
                    format!(
                        "commit {}",
                        p.groups[g].outgoing.as_ref().unwrap()[i].subject
                    )
                }
                Row::OutgoingNote(_) => "note".to_string(),
            })
            .collect()
    })
}

#[gpui_kit::test]
async fn a_clean_repository_with_unpushed_commits_lists_them(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("ahead", 2);
    let (_, this) = open(cx, repo.clone());
    assert_eq!(
        rows(cx, &this),
        [
            "repo",
            "message",
            "outgoing",
            "commit local 2",
            "commit local 1"
        ]
    );
    this.update(cx, |p, cx| {
        assert!(p.groups[0].clean() && p.groups[0].expandable());
        assert!(p.scm_push(0).is_some());
        p.groups[0].outgoing_collapsed = true;
        p.rebuild_rows();
        cx.notify();
    });
    assert_eq!(rows(cx, &this), ["repo", "message", "outgoing"]);
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn a_clean_repository_in_sync_does_not_open(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("synced", 0);
    let (_, this) = open(cx, repo.clone());
    assert_eq!(rows(cx, &this), ["repo"]);
    this.read_with(cx, |p, _| assert!(!p.groups[0].expandable()));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn the_branch_button_checks_out_and_creates_branches(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("branches", 0);
    let other = repo.parent().unwrap().join("other");
    let bare = repo.parent().unwrap().join("remote.git");
    git(
        repo.parent().unwrap(),
        &[
            "clone",
            "-q",
            "-b",
            "main",
            bare.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    git(&other, &["switch", "-q", "-c", "feature"]);
    git(&other, &["push", "-q", "origin", "feature"]);
    git(&repo, &["switch", "-q", "-c", "local"]);
    git(&repo, &["switch", "-q", "main"]);
    git(&repo, &["fetch", "-q", "--prune"]);
    let (window, this) = open(cx, repo.clone());

    // The branch picker: 创建新分支… first, then local branches, then remote ones.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_branch_picker(0, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.quick_open
                .as_ref()
                .is_some_and(|q| q.items.as_ref().is_some_and(|(items, _)| !items.is_empty()))
        })
    });
    let items: Vec<String> = this.read_with(cx, |p, _| {
        p.quick_open
            .as_ref()
            .unwrap()
            .items
            .as_ref()
            .unwrap()
            .0
            .iter()
            .map(|item| format!("{}|{}", item.label, item.detail))
            .collect()
    });
    assert!(items[0].starts_with("|"));
    let branches: Vec<&str> = items[1..]
        .iter()
        .map(|i| i.split('|').next().unwrap())
        .collect();
    assert_eq!(branches, ["local", "main", "origin/feature", "origin/main"]);
    assert!(items.iter().any(|i| i.contains("当前分支")));
    assert!(items.iter().any(|i| i.contains("远程分支")));

    // The query filters (input events land between window updates); confirming
    // "origin/feature" creates the tracking local branch.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("feat", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let filtered: Vec<String> = this.read_with(cx, |p, _| {
        let (items, filtered) = p.quick_open.as_ref().unwrap().items.as_ref().unwrap();
        filtered.iter().map(|i| items[*i].label.clone()).collect()
    });
    assert_eq!(filtered, ["origin/feature", ""]);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.groups[0]
                .status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| s.branch.as_deref() == Some("feature"))
        })
    });
    assert_eq!(git_out(&repo, &["branch", "--show-current"]), "feature");

    // 创建分支… asks for the name, then creates and switches to it.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_create_branch(0, window, cx));
        window.render_frame(cx);
        window.input("topic", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.groups[0]
                .status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| s.branch.as_deref() == Some("topic"))
        })
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
