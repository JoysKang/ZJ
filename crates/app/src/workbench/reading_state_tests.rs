use super::super::test_support::{open_window, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};
use std::{fs, fs::FileTimes, sync::atomic::AtomicUsize};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zj-reading-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[gpui_kit::test]
fn reading_state_restores_each_pinned_layout_and_position(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let left = dir.0.join("layout-left.txt");
    let right = dir.0.join("layout-right.txt");
    fs::write(&left, "old line\n".repeat(300)).unwrap();
    fs::write(&right, "new line\n".repeat(300)).unwrap();
    let (window, this) = open_window(cx, None, crate::settings::Settings::default());
    let mut pins = Vec::new();
    for inline in [false, true] {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.show_diff_tab(
                    DiffTab {
                        label: "layout".into(),
                        tooltip: "layout".into(),
                        path: right.clone(),
                        source: DiffSource::Local(
                            super::super::comparison::local_patch(
                                &"old line\n".repeat(300),
                                &"new line\n".repeat(300),
                                &right,
                            ),
                            "layout",
                        ),
                        checkpoint: Some(ComparisonSource::Files {
                            left: left.clone(),
                            right: right.clone(),
                        }),
                    },
                    window,
                    cx,
                );
            });
        })
        .unwrap();
        settle(cx, Some(window), |cx| {
            this.read_with(cx, |p, _| !p.diff.loading)
        });
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff.inline = inline;
                p.diff
                    .scroll
                    .scroll_to_item_strict(100, ScrollStrategy::Top);
                cx.notify();
            });
            window.render_frame(cx);
        })
        .unwrap();
        let saved = this.read_with(cx, |p, _| pinned(&p.diff).unwrap());
        let json = ReadingSession {
            pinned: vec![saved],
            ..Default::default()
        }
        .to_json();
        assert_eq!(json["pinned"][0]["inline"], serde_json::json!(inline));
        pins.push(json["pinned"][0].clone());
    }
    let saved = ReadingSession::from_json(&serde_json::json!({"pinned": pins}));
    let (restored_window, restored) = open_window(cx, None, crate::settings::Settings::default());
    cx.update_window(restored_window.into(), |_, window, cx| {
        restored.update(cx, |p, cx| p.restore_reading(saved, window, cx));
    })
    .unwrap();
    settle(cx, Some(restored_window), |cx| {
        restored.read_with(cx, |p, _| !p.reading.restoring_pins)
    });
    let ids = restored.read_with(cx, |p, _| {
        p.archived_diffs
            .iter()
            .map(|saved| saved.id)
            .collect::<Vec<_>>()
    });
    for (id, inline) in ids.into_iter().zip([false, true]) {
        cx.update_window(restored_window.into(), |_, window, cx| {
            restored.update(cx, |p, cx| p.activate_saved_diff(id, window, cx));
        })
        .unwrap();
        settle(cx, Some(restored_window), |cx| {
            restored.read_with(cx, |p, _| !p.diff.loading)
        });
        restored.read_with(cx, |p, _| {
            assert_eq!(p.diff.inline, inline);
            assert_eq!(p.diff.visible_top_row(), 100);
        });
    }
}

#[test]
fn reading_state_file_comparison_reloads_sources_instead_of_persisting_text() {
    let dir = Fixture::new();
    let left = dir.0.join("a.txt");
    let right = dir.0.join("b.txt");
    fs::write(&left, "old\n").unwrap();
    fs::write(&right, "first\n").unwrap();
    let source = ComparisonSource::Files {
        left,
        right: right.clone(),
    };
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    fs::write(&right, "external\n").unwrap();
    let restored = restore_comparison(&source, &service).unwrap();
    let DiffSource::Local(patch, _) = restored.source else {
        panic!("expected local comparison")
    };
    assert!(patch.contains("+external"));
    assert!(!patch.contains("+first"));
    fs::remove_file(right).unwrap();
    assert!(restore_comparison(&source, &service).is_err());
}

