//! Source Control in a headless window, on a temporary repository.

use super::super::test_support::{open, settle};
use super::super::*;
// `gpui_kit::*` also exports a `test` macro; `#[gpui_kit::test]` expands to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{InputEvent as _, TestAppContext, test::TestWindowExt};

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
async fn repositories_keep_name_order_after_changes_and_commits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("name-order", 0);
    let base = repo.parent().unwrap().to_path_buf();
    // The parent paths deliberately sort differently from the displayed repository names.
    let zeta = base.join("a-parent/zeta");
    let alpha = base.join("z-parent/alpha");
    let other_alpha = base.join("m-parent/alpha");
    for path in [&zeta, &alpha, &other_alpha] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    std::fs::rename(repo, &zeta).unwrap();
    for path in [&alpha, &other_alpha] {
        git(
            &base,
            &[
                "clone",
                "-q",
                base.join("remote.git").to_str().unwrap(),
                path.to_str().unwrap(),
            ],
        );
    }
    let (window, this) = open(cx, base.clone());
    let order = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.rows
                .iter()
                .filter_map(|row| match row {
                    Row::Group(g) => Some(p.groups[*g].repo.worktree.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
    };
    let refresh = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.refresh(window, cx));
        })
        .unwrap();
        settle(cx, None, |cx| this.read_with(cx, |p, _| !p.loading));
    };
    let expected = vec![other_alpha, alpha, zeta.clone()];
    assert_eq!(order(cx), expected);

    std::fs::write(zeta.join("a.txt"), "changed\n").unwrap();
    refresh(cx);
    assert_eq!(order(cx), expected);
    this.update(cx, |p, _| {
        p.hide_clean_repos = true;
        p.rebuild_rows();
    });
    assert_eq!(order(cx).as_slice(), std::slice::from_ref(&zeta));
    this.update(cx, |p, _| {
        p.hide_clean_repos = false;
        p.rebuild_rows();
    });

    git(&zeta, &["commit", "-q", "-am", "local change"]);
    refresh(cx);
    this.read_with(cx, |p, _| {
        let group = p.groups.iter().find(|g| g.repo.worktree == zeta).unwrap();
        assert!(group.clean());
        assert_eq!(group.ahead(), 1);
    });
    assert_eq!(order(cx), expected);
    let _ = std::fs::remove_dir_all(base);
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
        assert!(p.groups[0].clean());
        assert!(p.scm_push(0).is_some());
        p.groups[0].outgoing_collapsed = true;
        p.rebuild_rows();
        cx.notify();
    });
    assert_eq!(rows(cx, &this), ["repo", "message", "outgoing"]);
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn a_clean_repository_keeps_its_commit_controls(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("synced", 0);
    let (_, this) = open(cx, repo.clone());
    assert_eq!(rows(cx, &this), ["repo", "message"]);
    this.update(cx, |p, _| {
        p.groups[0].expanded = false;
        p.rebuild_rows();
    });
    assert_eq!(rows(cx, &this), ["repo"]);
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

#[test]
fn commit_message_diff_matches_the_next_commit_without_staging() {
    let repo = fixture("message-diff", 0);
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    let cancel = AtomicBool::new(false);
    let identity = service.identify(&repo, &cancel).unwrap();
    assert!(
        service
            .commit_message_diff(&identity, &cancel)
            .unwrap_err()
            .to_string()
            .contains("没有")
    );
    std::fs::write(repo.join("a.txt"), "staged-content\n").unwrap();
    std::fs::write(repo.join("new.txt"), "untracked-content\n").unwrap();
    let before = service.status(&identity, 0, &cancel).unwrap();
    let (status, diff) = service.commit_message_diff(&identity, &cancel).unwrap();
    assert_eq!(before, status);
    assert!(diff.contains("+staged-content") && diff.contains("+untracked-content"));
    assert_eq!(before, service.status(&identity, 0, &cancel).unwrap());
    git(&repo, &["add", "--", "a.txt"]);
    std::fs::write(repo.join("a.txt"), "unstaged-content\n").unwrap();
    let (_, diff) = service.commit_message_diff(&identity, &cancel).unwrap();
    assert!(diff.contains("+staged-content"));
    assert!(!diff.contains("unstaged-content") && !diff.contains("untracked-content"));
    std::fs::write(repo.join("a.txt"), "x".repeat(140 * 1024)).unwrap();
    git(&repo, &["add", "--", "a.txt"]);
    assert!(
        service
            .commit_message_diff(&identity, &cancel)
            .unwrap_err()
            .to_string()
            .contains("128 KiB")
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn ai_message_fills_the_repository_input_and_manual_edits_cancel(cx: &mut TestAppContext) {
    use workspace_editor_agent::registry::{EnvValue, UserAgentConfig};
    cx.executor().allow_parking();
    let repo = fixture("ai-message", 0);
    std::fs::write(repo.join("a.txt"), "edited\n").unwrap();
    let (window, this) = open(cx, repo.clone());
    let program = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("zj-fake-acp-agent");
    assert!(
        program.exists(),
        "run cargo test --workspace to build the fake agent"
    );
    let mut preset = UserAgentConfig {
        id: "fake-commit".into(),
        name: Some("Fake".into()),
        command: program.display().to_string(),
        args: Vec::new(),
        env: Default::default(),
    }
    .into_preset();
    preset.env.push((
        "FAKE_PROMPT".into(),
        EnvValue::Literal("echo feat: 生成提交信息".into()),
    ));
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent.agent_id = preset.id.clone();
            p.agent.presets = vec![preset];
            p.sidebar = Sidebar::SourceControl;
            p.scm_generate_message(p.groups[0].repo.id.clone(), window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.groups[0].commit_generation.is_none())
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.groups[0].commit_input.read(cx).value().as_ref(),
            "feat: 生成提交信息",
            "{}",
            p.groups[0].write_message
        );
    });
    assert_eq!(git_out(&repo, &["log", "-1", "--format=%s"]), "pushed");
    assert_eq!(git_out(&repo, &["diff", "--cached"]), "");

    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent.presets[0].env =
                vec![("FAKE_PROMPT".into(), EnvValue::Literal("stuck".into()))];
            p.groups[0]
                .commit_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            p.scm_generate_message(p.groups[0].repo.id.clone(), window, cx);
            p.groups[0].commit_input.focus_handle(cx).focus(window, cx);
        });
        window.render_frame(cx);
        window.input("我的消息", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, cx| {
        assert!(p.groups[0].commit_generation.is_none());
        assert_eq!(
            p.groups[0].commit_input.read(cx).value().as_ref(),
            "我的消息"
        );
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
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
async fn the_line_end_blames_the_cursor_line(cx: &mut TestAppContext) {
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
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let annotation = window
            .try_find("line-blame")
            .expect("initial line annotation")
            .bounds();
        let end = this.read(cx).documents[0]
            .editor
            .read(cx)
            .range_to_bounds(&(1..1))
            .unwrap();
        assert!((annotation.origin.x - end.origin.x - theme::BLAME_GAP).abs() <= px(1.));
        assert!((annotation.origin.y - end.origin.y).abs() <= px(1.));
        // Only the annotation is clickable; the remaining blank space still belongs
        // to the editor, so clicking past a short line can place the caret there.
        assert!(
            annotation.right() + px(100.)
                < this.read(cx).documents[0]
                    .editor
                    .read(cx)
                    .input_bounds()
                    .right()
        );
        assert!(window.try_find("status-blame").is_none());
        let before = this.read(cx).documents[0]
            .editor
            .read(cx)
            .text()
            .to_string();
        window.click("line-blame", cx);
        let expected = this
            .read(cx)
            .blame
            .shown
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .commit
            .clone();
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), expected);
        assert_eq!(
            this.read(cx).documents[0]
                .editor
                .read(cx)
                .text()
                .to_string(),
            before
        );
    })
    .unwrap();

    // The empty line after the last line break: no blame, and no error either.
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_cursor_position(lsp_types::Position::new(1, 0), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| shown(cx).is_none());
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(shown(cx), None);
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("line-blame").is_none());
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_cursor_position(lsp_types::Position::new(0, 0), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| shown(cx).is_some());

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

    // The annotation follows the last visual segment of a wrapped Unicode line.
    let long = "世界🙂 ".repeat(36);
    let contents = format!("{long}\n{}", "short\n".repeat(80));
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_all(contents, window, cx);
            state.set_soft_wrap(true, window, cx);
            state.set_cursor_position(lsp_types::Position::new(0, 0), window, cx);
        });
    })
    .unwrap();
    // Wait for the edit event and its new blame query, not the old shown result.
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    settle(cx, Some(window), |cx| {
        matches!(shown(cx), Some(Ok((_, true))))
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        let annotation = window
            .try_find("line-blame")
            .expect("wrapped line annotation")
            .bounds();
        let end = editor
            .read(cx)
            .range_to_bounds(&(long.len()..long.len()))
            .unwrap();
        assert!((annotation.origin.x - end.origin.x - theme::BLAME_GAP).abs() <= px(1.));
        assert!((annotation.origin.y - end.origin.y).abs() <= px(1.));
        assert!(annotation.top() > editor.read(cx).input_bounds().top());

        // A manual vertical scroll must immediately hide the offscreen annotation.
        editor.update(cx, |state, cx| {
            state.set_scroll_offset(point(px(0.), px(-600.)), cx)
        });
        window.render_frame(cx);
        assert!(window.try_find("line-blame").is_none());

        // With wrapping off, a long line's end outside the viewport stays hidden.
        editor.update(cx, |state, cx| {
            state.set_soft_wrap(false, window, cx);
            state.set_scroll_offset(point(px(0.), px(0.)), cx);
        });
        window.render_frame(cx);
        assert!(window.try_find("line-blame").is_none());
        // The longest line must make room for its own annotation, without another
        // wider line granting extra horizontal scroll space.
        editor.update(cx, |state, cx| {
            state.set_cursor_position(
                lsp_types::Position::new(0, long.encode_utf16().count() as u32),
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window.dispatch_event(
            ScrollWheelEvent {
                position: editor.read(cx).input_bounds().center(),
                delta: ScrollDelta::Pixels(point(px(-10000.), px(0.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        let annotation = window
            .try_find("line-blame")
            .expect("horizontally scrolled line annotation")
            .bounds();
        let end = editor
            .read(cx)
            .range_to_bounds(&(long.len()..long.len()))
            .unwrap();
        assert!((annotation.origin.x - end.origin.x - theme::BLAME_GAP).abs() <= px(1.));
        assert!((annotation.origin.y - end.origin.y).abs() <= px(1.));
        assert!(annotation.right() <= editor.read(cx).input_bounds().right());
        assert!(
            annotation.size.width > theme::BLAME_GAP,
            "the full annotation is scrollable"
        );
        assert_eq!(
            editor.read(cx).text().to_string().lines().next(),
            Some(long.as_str())
        );
    })
    .unwrap();
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
