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

/// Advance only the temporary bare remote, then load its tracking ref in the local clone.
fn incoming_fixture(name: &str, ahead: usize) -> PathBuf {
    let repo = fixture(name, ahead);
    let base = repo.parent().unwrap();
    let writer = base.join("writer");
    git(base, &["clone", "-q", "-b", "main", "remote.git", "writer"]);
    git(&writer, &["config", "user.name", "Fixture"]);
    git(
        &writer,
        &["config", "user.email", "fixture@example.invalid"],
    );
    git(&writer, &["config", "commit.gpgsign", "false"]);
    git(
        &writer,
        &["commit", "-q", "--allow-empty", "-m", "incoming"],
    );
    git(&writer, &["push", "-q", "origin", "main"]);
    git(&repo, &["fetch", "-q", "origin"]);
    git(&repo, &["config", "pull.rebase", "true"]);
    repo
}

#[gpui_kit::test]
async fn incoming_commits_have_an_explicit_pull_action(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = incoming_fixture("incoming-pull", 0);
    let (window, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        p.hide_clean_repos = true;
        p.rebuild_rows();
        cx.notify();
    });
    assert_eq!(rows(cx, &this), ["repo", "message"]);
    this.read_with(cx, |p, _| {
        let status = p.groups[0].status.as_ref().unwrap().as_ref().unwrap();
        assert!(status.changes.is_empty());
        assert_eq!((status.behind, status.ahead), (1, 0));
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(("scm-repo-pull", 0usize)).is_some(),
            "incoming commits need an explicit pull action"
        );
        assert!(window.try_find(("scm-repo-sync", 0usize)).is_none());
        let pull = window.find(("scm-repo-pull", 0usize));
        assert!(pull.visible());
        assert_eq!(pull.role(), Some(Role::Button));
        assert_eq!(pull.label(), Some("从 origin/main 拉取 1 个提交"));
        window.click(("scm-repo-pull", 0usize), cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            !p.groups[0].write_pending
                && p.groups[0]
                    .status
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .behind
                    == 0
        })
    });
    assert_eq!(
        git_out(&repo, &["rev-parse", "HEAD"]),
        git_out(&repo, &["rev-parse", "origin/main"])
    );
    assert!(
        rows(cx, &this).is_empty(),
        "fully synced clean repository should hide again"
    );
    this.update(cx, |p, cx| {
        p.hide_clean_repos = false;
        p.rebuild_rows();
        cx.notify();
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("scm-repo-pull", 0usize)).is_none());
        assert!(window.try_find(("scm-repo-sync", 0usize)).is_some());
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn incoming_pull_keeps_local_commits_without_pushing(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = incoming_fixture("incoming-diverged", 1);
    let remote = repo.parent().unwrap().join("remote.git");
    let remote_tip = git_out(&remote, &["rev-parse", "main"]);
    let (window, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    this.read_with(cx, |p, _| {
        let status = p.groups[0].status.as_ref().unwrap().as_ref().unwrap();
        assert_eq!((status.behind, status.ahead), (1, 1));
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("scm-repo-pull", 0usize), cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            let status = p.groups[0].status.as_ref().unwrap().as_ref().unwrap();
            !p.groups[0].write_pending && (status.behind, status.ahead) == (0, 1)
        })
    });
    assert_eq!(git_out(&remote, &["rev-parse", "main"]), remote_tip);
    assert_eq!(git_out(&repo, &["log", "-1", "--format=%s"]), "local 1");
    assert_eq!(
        git_out(&repo, &["rev-list", "--count", "HEAD..origin/main"]),
        "0"
    );
    assert_eq!(
        git_out(&repo, &["rev-list", "--count", "origin/main..HEAD"]),
        "1"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

/// Render the production SCM panel at the sidebar's minimum width.
struct NarrowScm(Entity<Workbench>);

impl Render for NarrowScm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |p, cx| p.render_scm(cx))
    }
}

