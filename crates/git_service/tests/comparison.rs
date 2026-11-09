mod common;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use workspace_editor_git::{CommitSearch, ComparisonMode, GitService, GraphScope};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
impl Fixture {
    fn new() -> Self {
        common::hermetic();
        let root = std::env::temp_dir().join(format!(
            "zj-comparison-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self(root);
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Search Author"]);
        fixture.git(&["config", "user.email", "search@example.invalid"]);
        fixture
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
    fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

#[test]
fn comparisons_history_blob_search_and_patch_preflight_use_real_local_git() {
    let f = Fixture::new();
    let old_path = "old name\t.txt";
    let new_path = "renamed\nname.txt";
    fs::write(f.0.join(old_path), "first\nsecond\n").unwrap();
    let base = f.commit("ancient needle");
    f.git(&["checkout", "-qb", "feature"]);
    f.git(&["mv", old_path, new_path]);
    let renamed = f.commit("rename tracked file");
    fs::write(f.0.join(new_path), "first\nchanged\n").unwrap();
    let target = f.commit("feature message");
    f.git(&["checkout", "-q", "main"]);
    fs::write(f.0.join("main-only"), "main\n").unwrap();
    f.commit("main diverges");
    let service = GitService::new(2, Duration::from_secs(5)).unwrap();
    let cancel = AtomicBool::new(false);
    let repo = service.identify(&f.0, &cancel).unwrap();
    let direct = service
        .compare_revisions(&repo, "main", "feature", ComparisonMode::Direct, &cancel)
        .unwrap();
    assert!(
        direct
            .files
            .iter()
            .any(|file| file.path == std::path::Path::new("main-only"))
    );
    let fork = service
        .compare_revisions(&repo, "main", "feature", ComparisonMode::MergeBase, &cancel)
        .unwrap();
    assert_eq!(fork.base, base);
    assert_eq!(fork.target, target);
    assert!(
        !fork
            .files
            .iter()
            .any(|file| file.path == std::path::Path::new("main-only"))
    );
    assert!(
        !service
            .comparison_patch(&repo, &fork, None, &cancel)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        service
            .read_blob(&repo, &renamed, new_path.as_ref(), &cancel)
            .unwrap(),
        b"first\nsecond\n"
    );
    assert!(
        service
            .read_blob(
                &repo,
                "--output=/tmp/should-not-exist",
                old_path.as_ref(),
                &cancel
            )
            .is_err()
    );
    assert!(
        service
            .read_blob(&repo, "HEAD", "../outside".as_ref(), &cancel)
            .is_err()
    );
    f.git(&["checkout", "-q", "feature"]);
    assert_eq!(
        service
            .read_index_blob(&repo, new_path.as_ref(), &cancel)
            .unwrap(),
        b"first\nchanged\n"
    );
    let history = service
        .file_history(&repo, new_path.as_ref(), 0, 10, &cancel)
        .unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].commit.hash, target);
    assert_eq!(history[1].path, PathBuf::from(new_path));
    assert_eq!(history[2].path, PathBuf::from(old_path));
    let page = service
        .file_history(&repo, new_path.as_ref(), 2, 1, &cancel)
        .unwrap();
    assert_eq!(page[0].path, PathBuf::from(old_path));
    assert_eq!(
        service
            .read_blob(&repo, &page[0].commit.hash, &page[0].path, &cancel)
            .unwrap(),
        b"first\nsecond\n"
    );
    for query in [
        CommitSearch {
            message: "ancient needle".into(),
            ..Default::default()
        },
        CommitSearch {
            author: "Search Author".into(),
            message: "ancient".into(),
            ..Default::default()
        },
        CommitSearch {
            hash: base[..8].into(),
            ..Default::default()
        },
        CommitSearch {
            since: "2000-01-01".into(),
            until: "2099-01-01".into(),
            message: "ancient".into(),
            path: Some(old_path.into()),
            ..Default::default()
        },
    ] {
        let results = service
            .search_commits(&repo, &GraphScope::All, &query, 0, 1, &cancel)
            .unwrap();
        assert_eq!(results.len(), 1, "query: {query:?}");
        assert_eq!(results[0].hash, base);
    }
    let patch = "diff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1 @@\n+added\n";
    service.check_patch(&repo, patch, false, &cancel).unwrap();
    assert_eq!(
        service.patch_target(&repo, patch, &cancel).unwrap(),
        PathBuf::from("new.txt")
    );
    assert!(
        service
            .patch_target(
                &repo,
                &format!("{patch}{}", patch.replace("new.txt", "another.txt")),
                &cancel
            )
            .is_err()
    );
    assert!(!f.0.join("new.txt").exists(), "preflight must not write");
    assert!(
        service
            .check_patch(
                &repo,
                &patch.replace("new.txt", "../../escape.txt"),
                false,
                &cancel
            )
            .is_err()
    );
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        service
            .read_blob(&repo, "HEAD", new_path.as_ref(), &cancel)
            .is_err()
    );
}
