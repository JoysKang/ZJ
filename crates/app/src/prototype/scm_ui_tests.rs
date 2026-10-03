//! Source Control in a headless window, on a temporary repository.

use super::super::agent::AgentStore;
use super::super::*;
// `gpui_kit::*` also exports a `test` macro; `#[gpui_kit::test]` expands to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root};

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

fn open(cx: &mut TestAppContext, root: PathBuf) -> Entity<Prototype> {
    let (_, this) = cx.update(|cx| {
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
            return this;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the repository never finished loading");
}

fn rows(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Vec<String> {
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
    let this = open(cx, repo.clone());
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
    let this = open(cx, repo.clone());
    assert_eq!(rows(cx, &this), ["repo"]);
    this.read_with(cx, |p, _| assert!(!p.groups[0].expandable()));
    let _ = std::fs::remove_dir_all(repo.parent().unwrap());
}