#[test]
fn reading_state_git_comparison_resolves_current_local_refs() {
    let dir = Fixture::new();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir.0)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "--quiet"]);
    fs::write(dir.0.join("code.rs"), "first\n").unwrap();
    git(&["add", "code.rs"]);
    git(&["commit", "--quiet", "-m", "first"]);
    fs::write(dir.0.join("code.rs"), "second\n").unwrap();
    git(&["commit", "--quiet", "-am", "second"]);
    let source = ComparisonSource::Git {
        repo: dir.0.clone(),
        base: "HEAD~1".into(),
        target: "HEAD".into(),
        path: "code.rs".into(),
        original_path: None,
    };
    fs::write(dir.0.join("code.rs"), "third\n").unwrap();
    git(&["commit", "--quiet", "-am", "third"]);
    let service = GitService::new(1, Duration::from_secs(5)).unwrap();
    let DiffSource::Local(patch, _) = restore_comparison(&source, &service).unwrap().source else {
        panic!("expected Git patch")
    };
    assert!(patch.contains("-second"));
    assert!(patch.contains("+third"));
    for current_path in [None, Some(PathBuf::from("code.rs"))] {
        let source = ComparisonSource::History {
            repo: dir.0.clone(),
            commit: "HEAD~1".into(),
            path: "code.rs".into(),
            current_path: current_path.clone(),
        };
        let saved = ReadingSession {
            pinned: vec![PinnedComparison {
                source: source.clone(),
                top_row: 0,
                inline: Some(false),
            }],
            ..Default::default()
        };
        let restored = ReadingSession::from_json(&saved.to_json());
        assert_eq!(restored, saved);
        let tab = restore_comparison(&restored.pinned[0].source, &service).unwrap();
        match (current_path, tab.source) {
            (None, DiffSource::Snapshot(text)) => assert_eq!(&*text, "second\n"),
            (Some(_), DiffSource::Local(patch, _)) => {
                assert!(patch.contains("-second"));
                assert!(patch.contains("+third"));
            }
            _ => panic!("history source restored as another kind of view"),
        }
    }
    for parent in [None, Some("HEAD~1".to_owned())] {
        let source = ComparisonSource::Commit {
            repo: dir.0.clone(),
            commit: "HEAD".into(),
            parent: parent.clone(),
            path: "code.rs".into(),
            original_path: None,
        };
        let saved = ReadingSession {
            pinned: vec![PinnedComparison {
                source,
                top_row: 0,
                inline: Some(true),
            }],
            ..Default::default()
        };
        let restored = ReadingSession::from_json(&saved.to_json());
        assert_eq!(restored, saved);
        let tab = restore_comparison(&restored.pinned[0].source, &service).unwrap();
        let DiffSource::Git(request) = tab.source else {
            panic!("commit comparison not restored")
        };
        assert!(
            matches!(&request.operation, Operation::CommitDiff { parent: restored_parent, .. } if restored_parent == &parent)
        );
    }
    let missing = ComparisonSource::Git {
        repo: dir.0.clone(),
        base: "missing-ref".into(),
        target: "HEAD".into(),
        path: "code.rs".into(),
        original_path: None,
    };
    assert!(restore_comparison(&missing, &service).is_err());
}

#[gpui_kit::test]
fn reading_state_marks_reviewed_and_invalidates_equal_timestamp_edits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = Fixture::new();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&dir.0)
            .status()
            .unwrap()
            .success()
    );
    let path = dir.0.join("code.rs");
    fs::write(&path, "original\n").unwrap();
    let (window, this) = open_window(
        cx,
        Some(dir.0.clone()),
        crate::settings::Settings::default(),
    );
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.refresh_completed && !p.loading)
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_now(path.clone(), window, cx))
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.documents.is_empty())
    });
    let index_before = fs::read(dir.0.join(".git/index")).ok();
    cx.update_window(window.into(), |_, window, cx| {
        crate::session::track_at(Some(dir.0.join("session.json")), cx);
        crate::session::remember(window, Some(&dir.0), true, cx);
        window.render_frame(cx);
        window.click("review-mark", cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.review_status(&path) == "已读")
    });
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.reading
                .store_tail
                .as_ref()
                .is_some_and(|tail| tail.is_closed())
        })
    });
    assert_eq!(
        review_progress::load_store(&dir.0.join("review-progress.json"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(fs::read(dir.0.join(".git/index")).ok(), index_before);
    assert_eq!(fs::read_to_string(&path).unwrap(), "original\n");
    let saved = crate::session::load_from(&dir.0.join("session.json"));
    assert_eq!(saved[0].reading.reviewed.len(), 1);
    assert_eq!(
        saved[0].reading.reviewed[0].verify(),
        ReviewStatus::Reviewed
    );
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, "external\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    // Drive the same event hook used by native watcher batches; no timer is required.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.refresh_review(Some(std::slice::from_ref(&path)), window, cx)
        })
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.reading.task.is_none())
    });
    assert_eq!(
        this.read_with(cx, |p, _| p.review_status(&path).to_string()),
        "待审"
    );
}