#[gpui_kit::test]
async fn incoming_pull_stays_visible_when_narrow_and_respects_busy_and_upstream(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let repo = incoming_fixture("incoming-narrow", 0);
    let (handle, this) = cx.update(|cx| {
        let documents = super::super::test_support::install_globals(
            cx,
            crate::settings::Settings::default(),
            super::super::test_support::empty_store(),
        );
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let mut this = None;
        let (handle, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(theme::SIDEBAR_MIN, px(640.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let workbench = cx.new(|cx| {
                    Workbench::new(Some(repo.clone()), service, documents, 1, window, cx)
                });
                this = Some(workbench.clone());
                cx.new(|_| NarrowScm(workbench))
            },
        )
        .unwrap();
        (handle, this.unwrap())
    });
    settle(cx, None, |cx| super::super::test_support::loaded(cx, &this));
    for busy in [false, true] {
        this.update(cx, |p, cx| {
            p.groups[0].write_pending = busy;
            cx.notify();
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let pull = window.find(("scm-repo-pull", 0usize));
            assert!(pull.visible());
            assert!(pull.bounds().left() >= px(0.));
            assert!(pull.bounds().right() <= window.viewport_size().width);
            assert!(pull.bounds().size.width > px(0.));
            assert_eq!(pull.label(), Some("从 origin/main 拉取 1 个提交"));
            if busy {
                window.click(("scm-repo-pull", 0usize), cx);
            }
        })
        .unwrap();
    }
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(p.groups[0].expanded, "disabled pull toggled its repository");
        assert_eq!(p.write_generation, 0, "busy pull started a write");
        assert!(p.groups[0].write_task.is_none());
        assert_eq!(
            p.groups[0]
                .status
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .behind,
            1
        );
    });
    assert_eq!(
        git_out(&repo, &["rev-list", "--count", "HEAD..origin/main"]),
        "1"
    );
    assert!(!cx.has_pending_prompt());
    this.update(cx, |p, cx| {
        p.groups[0].write_pending = false;
        let status = p.groups[0].status.as_mut().unwrap().as_mut().unwrap();
        Arc::make_mut(status).upstream = None;
        cx.notify();
    });
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("scm-repo-pull", 0usize)).is_none());
        assert!(window.try_find(("scm-repo-sync", 0usize)).is_none());
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn remote_check_discovers_incoming_and_does_not_fetch_from_watch_refresh(
    cx: &mut TestAppContext,
) {
    use std::os::unix::fs::PermissionsExt;
    cx.executor().allow_parking();
    let repo = incoming_fixture("remote-check", 0);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let contents = std::fs::read(repo.join("a.txt")).unwrap();
    let base = repo.parent().unwrap();
    let calls = base.join("fetch-calls");
    let upload = base.join("upload-pack");
    std::fs::write(
        &upload,
        format!(
            "#!/bin/sh\nprintf 'fetch\\n' >> '{}'\nexec /usr/bin/git-upload-pack \"$@\"\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &repo,
        &[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ],
    );
    let (window, this) = open(cx, repo.clone());
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.groups[0]
                .status
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .behind,
            0
        )
    });
    cx.update_window(window.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    settle(cx, Some(window), |cx| this.read_with(cx, |p, _| !p.loading));
    cx.update_window(window.into(), |_, window, cx| {
        assert!(window.is_window_active());
        window.render_frame(cx);
        window.click("源代码管理", cx);
        this.update(cx, |p, cx| {
            let generation = p.generation;
            p.request_remote_check(false, window, cx);
            assert_eq!(
                generation, p.generation,
                "duplicate remote check restarted the job"
            );
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.loading && !p.watch.remote_pending)
    });
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.groups[0]
                .status
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .behind,
            1
        );
        assert!(!p.watch.remote_active);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find(("scm-repo-pull", 0usize)).visible());
        this.update(cx, |p, cx| {
            p.request_remote_check(false, window, cx);
            p.refresh_on_activation(window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| this.read_with(cx, |p, _| !p.loading));
    assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 1);
    assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), contents);
    let _ = std::fs::remove_dir_all(base);
}

/// Make Fetch observable and slow enough to exercise interruption and window release.
fn slow_upload_pack(repo: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let base = repo.parent().unwrap();
    let pids = base.join("fetch-pids");
    let upload = base.join("slow-upload-pack");
    std::fs::write(
        &upload,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$$\" >> '{}'\nsleep 2\nexec /usr/bin/git-upload-pack \"$@\"\n",
            pids.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&upload, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        repo,
        &[
            "config",
            "remote.origin.uploadpack",
            upload.to_str().unwrap(),
        ],
    );
    pids
}

