use super::super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl Fixture {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "zj-comparison-ui-{name}-{}-{}",
            std::process::id(),
            crate::time::now_ms()
        ));
        std::fs::create_dir(&path).unwrap();
        let path = std::fs::canonicalize(path).unwrap();
        let f = Self(path);
        f.git(&["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            f.git(&["config", key, value]);
        }
        f
    }
    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.0)
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
    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
    }
}

#[gpui_kit::test]
async fn comparison_mode_change_discards_the_inflight_result(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("mode-race");
    std::fs::write(f.0.join("a.txt"), "old\n").unwrap();
    f.commit("old");
    std::fs::write(f.0.join("a.txt"), "new\n").unwrap();
    f.commit("new");
    let (handle, this) = open(cx, f.0.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.groups.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_revision_comparison(0, window, cx);
            p.diff
                .review
                .comparison
                .base
                .update(cx, |input, cx| input.set_value("HEAD~1", window, cx));
        });
        window.render_frame(cx);
        window.click("comparison-run", cx);
        window.render_frame(cx);
        window.click("comparison-mode", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.diff.review.comparison.mode, ComparisonMode::MergeBase);
        assert!(!p.diff.review.comparison.loading);
        assert!(
            p.diff.review.comparison.comparison.is_none(),
            "old direct comparison was installed after switching mode"
        );
    });
}

#[gpui_kit::test]
async fn comparison_files_clipboard_revisions_and_renamed_readonly_history(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let f = Fixture::new("history");
    let old = "context\n".repeat(12) + "old value\n";
    let new = old.replace("old value", "new value");
    std::fs::write(f.0.join("before.txt"), &old).unwrap();
    f.commit("original file");
    f.git(&["mv", "before.txt", "after.txt"]);
    std::fs::write(f.0.join("after.txt"), &new).unwrap();
    f.commit("rename and change");
    std::fs::write(f.0.join("left.txt"), &old).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.select_for_comparison(f.0.join("left.txt"), cx);
            p.compare_with_selected(f.0.join("after.txt"), window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    this.read_with(cx, |p, _| {
        assert!(p.diff.doc.as_ref().unwrap().old.text.contains("old value"));
        assert!(p.diff.doc.as_ref().unwrap().new.text.contains("new value"));
        assert!(!p.diff_partial_ok());
    });
    cx.update_window(handle.into(), |_, window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("clipboard baseline\n".into()));
        this.update(cx, |p, cx| {
            p.compare_with_clipboard(f.0.join("after.txt"), window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff
                .doc
                .as_ref()
                .is_some_and(|doc| doc.old.text.contains("clipboard baseline"))
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_revision_comparison(0, window, cx);
            let input = p.diff.review.comparison.base.clone();
            input.update(cx, |input, cx| input.set_value("HEAD~1", window, cx));
        });
        window.render_frame(cx);
        window.click("comparison-run", cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.comparison.comparison.is_some())
    });
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.diff
                .review
                .comparison
                .comparison
                .as_ref()
                .unwrap()
                .files
                .len(),
            1
        )
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("revision-file", 0usize), cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff
                .tab
                .as_ref()
                .is_some_and(|tab| tab.checkpoint.is_some())
                && !p.diff.loading
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file_history(f.0.join("after.txt"), window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.comparison.history.len() == 2)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("history-entry", 1usize), cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.comparison.snapshot.is_some())
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.diff.review.comparison.history[1].path,
            PathBuf::from("before.txt")
        );
        assert_eq!(
            p.diff
                .review
                .comparison
                .snapshot
                .as_ref()
                .unwrap()
                .read(cx)
                .text()
                .to_string(),
            old
        );
        assert!(p.diff.doc.is_none());
        assert!(p.diff.source.is_none());
        assert!(matches!(
            p.diff.tab.as_ref().unwrap().checkpoint,
            Some(crate::session::review_progress::ComparisonSource::History {
                current_path: None,
                ..
            })
        ));
    });
    // A pinned full historical file must survive opening and returning from another Diff.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.pin_current_diff(window, cx);
            p.compare_with_selected(f.0.join("after.txt"), window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| !p.diff.loading && !p.archived_diffs.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let id = p.archived_diffs[0].id;
            p.activate_saved_diff(id, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.diff
                .review
                .comparison
                .snapshot
                .as_ref()
                .expect("pinned historical content lost")
                .read(cx)
                .text()
                .to_string(),
            old
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.select_pane(Pane::Welcome, window, cx);
            p.select_pane(Pane::Diff, window, cx);
            let snapshot = p.diff.review.comparison.snapshot.as_ref().unwrap();
            assert!(
                snapshot.read(cx).focus_handle(cx).is_focused(window),
                "historical content must regain editor focus"
            );
        });
    })
    .unwrap();
    // Reopen the history controls to exercise their independent compare action.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file_history(f.0.join("after.txt"), window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.comparison.history.len() == 2)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("history-entry", 1usize), cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.comparison.snapshot.is_some())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("history-compare-current", cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff.review.comparison.snapshot.is_none()
                && p.diff
                    .tab
                    .as_ref()
                    .is_some_and(|tab| tab.label == "历史比较")
                && !p.diff.loading
        })
    });
    this.read_with(cx, |p, _| {
        let doc = p.diff.doc.as_ref().unwrap();
        assert_eq!(doc.old.text, old);
        assert_eq!(doc.new.text, new);
        assert!(
            p.diff.tab.as_ref().unwrap().checkpoint.is_some(),
            "stable history comparison was excluded from persistence"
        );
    });
    assert_eq!(std::fs::read_to_string(f.0.join("after.txt")).unwrap(), new);
    // A later tab choice wins even if the comparison read is already queued.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.compare_with_selected(f.0.join("after.txt"), window, cx);
            p.select_pane(Pane::Welcome, window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| !p.diff.review.comparison.loading)
    });
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Welcome));
}

#[gpui_kit::test]
async fn patch_file_preflight_preview_export_and_confirmed_apply(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("patch");
    std::fs::write(f.0.join("existing.txt"), "base\n").unwrap();
    f.commit("base");
    let patch = "diff --git a/added.txt b/added.txt\nnew file mode 100644\n--- /dev/null\n+++ b/added.txt\n@@ -0,0 +1 @@\n+created by patch\n";
    let patch_path = f.0.with_extension("patch");
    std::fs::write(&patch_path, patch).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_patch_file(window, cx));
    })
    .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![patch_path.clone()]));
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff.review.comparison.imported.is_some() && !p.diff.loading
        })
    });
    assert!(!f.0.join("added.txt").exists());
    let export = f.0.with_extension("export.patch");
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("diff-export-patch", cx);
    })
    .unwrap();
    cx.simulate_new_path_selection(|_| Some(export.clone()));
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.message.starts_with("补丁已导出"))
    });
    assert_eq!(std::fs::read_to_string(&export).unwrap(), patch);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("patch-apply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        cx.has_pending_prompt(),
        "external patch application must ask for confirmation"
    );
    cx.simulate_prompt_answer("应用补丁");
    settle(cx, None, |_| f.0.join("added.txt").exists());
    assert_eq!(
        std::fs::read_to_string(f.0.join("added.txt")).unwrap(),
        "created by patch\n"
    );
    std::fs::remove_file(patch_path).unwrap();
    std::fs::remove_file(export).unwrap();
}
