//! Git Graph in a headless window, on a temporary repository with a merged branch.

use super::super::agent::AgentStore;
use super::super::*;
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root, test::TestWindowExt};

fn git(dir: &std::path::Path, args: &[&str]) {
    git_env(dir, args, None);
}

/// `date` pins author and committer time, so --date-order is deterministic.
fn git_env(dir: &std::path::Path, args: &[&str], date: Option<&str>) {
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(date) = date {
        command.env("GIT_AUTHOR_DATE", date);
        command.env("GIT_COMMITTER_DATE", date);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// main: one → two, with feature (one → feat) merged back: a two-lane graph.
fn fixture(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zj-graph-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for (key, value) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.invalid"),
        ("commit.gpgsign", "false"),
        ("tag.gpgSign", "false"),
        ("core.hooksPath", ".git/hooks"),
    ] {
        git(&repo, &["config", key, value]);
    }
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    git(&repo, &["add", "."]);
    git_env(
        &repo,
        &["commit", "-q", "-m", "one"],
        Some("2024-01-01T00:00:00Z"),
    );
    git_env(
        &repo,
        &["commit", "-q", "--allow-empty", "-m", "two"],
        Some("2024-01-02T00:00:00Z"),
    );
    git(&repo, &["switch", "-q", "-c", "feature", "HEAD~1"]);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    git(&repo, &["add", "."]);
    git_env(
        &repo,
        &["commit", "-q", "-m", "feat"],
        Some("2024-01-03T00:00:00Z"),
    );
    git(&repo, &["switch", "-q", "main"]);
    git_env(
        &repo,
        &["merge", "-q", "--no-ff", "-m", "merge", "feature"],
        Some("2024-01-04T00:00:00Z"),
    );
    std::fs::canonicalize(repo).unwrap()
}

fn open(cx: &mut TestAppContext, root: PathBuf) -> (WindowHandle<Root>, Entity<Prototype>) {
    let (window, this) = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::settings::Settings::default());
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(AgentStore {
            history: None,
            default_workspace: None,
        });
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(OpenDocuments(documents.clone()));
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Prototype::new(Some(root), service, documents, 1, window, cx)),
        )
        .map(|(window, this)| (window.downcast::<Root>().unwrap(), this))
        .unwrap()
    });
    // Discovery and status run on real threads; the view polls them on a 50 ms timer.
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        if this.read_with(cx, |p, _| p.refresh_completed && !p.loading) {
            return (window, this);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the repository never finished loading");
}