#[gpui_kit::test]
async fn remote_check_survives_an_ordinary_refresh(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = incoming_fixture("remote-interrupted", 0);
    git(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let pids = slow_upload_pack(&repo);
    let (window, this) = open(cx, repo.clone());
    std::fs::write(repo.join("a.txt"), "local edit\n").unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.sidebar = Sidebar::SourceControl;
            p.request_remote_check(true, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        pids.exists()
            && this.read_with(cx, |p, _| {
                p.watch.remote_active
                    && p.groups[0]
                        .status
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .changes
                        .len()
                        == 1
            })
    });
    let cancelled = this.read_with(cx, |p, _| p.cancel.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            // This is also called by file saves and completed Git writes.
            p.refresh(window, cx);
            assert!(p.watch.remote_pending);
            assert!(p.watch.remote_manual);
            assert!(p.watch.last_remote_check.is_none());
        });
    })
    .unwrap();
    assert!(cancelled.load(Ordering::Relaxed));
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.loading && !p.watch.remote_pending)
    });
    this.read_with(cx, |p, _| {
        let status = p.groups[0].status.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(status.behind, 1, "interrupted remote check was lost");
        assert_eq!(status.changes.len(), 1);
        assert!(!p.watch.remote_active);
        assert!(!p.watch.remote_manual);
        assert!(p.watch.last_remote_check.is_some());
    });
    assert_eq!(std::fs::read_to_string(&pids).unwrap().lines().count(), 2);
    assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "local edit\n"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn remote_check_is_cancelled_and_reaped_when_the_window_closes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("remote-close", 0);
    let pids = slow_upload_pack(&repo);
    let (window, this) = open(cx, repo.clone());
    std::fs::write(repo.join("a.txt"), "local edit\n").unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.request_remote_check(true, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        pids.exists()
            && this.read_with(cx, |p, _| {
                p.watch.remote_active
                    && p.groups[0]
                        .status
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .changes
                        .len()
                        == 1
            })
    });
    let pid: i32 = std::fs::read_to_string(&pids)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let cancel = this.read_with(cx, |p, _| p.cancel.clone());
    cx.update_window(window.into(), |_, window, _| window.remove_window())
        .unwrap();
    drop(this);
    // Process the release of the test's last entity owner before waiting on plain atomics.
    cx.update(|_| {});
    settle(cx, None, |_| {
        // SAFETY: signal 0 only checks the known temporary helper's pid.
        cancel.load(Ordering::Relaxed) && unsafe { libc::kill(pid, 0) } == -1
    });
    assert!(
        !cx.has_pending_prompt(),
        "closed window received a late error"
    );
    assert_eq!(std::fs::read_to_string(&pids).unwrap().lines().count(), 1);
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn remote_check_failure_keeps_local_changes_and_reports_the_error(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("remote-failure", 0);
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            repo.parent().unwrap().join("missing.git").to_str().unwrap(),
        ],
    );
    std::fs::write(repo.join("a.txt"), "local edit\n").unwrap();
    let (window, this) = open(cx, repo.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.sidebar = Sidebar::SourceControl;
            p.hide_clean_repos = true;
            p.request_remote_check(true, window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| !p.loading && p.groups[0].remote_error.is_some())
    });
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.groups[0]
                .status
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .changes
                .len(),
            1
        );
        assert!(
            p.issues.is_empty(),
            "remote failure invalidated repository discovery"
        );
        assert!(p.message.starts_with("远端检查失败"));
        assert!(!p.groups[0].clean());
    });
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("确定");
    cx.run_until_parked();
    assert_eq!(
        std::fs::read_to_string(repo.join("a.txt")).unwrap(),
        "local edit\n"
    );
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