#[gpui_kit::test]
fn reading_state_cleared_checkpoint_syncs_and_overrides_stale_sessions(cx: &mut TestAppContext) {
    use super::super::test_support::{install_globals, new_window};
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let path = dir.0.join("file.rs");
    fs::write(&path, "reviewed\n").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &path, None).unwrap();
    review_progress::update_store(&dir.0.join("review-progress.json"), &record, true).unwrap();
    let owners = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default());
        crate::session::track_at(Some(dir.0.join("session.json")), cx);
        owners
    });
    let mut views = Vec::new();
    for _ in 0..2 {
        let (handle, this) = cx.update(|cx| {
            new_window(
                cx,
                Some(dir.0.clone()),
                owners.clone(),
                Bounds::new(Point::default(), size(px(1400.), px(900.))),
            )
        });
        let window = handle.downcast::<gpui_kit::base::Root>().unwrap();
        settle(cx, Some(window), |cx| {
            this.read_with(cx, |p, _| p.review_status(&path) == "已读")
        });
        views.push((window, this));
    }
    let (window, this) = &views[0];
    cx.update_window((*window).into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff.tab = Some(DiffTab {
                label: "review".into(),
                tooltip: "review".into(),
                path: path.clone(),
                source: DiffSource::Local("".into(), "review"),
                checkpoint: None,
            });
            p.active = Pane::Diff;
            p.clear_reviewed(window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    for (_, view) in &views {
        view.read_with(cx, |p, _| assert_eq!(p.review_status(&path), "待审"));
    }
    assert!(
        review_progress::load_store(&dir.0.join("review-progress.json"))
            .unwrap()
            .is_empty()
    );
    let (handle, restored) = cx.update(|cx| {
        new_window(
            cx,
            Some(dir.0.clone()),
            owners,
            Bounds::new(Point::default(), size(px(1400.), px(900.))),
        )
    });
    cx.update_window(handle, |_, window, cx| {
        restored.update(cx, |p, cx| {
            p.restore_reading(
                ReadingSession {
                    reviewed: vec![record],
                    ..Default::default()
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    cx.run_until_parked();
    restored.read_with(cx, |p, _| assert_eq!(p.review_status(&path), "待审"));
}

#[gpui_kit::test]
fn reading_state_repository_checkpoint_outlives_its_workspace_window(cx: &mut TestAppContext) {
    use super::super::test_support::{install_globals, new_window};
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let path = dir.0.join("file.rs");
    fs::write(&path, "reviewed\n").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &path, None).unwrap();
    review_progress::update_store(&dir.0.join("review-progress.json"), &record, true).unwrap();
    let owners = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default());
        crate::session::track_at(Some(dir.0.join("session.json")), cx);
        owners
    });
    for _ in 0..2 {
        let (handle, this) = cx.update(|cx| {
            new_window(
                cx,
                Some(dir.0.clone()),
                owners.clone(),
                Bounds::new(Point::default(), size(px(1400.), px(900.))),
            )
        });
        let window = handle.downcast::<gpui_kit::base::Root>().unwrap();
        cx.update_window(handle, |_, window, cx| {
            this.update(cx, |p, cx| p.load_review_progress(window, cx))
        })
        .unwrap();
        settle(cx, Some(window), |cx| {
            this.read_with(cx, |p, _| p.review_status(&path) == "已读")
        });
        cx.update_window(handle, |_, window, _| window.remove_window())
            .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(
        review_progress::load_store(&dir.0.join("review-progress.json")).unwrap(),
        [record]
    );
}

#[gpui_kit::test]
fn reading_state_pinned_scroll_uses_source_rows_after_folding(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let left = dir.0.join("old.txt");
    let right = dir.0.join("new.txt");
    let old: String = (0..500).map(|n| format!("original line {n}\n")).collect();
    let new: String = (0..500)
        .map(|n| {
            if (100..300).contains(&n) {
                format!("changed line {n}\n")
            } else {
                format!("original line {n}\n")
            }
        })
        .collect();
    fs::write(&left, &old).unwrap();
    fs::write(&right, &new).unwrap();
    let patch = super::super::comparison::local_patch(&old, &new, &right);
    let (window, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "folding".into(),
                    tooltip: "folding".into(),
                    path: right.clone(),
                    source: DiffSource::Local(patch, "文件比较"),
                    checkpoint: Some(ComparisonSource::Files { left, right }),
                },
                window,
                cx,
            )
        })
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("diff-collapse", cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.diff_display_rows(false)
                .is_some_and(|rows| rows.len() < p.diff.doc.as_ref().unwrap().rows.len())
        })
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff.scroll.scroll_to_item_strict(50, ScrollStrategy::Top);
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
    this.read_with(cx, |p, cx| {
        let scroll = p.diff.scroll.0.borrow();
        let height = theme::diff_metrics(gpui_kit::component::Theme::global(cx).mono_font_size).row;
        let visible =
            (f32::from(-scroll.base_handle.offset().y) / f32::from(height)).floor() as usize;
        let saved = pinned(&p.diff).unwrap();
        assert_eq!(visible, 50);
        assert!(
            saved.top_row > visible + 50,
            "folded rows persisted a visible index instead of the original source row"
        );
    });
}

