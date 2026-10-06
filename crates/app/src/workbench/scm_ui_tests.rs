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

#[gpui_kit::test]
async fn stash_with_a_message_then_pop_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("stash", 0);
    std::fs::write(repo.join("a.txt"), "edited\n").unwrap();
    let (window, this) = open(cx, repo.clone());
    let changes = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.groups[0]
                .status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .map(|s| s.changes.len())
        })
    };
    settle(cx, None, |cx| changes(cx) == Some(1));

    // Stash…: type the message, Enter on the first row (tracked changes only).
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_stash_push(0, window, cx));
        window.render_frame(cx);
        window.input("半成品", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, None, |cx| changes(cx) == Some(0));
    assert!(git_out(&repo, &["stash", "list"]).contains("半成品"));

    // 弹出 Stash…: the list shows it; Enter pops it back.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_stash_picker(0, super::super::quick_open::StashAction::Pop, window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.quick_open
                .as_ref()
                .and_then(|q| q.items.as_ref())
                .is_some_and(|(items, _)| items.len() == 1 && items[0].label.ends_with("半成品"))
        })
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, None, |cx| changes(cx) == Some(1));
    assert_eq!(git_out(&repo, &["stash", "list"]), "");
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "edited\n"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn the_status_bar_blames_the_cursor_line(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("blame", 0);
    let (window, this) = open(cx, repo.clone());
    let folder = Some(repo.clone());
    let path = repo.join("a.txt");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    let shown = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.blame.shown.as_ref().map(|b| {
                b.as_ref()
                    .map(|b| (b.author.clone(), b.uncommitted))
                    .map_err(|e| e.clone())
            })
        })
    };
    settle(cx, Some(window), |cx| shown(cx).is_some());
    assert_eq!(shown(cx), Some(Ok(("Fixture".to_string(), false))));

    // Typing on the line: blamed as typed, so not committed yet.
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "x", window, cx)
        });
    })
    .unwrap();
    // (Git names the author of such lines differently across versions.)
    settle(cx, Some(window), |cx| {
        matches!(shown(cx), Some(Ok((_, true))))
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn merge_conflicts_are_resolved_from_the_bar_and_marked_resolved(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("conflict", 0);
    std::fs::write(repo.join("a.txt"), "top\nbase 1\nm1\nm2\nm3\nm4\nbase 2\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "base"]);
    git(&repo, &["switch", "-q", "-c", "feature"]);
    std::fs::write(
        repo.join("a.txt"),
        "top\nfeature 1\nm1\nm2\nm3\nm4\nfeature 2\n",
    )
    .unwrap();
    git(&repo, &["commit", "-q", "-am", "feature"]);
    git(&repo, &["switch", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "top\nmain 1\nm1\nm2\nm3\nm4\nmain 2\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "main"]);
    let merge = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "merge", "-q", "feature"])
        .output()
        .unwrap();
    assert!(!merge.status.success(), "the merge must conflict");

    let (window, this) = open(cx, repo.clone());
    let folder = Some(repo.clone());
    let path = repo.join("a.txt");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    let count = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| p.documents.first().map(|d| d.conflicts_found()))
    };
    settle(cx, Some(window), |cx| count(cx) == Some(Some(2)));

    // The first conflict (the cursor is at the top): keep both; the rest: take theirs.
    let resolve = |cx: &mut TestAppContext, choice, all| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.resolve_conflict(choice, all, window, cx));
        })
        .unwrap();
    };
    resolve(cx, crate::conflicts::Choice::Both, false);
    settle(cx, Some(window), |cx| count(cx) == Some(Some(1)));
    resolve(cx, crate::conflicts::Choice::Theirs, true);
    settle(cx, Some(window), |cx| count(cx) == Some(Some(0)));
    let text = this.read_with(cx, |p, cx| {
        p.documents[0].editor.read(cx).text().to_string()
    });
    assert_eq!(text, "top\nmain 1\nfeature 1\nm1\nm2\nm3\nm4\nfeature 2\n");

    // 标记为已解决: saved and staged, so Git no longer lists a conflict.
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.mark_resolved(id, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.groups[0]
                .status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| {
                    s.changes
                        .iter()
                        .all(|c| c.kind != workspace_editor_git::ChangeKind::Conflict)
                })
        })
    });
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), text);
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn repositories_added_by_hand_are_listed_and_can_be_removed(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-scm-extra-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    // Deeper than discovery's 4 levels.
    let deep = base.join("a/b/c/d/e/repo");
    std::fs::create_dir_all(&deep).unwrap();
    git(&deep, &["init", "-q", "-b", "main"]);
    let base = std::fs::canonicalize(base).unwrap();
    let deep = std::fs::canonicalize(deep).unwrap();
    let settings = crate::settings::Settings {
        extra_repos: [(
            base.to_string_lossy().into_owned(),
            vec![
                deep.to_string_lossy().into_owned(),
                base.join("gone").to_string_lossy().into_owned(),
            ],
        )]
        .into(),
        ..Default::default()
    };
    let (window, this) = super::super::test_support::open_window(
        cx,
        Some(base.clone()),
        settings,
        super::super::test_support::empty_store(),
    );
    settle(cx, None, |cx| super::super::test_support::loaded(cx, &this));
    this.read_with(cx, |p, _| {
        assert_eq!(p.groups.len(), 1);
        assert_eq!(p.groups[0].repo.worktree, deep);
        // The one that is gone is reported, not silently dropped.
        assert!(
            p.issues.iter().any(|i| i.contains("gone")),
            "{:?}",
            p.issues
        );
    });
    this.read_with(cx, |p, cx| assert!(p.is_extra_repo(0, cx)));
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.remove_extra_repository(0, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.refresh_completed && !p.loading && p.groups.is_empty()
        })
    });
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui_kit::test]
async fn staging_a_file_that_still_has_conflict_markers_asks_first(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("stage-conflict", 0);
    std::fs::write(repo.join("a.txt"), "base\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "base"]);
    git(&repo, &["switch", "-q", "-c", "feature"]);
    std::fs::write(repo.join("a.txt"), "feature\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "feature"]);
    git(&repo, &["switch", "-q", "main"]);
    std::fs::write(repo.join("a.txt"), "main\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "main"]);
    let merge = std::process::Command::new("git")
        .args(["-C", repo.to_str().unwrap(), "merge", "-q", "feature"])
        .output()
        .unwrap();
    assert!(!merge.status.success());
    let (window, this) = open(cx, repo.clone());
    let conflicted = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.groups[0]
                .status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| {
                    s.changes
                        .iter()
                        .any(|c| c.kind == workspace_editor_git::ChangeKind::Conflict)
                })
        })
    };
    settle(cx, None, conflicted);
    // 暂存所有更改: the file still has markers, so it asks; 取消 leaves it conflicted.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let request = p
                .scm_paths(0, None, workspace_editor_git::DiffSide::Worktree)
                .unwrap();
            p.request_git_write(request, window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(conflicted(cx));
    assert!(git_out(&repo, &["diff", "--name-only", "--diff-filter=U"]).contains("a.txt"));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