fn rows(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .map(|row| match row.clone() {
                Row::Group(_) => "repo".to_string(),
                Row::Commit(_) => "message".to_string(),
                Row::Heading(_, _, n) => format!("changes {n}"),
                Row::File(..) => "file".to_string(),
                Row::Folder(_, path, _) => format!("folder {}", path.path.display()),
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
async fn inactive_commit_button_does_not_keep_the_active_fill(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("inactive-fill", 0);
    let (window, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let check_fill = |cx: &mut TestAppContext, enabled: bool| {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let button = window.find(("scm-repo-commit", 0usize));
            let center = button.bounds().center().scale(window.scale_factor());
            let colors = theme::colors(cx);
            let fill: gpui_kit::Background = if enabled {
                colors.commit
            } else {
                colors.keycap
            }
            .into();
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.bounds.contains(&center)
                        && quad.content_mask.bounds.contains(&center)
                        && (quad.background == fill
                            || (enabled && quad.background == colors.commit_hover.into()))
                }),
                "commit action lost its opaque enabled or disabled fill (enabled={enabled})"
            );
            assert_eq!(button.bounds().size.width, theme::ROW_HEIGHT);
            assert_eq!(button.bounds().size.height, theme::ROW_HEIGHT);
            assert!(
                window.find(("scm-repo-graph", 0usize)).bounds().right() <= button.bounds().left()
            );
            assert!(window.try_find(("scm-commit", 0usize)).is_none());
            assert!(window.try_find(("scm-commit-main", 0usize)).is_none());
            assert!(window.try_find(("scm-generate-message", 0usize)).is_some());
            if !enabled {
                window.click(("scm-repo-commit", 0usize), cx);
            }
        })
        .unwrap();
        if !enabled {
            cx.run_until_parked();
            this.read_with(cx, |p, _| {
                assert!(
                    p.groups[0].expanded,
                    "disabled commit toggled its repository"
                );
                assert!(p.groups[0].write_message.is_empty());
            });
            assert!(!cx.has_pending_prompt());
        }
    };
    check_fill(cx, false);

    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.refresh(window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| this.read_with(cx, |p, _| !p.loading));
    check_fill(cx, true);

    this.update(cx, |p, cx| {
        p.groups[0].write_pending = true;
        cx.notify();
    });
    check_fill(cx, false);
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn repository_more_menu_keeps_advanced_commit_actions(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("commit-menu", 0);
    let (window, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let check_menu = |cx: &mut TestAppContext, enabled: [bool; 3]| {
        for (index, label) in ["提交", "提交并推送", "修改上次提交"]
            .into_iter()
            .enumerate()
        {
            this.update(cx, |p, cx| {
                p.groups[0].write_message.clear();
                cx.notify();
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.click(("scm-repo-more", 0usize), cx);
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                assert_eq!(window.within("popup-menu").find(index).label(), Some(label));
                window.within("popup-menu").click(index, cx);
            })
            .unwrap();
            cx.run_until_parked();
            if index == 2 {
                assert_eq!(cx.has_pending_prompt(), enabled[index]);
                if enabled[index] {
                    cx.simulate_prompt_answer("取消");
                }
            } else {
                // An enabled new commit with no message asks for one; a disabled item is inert.
                assert_eq!(
                    this.read_with(cx, |p, _| p.groups[0].write_message == "请输入提交消息"),
                    enabled[index]
                );
                assert!(!cx.has_pending_prompt());
            }
            cx.update_window(window.into(), |_, window, cx| {
                window.press("escape", cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    check_menu(cx, [false, false, true]);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.refresh(window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| this.read_with(cx, |p, _| !p.loading));
    check_menu(cx, [true, true, true]);
    this.update(cx, |p, cx| {
        let status = p.groups[0].status.as_mut().unwrap().as_mut().unwrap();
        Arc::make_mut(status).upstream = None;
        cx.notify();
    });
    check_menu(cx, [true, false, true]);
    this.update(cx, |p, cx| {
        p.groups[0].write_pending = true;
        cx.notify();
    });
    check_menu(cx, [false, false, false]);
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
async fn saving_as_another_path_never_keeps_the_old_files_blame(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("blame-path", 0);
    let base = repo.parent().unwrap().to_path_buf();
    let original = repo.join("a.txt");
    let (handle, this) = open(cx, repo.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(original.clone(), Some(repo.clone()), window, cx)
        });
    })
    .unwrap();
    let committed = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            matches!(
                &p.blame.shown,
                Some(Ok(blame)) if !blame.uncommitted && blame.author == "Fixture"
            )
        })
    };
    settle(cx, Some(handle), committed);
    let id = this.read_with(cx, |p, _| p.active_document_id().unwrap());

    for target in [base.join("outside.txt"), repo.join("new.txt")] {
        let task = cx
            .update_window(handle.into(), |_, window, cx| {
                this.update(cx, |p, cx| p.write_as(id, target.clone(), window, cx))
            })
            .unwrap();
        assert!(task.await);
        assert!(
            this.read_with(cx, |p, _| p.blame.shown.is_none()),
            "old file blame survived Save As to {}",
            target.display()
        );
        settle(cx, Some(handle), |cx| this.read_with(cx, |p, _| !p.loading));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(
            this.read_with(cx, |p, _| p.blame.shown.is_none()),
            "file without history retained old blame"
        );
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("line-blame").is_none());
        })
        .unwrap();

        let task = cx
            .update_window(handle.into(), |_, window, cx| {
                this.update(cx, |p, cx| p.write_as(id, original.clone(), window, cx))
            })
            .unwrap();
        assert!(task.await);
        settle(cx, Some(handle), committed);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find("line-blame").is_some(),
                "tracked file did not recover its annotation"
            );
        })
        .unwrap();
    }
    let _ = std::fs::remove_dir_all(base);
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