/// Waits until `done` holds (pages and details load through real Git calls).
fn settle(cx: &mut TestAppContext, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    for _ in 0..200 {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the operation never settled");
}

#[gpui_kit::test]
async fn the_graph_shows_lanes_refs_details_and_diffs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("lanes");
    let (window, this) = open(cx, repo.clone());

    // Opening the graph (status bar / ··· menu) shows the tab and loads the first page.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_git_graph(0, window, cx));
    })
    .unwrap();
    settle(cx, |cx| {
        this.read_with(cx, |p, _| {
            p.graph
                .as_ref()
                .is_some_and(|g| !g.loading && g.error.is_none() && !g.commits.is_empty())
        })
    });
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Graph));

    let (subjects, shape, refs, branches) = this.read_with(cx, |p, _| {
        let graph = p.graph.as_ref().unwrap();
        (
            graph
                .commits
                .iter()
                .map(|commit| commit.subject.clone())
                .collect::<Vec<_>>(),
            graph
                .rows
                .iter()
                .map(|row| {
                    let down: Vec<_> = row.down.iter().map(|&(from, to, _)| (from, to)).collect();
                    (row.lane, down)
                })
                .collect::<Vec<_>>(),
            graph.commits[0]
                .refs
                .iter()
                .map(|r| (r.name.clone(), r.head))
                .collect::<Vec<_>>(),
            graph
                .branches
                .iter()
                .map(|branch| branch.name.clone())
                .collect::<Vec<_>>(),
        )
    });
    assert_eq!(subjects, ["merge", "feat", "two", "one"]);
    // main stays on lane 0; the feature line runs beside "two" and curves into "one".
    assert_eq!(
        shape,
        [
            (0, vec![(0, 0), (0, 1)]),
            (1, vec![(0, 0), (1, 1)]),
            (0, vec![(0, 0), (1, 0)]),
            (0, vec![]),
        ]
    );
    assert!(refs.iter().any(|(name, head)| name == "main" && *head));
    // for-each-ref sorts by committerdate: the merge (main) is newer than feat (feature).
    assert_eq!(branches, ["main", "feature"]);

    // Selecting a commit loads its details; the merge lists the feature's file.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.graph_select(0, window, cx));
    })
    .unwrap();
    settle(cx, |cx| {
        this.read_with(cx, |p, _| {
            p.graph
                .as_ref()
                .is_some_and(|g| !g.details_loading && g.details.is_some())
        })
    });
    let files = this.read_with(cx, |p, _| {
        let graph = p.graph.as_ref().unwrap();
        match graph.details.as_ref().unwrap() {
            Ok(details) => details
                .files
                .iter()
                .map(|file| file.path.display().to_string())
                .collect::<Vec<_>>(),
            Err(error) => panic!("{error}"),
        }
    });
    assert_eq!(files, ["b.txt"]);

    // Clicking the file opens the commit's diff for it; the graph tab stays open.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.graph_open_diff(0, window, cx));
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Diff);
        assert!(p.graph.is_some());
        let diff = p.preview_diff.as_ref().unwrap();
        assert!(diff.label.contains("b.txt"), "{}", diff.label);
        let Some(request) = diff.request() else {
            panic!("the tab is not a Git diff");
        };
        let Operation::CommitDiff { commit, path, .. } = &request.operation else {
            panic!("not a commit diff");
        };
        assert_eq!(path, &PathBuf::from("b.txt"));
        assert_eq!(commit.len(), 40);
    });

    // The branch filter narrows the history to one branch.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.graph_set_scope(GraphScope::Branch("refs/heads/feature".into()), window, cx)
        });
    })
    .unwrap();
    settle(cx, |cx| {
        this.read_with(cx, |p, _| {
            p.graph.as_ref().is_some_and(|g| {
                !g.loading && g.commits.len() == 2 && g.commits[0].subject == "feat"
            })
        })
    });

    // Closing the graph tab in the background leaves the diff tab active.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_graph(window, cx));
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.graph.is_none());
        assert_eq!(p.active, Pane::Diff);
    });
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// Types into the focused picker, then presses Enter (input events land between updates).
fn type_and_enter(cx: &mut TestAppContext, window: WindowHandle<Root>, text: Option<&str>) {
    if let Some(text) = text {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn tags_on(cx: &mut TestAppContext, this: &Entity<Prototype>, index: usize) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.graph.as_ref().map_or(Vec::new(), |graph| {
            graph.commits.get(index).map_or(Vec::new(), |commit| {
                commit
                    .refs
                    .iter()
                    .filter(|r| matches!(r.kind, RefKind::Tag))
                    .map(|r| r.name.clone())
                    .collect()
            })
        })
    })
}

#[gpui_kit::test]
async fn tags_are_added_and_deleted_from_the_graph(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let repo = fixture("tags");
    let (window, this) = open(cx, repo.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_git_graph(0, window, cx));
    })
    .unwrap();
    settle(cx, |cx| {
        this.read_with(cx, |p, _| {
            p.graph
                .as_ref()
                .is_some_and(|g| !g.loading && g.commits.len() == 4)
        })
    });
    let (id, merge, one) = this.read_with(cx, |p, _| {
        let graph = p.graph.as_ref().unwrap();
        (
            graph.repo.id.clone(),
            graph.commits[0].hash.clone(),
            graph.commits[3].hash.clone(),
        )
    });

    // 添加标签…: the name, then a message, makes an annotated tag shown on the commit.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_create_tag(&id, merge.clone(), window, cx)
        });
    })
    .unwrap();
    type_and_enter(cx, window, Some("v1"));
    let labels: Vec<bool> = this.read_with(cx, |p, _| {
        let (items, _) = p.quick_open.as_ref().unwrap().items.as_ref().unwrap();
        items
            .iter()
            .map(|item| matches!(item.pick, super::super::quick_open::Pick::CreateTag { push, .. } if push))
            .collect()
    });
    assert_eq!(labels, [false, true]);
    type_and_enter(cx, window, Some("第一版"));
    settle(cx, |cx| tags_on(cx, &this, 0) == ["v1"]);
    assert_eq!(git_out(&repo, &["cat-file", "-t", "v1"]), "tag");
    assert_eq!(
        git_out(&repo, &["tag", "-l", "--format=%(contents)", "v1"]),
        "第一版"
    );

    // Without a message the tag is lightweight.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_create_tag(&id, one.clone(), window, cx));
    })
    .unwrap();
    type_and_enter(cx, window, Some("v0"));
    type_and_enter(cx, window, None);
    settle(cx, |cx| tags_on(cx, &this, 3) == ["v0"]);
    assert_eq!(git_out(&repo, &["cat-file", "-t", "v0"]), "commit");

    // 删除标签… asks first; 删除本地标签 removes it from the repository and the graph.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.scm_delete_tag(&id, "v1".into(), window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("删除本地标签");
    settle(cx, |cx| tags_on(cx, &this, 0).is_empty());
    assert_eq!(git_out(&repo, &["tag", "--list"]), "v0");
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