#[gpui_kit::test]
fn reading_state_restores_pinned_sources_without_cancelling_file_opens(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let left = dir.0.join("left.txt");
    let right = dir.0.join("right.txt");
    fs::write(&left, "left\n").unwrap();
    fs::write(&right, "external\n").unwrap();
    let (window, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_now(left.clone(), window, cx))
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (active, generation) = this.read_with(cx, |p, _| (p.active, p.file_generation));
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.restore_reading(
                ReadingSession {
                    pinned: vec![PinnedComparison {
                        source: ComparisonSource::Files {
                            left: left.clone(),
                            right: right.clone(),
                        },
                        top_row: 1,
                        inline: None,
                    }],
                    ..Default::default()
                },
                window,
                cx,
            )
        })
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.reading.restoring_pins)
    });
    let saved = this.read_with(cx, |p, _| {
        assert_eq!(p.active, active);
        assert_eq!(p.file_generation, generation);
        assert_eq!(p.archived_diffs.len(), 1);
        p.archived_diffs[0].id
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.select_pane(Pane::SavedDiff(saved), window, cx)
        })
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    this.read_with(cx, |p, _| {
        assert!(p.diff.doc.as_ref().unwrap().new.text.contains("external"))
    });
}

#[gpui_kit::test]
fn reading_state_restores_cursor_and_keeps_unopened_tabs_lazy(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let dir = Fixture::new();
    let path = dir.0.join("one.txt");
    let pending = dir.0.join("two.txt");
    fs::write(&path, "a readable line\n".repeat(200)).unwrap();
    fs::write(&pending, "unopened\n").unwrap();
    let (window, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.restore_reading(
                ReadingSession {
                    positions: vec![FilePosition {
                        path: path.clone(),
                        line: 40,
                        column: 4,
                        scroll_x: 0.,
                        scroll_y: -360.,
                    }],
                    ..Default::default()
                },
                window,
                cx,
            );
            p.restore_tabs(
                vec![path.clone(), pending.clone()],
                Some(path.clone()),
                window,
                cx,
            );
        })
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(p.pending_tabs, [pending]);
        let editor = p.documents[0].editor.read(cx);
        assert_eq!(
            editor.text().offset_to_position(editor.cursor()),
            lsp_types::Position::new(40, 4)
        );
        assert!((f32::from(editor.scroll_offset().y) + 360.).abs() < 1.);
    });
}