#[gpui_kit::test]
async fn failed_git_writes_prompt_without_leaving_inline_errors(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    cx.executor().allow_parking();
    for (kind, title, reason) in [
        ("stage", "暂存失败", "index.lock"),
        ("commit", "提交失败", "fixture rejected commit"),
        ("push", "推送失败", "missing-remote.git"),
        ("fetch", "抓取失败", "missing-remote.git"),
    ] {
        let repo = fixture(&format!("error-dialog-{kind}"), 1);
        std::fs::write(repo.join("a.txt"), "edited\n").unwrap();
        if kind == "commit" {
            git(&repo, &["add", "a.txt"]);
            let hook = repo.join(".git/hooks/pre-commit");
            std::fs::write(
                &hook,
                "#!/bin/sh\necho 'fixture rejected commit' >&2\nexit 1\n",
            )
            .unwrap();
            std::fs::set_permissions(hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        if matches!(kind, "push" | "fetch") {
            let remote = repo.parent().unwrap().join("missing-remote.git");
            git(
                &repo,
                &["remote", "set-url", "origin", remote.to_str().unwrap()],
            );
        }
        let (handle, this) = open(cx, repo.clone());
        if kind == "stage" {
            std::fs::write(repo.join(".git/index.lock"), "").unwrap();
        }
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.sidebar = Sidebar::SourceControl;
                let group = &p.groups[0];
                let request = WriteRequest {
                    repo: group.repo.clone(),
                    generation: 0,
                    expected: group.status.as_ref().unwrap().as_ref().unwrap().clone(),
                    operation: match kind {
                        "stage" => WriteOperation::Stage {
                            paths: vec!["a.txt".into()],
                        },
                        "commit" => WriteOperation::Commit {
                            message: "feat: 保留输入".into(),
                            amend: false,
                            push: false,
                            all: false,
                        },
                        "push" => WriteOperation::Push,
                        "fetch" => WriteOperation::Fetch,
                        _ => unreachable!(),
                    },
                };
                group.commit_input.update(cx, |input, cx| {
                    input.set_value("feat: 保留输入", window, cx);
                });
                p.request_git_write(request, window, cx);
            });
        })
        .unwrap();
        if kind == "push" {
            assert!(cx.has_pending_prompt());
            cx.simulate_prompt_answer("推送");
            cx.run_until_parked();
        }
        settle(cx, None, |cx| {
            this.read_with(cx, |p, _| !p.groups[0].write_pending)
        });
        let (actual_title, detail) = cx
            .pending_prompt()
            .expect("failed Git write must show a dialog");
        assert_eq!(actual_title, title);
        assert!(detail.contains(repo.to_str().unwrap()), "{detail}");
        assert!(detail.contains(reason), "{detail}");
        this.read_with(cx, |p, cx| {
            assert!(p.groups[0].write_message.is_empty());
            assert_eq!(
                p.groups[0].commit_input.read(cx).value().as_ref(),
                "feat: 保留输入"
            );
            assert!(!p.message.contains("正在"));
        });
        cx.simulate_prompt_answer("确定");
        cx.run_until_parked();
        assert!(!cx.has_pending_prompt());
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }
}

#[gpui_kit::test]
async fn file_statuses_stay_in_the_right_column_for_long_names(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("status-column", 0);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    std::fs::write(
        repo.join("graft-statusline-with-a-very-long-file-name.cjs"),
        "new\n",
    )
    .unwrap();
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let indices = this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| matches!(row, Row::File(..)).then_some(i))
            .collect::<Vec<_>>()
    });
    assert_eq!(indices.len(), 2);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let mut right = None;
        for index in indices {
            let row = window.find(("scm-row-bounds", index)).bounds();
            let tag = window.find(("scm-status", index)).bounds();
            assert!(
                tag.right() <= row.right(),
                "status {tag:?} escapes row {row:?}"
            );
            assert_eq!(*right.get_or_insert(tag.right()), tag.right());
        }
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn hidden_file_actions_keep_their_slot_and_do_not_stage(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("hidden-actions", 0);
    std::fs::write(repo.join("a.txt"), "changed\n").unwrap();
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let index = this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .position(|row| matches!(row, Row::File(..)))
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let slot = window.find(("scm-file-actions", index));
        assert!(!slot.visible());
        let bounds = slot.bounds();
        let status = window.find(("scm-status", index)).bounds();
        window.hover(("scm-row-bounds", index), cx);
        assert!(window.find(("scm-file-actions", index)).visible());
        assert_eq!(window.find(("scm-file-actions", index)).bounds(), bounds);
        assert_eq!(window.find(("scm-status", index)).bounds(), status);
        let stage = window.find(("scm-stage", index)).bounds().center();
        window.hover(("scm-repo-graph", 0usize), cx);
        assert!(!window.find(("scm-file-actions", index)).visible());
        // A pointer-down without a preceding move must not hit the hidden button.
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: stage,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: stage,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        this.read_with(cx, |p, _| assert!(!p.groups[0].write_pending));
        window.hover(("scm-row-bounds", index), cx);
        window.click(("scm-stage", index), cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            !p.groups[0].write_pending
                && p.groups[0]
                    .status
                    .as_ref()
                    .and_then(|s| s.as_ref().ok())
                    .is_some_and(|s| s.changes.iter().any(|c| c.index != b'.'))
        })
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

fn change_tree_fixture(name: &str) -> PathBuf {
    let repo = fixture(name, 0);
    for directory in ["alpha/nested", "beta"] {
        std::fs::create_dir_all(repo.join(directory)).unwrap();
    }
    for path in ["alpha/nested/same.txt", "beta/same.txt"] {
        std::fs::write(repo.join(path), "base\n").unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "tree fixture"]);
    std::fs::write(repo.join("alpha/nested/same.txt"), "staged\n").unwrap();
    git(&repo, &["add", "alpha/nested/same.txt"]);
    for path in ["alpha/nested/same.txt", "beta/same.txt", "a.txt"] {
        std::fs::write(repo.join(path), "worktree\n").unwrap();
    }
    repo
}

fn change_tree_rows(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .filter_map(|row| {
                let (kind, path, side) = match row {
                    Row::Folder(_, folder, side) => ("folder", &folder.path, side),
                    Row::File(g, i, side, _) => (
                        "file",
                        &p.groups[*g]
                            .status
                            .as_ref()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .changes[*i]
                            .path,
                        side,
                    ),
                    _ => return None,
                };
                Some(format!(
                    "{} {kind} {}",
                    match side {
                        DiffSide::Staged => "staged",
                        DiffSide::Worktree => "worktree",
                    },
                    path.display()
                ))
            })
            .collect()
    })
}

#[gpui_kit::test]
async fn change_tree_folders_are_independent_and_survive_refresh(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = change_tree_fixture("change-tree");
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let expanded = vec![
        "staged folder alpha/nested",
        "staged file alpha/nested/same.txt",
        "worktree folder alpha/nested",
        "worktree file alpha/nested/same.txt",
        "worktree folder beta",
        "worktree file beta/same.txt",
        "worktree file a.txt",
    ];
    assert_eq!(change_tree_rows(cx, &this), expanded);
    // A fold saved before compaction still controls the merged row; expanding clears it.
    this.update(cx, |p, cx| {
        p.groups[0]
            .changes_folders_collapsed
            .insert(PathBuf::from("alpha"));
        p.rebuild_rows();
        cx.notify();
    });
    let legacy_folder = this.read_with(cx, |p, _| p.rows.iter().position(|row| {
        matches!(row, Row::Folder(_, folder, DiffSide::Worktree) if folder.path == std::path::Path::new("alpha/nested"))
    }).unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("scm-row", legacy_folder)).expanded(),
            Some(false)
        );
        window.click(("scm-row", legacy_folder), cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.groups[0].changes_folders_collapsed.is_empty())
    });
    assert_eq!(change_tree_rows(cx, &this), expanded);
    let folder = this.read_with(cx, |p, _| p.rows.iter().position(|row| {
        matches!(row, Row::Folder(_, folder, DiffSide::Worktree) if folder.path == std::path::Path::new("alpha/nested"))
    }).unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let item = window.find(("scm-row", folder));
        assert_eq!(item.role(), Some(Role::TreeItem));
        assert_eq!(item.expanded(), Some(true));
        assert!(
            item.label()
                .unwrap()
                .contains(repo.join("alpha/nested").to_str().unwrap())
        );
        window.click(("scm-row", folder), cx);
    })
    .unwrap();
    let collapsed = vec![
        "staged folder alpha/nested",
        "staged file alpha/nested/same.txt",
        "worktree folder alpha/nested",
        "worktree folder beta",
        "worktree file beta/same.txt",
        "worktree file a.txt",
    ];
    assert_eq!(change_tree_rows(cx, &this), collapsed);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("scm-row", folder)).expanded(), Some(false));
    })
    .unwrap();
    // A real status refresh adds a leaf while retaining this side's collapsed parent.
    std::fs::write(repo.join("beta/new.txt"), "new\n").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.refresh(window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            !p.loading
                && p.groups[0]
                    .status
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .changes
                    .iter()
                    .any(|c| c.path == std::path::Path::new("beta/new.txt"))
        })
    });
    let mut refreshed = collapsed.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    refreshed.insert(4, "worktree file beta/new.txt".into());
    assert_eq!(change_tree_rows(cx, &this), refreshed);
    this.read_with(cx, |p, _| {
        assert!(p.groups[0].staged_folders_collapsed.is_empty());
        assert!(
            p.groups[0]
                .changes_folders_collapsed
                .contains(std::path::Path::new("alpha/nested"))
        );
    });
    git(
        &repo,
        &[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "alpha",
        ],
    );
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.refresh(window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            !p.loading
                && !p.groups[0]
                    .status
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .changes
                    .iter()
                    .any(|c| c.path.starts_with("alpha"))
        })
    });
    this.read_with(cx, |p, _| {
        assert!(p.groups[0].changes_folders_collapsed.is_empty())
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

#[gpui_kit::test]
async fn change_tree_file_actions_keep_original_status_indices(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = change_tree_fixture("change-tree-actions");
    let (handle, this) = open(cx, repo.clone());
    this.update(cx, |p, cx| {
        p.sidebar = Sidebar::SourceControl;
        cx.notify();
    });
    let index = this.read_with(cx, |p, _| {
        p.rows
            .iter()
            .position(|row| {
                matches!(row, Row::File(g, i, DiffSide::Worktree, _)
            if p.groups[*g].status.as_ref().unwrap().as_ref().unwrap().changes[*i].path
                == std::path::Path::new("alpha/nested/same.txt"))
            })
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.hover(("scm-row-bounds", index), cx);
        window.click(("scm-stage", index), cx);
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            !p.groups[0].write_pending
                && p.groups[0]
                    .status
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .changes
                    .iter()
                    .any(|c| {
                        c.path == std::path::Path::new("alpha/nested/same.txt")
                            && c.staged()
                            && !c.unstaged()
                    })
        })
    });
    assert_eq!(git_out(&repo, &["show", ":beta/same.txt"]), "base");
    assert_eq!(
        git_out(&repo, &["show", ":alpha/nested/same.txt"]),
        "worktree"
    );
    assert_eq!(git_out(&repo, &["show", ":a.txt"]), "a");
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
