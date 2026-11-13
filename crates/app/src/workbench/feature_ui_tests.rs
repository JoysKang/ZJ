//! End-to-end feature integration over disposable local files and repositories.
use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "zj-features-{name}-{}-{}",
            std::process::id(),
            crate::time::now_ms()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(std::fs::canonicalize(root).unwrap())
    }
    fn git(&self, args: &[&str]) {
        let result = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[gpui_kit::test]
async fn feature_preview_promotes_edits_and_drag_preserves_active_identity(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let f = Fixture::new("preview");
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(f.0.join(name), name).unwrap();
    }
    let (handle, this) = open(cx, f.0.clone());
    for name in ["a.txt", "b.txt"] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_preview_file(f.0.join(name), Some(f.0.clone()), false, window, cx)
            })
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.documents.len() == 1 && p.documents[0].path == f.0.join(name)
            })
        });
    }
    let edited = this.read_with(cx, |p, _| p.documents[0].id);
    cx.update_window(handle.into(), |_, window, cx| {
        let editor = this.read(cx).documents[0].editor.clone();
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..5), "unsaved b", window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents[0].dirty)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_preview_file(f.0.join("c.txt"), Some(f.0.clone()), false, window, cx)
        })
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.document(edited)
                .unwrap()
                .editor
                .read(cx)
                .text()
                .to_string(),
            "unsaved b"
        )
    });
    let active = this.read_with(cx, |p, _| p.active);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let from = window.find(("tab-label", 1usize)).bounds().center();
        let to = window.find(("tab-label", 0usize)).bounds().center();
        window.drag(from, to, cx);
        window.render_frame(cx);
        this.read_with(cx, |p, _| {
            assert_eq!(p.active, active);
            assert_eq!(p.tab_order, vec![f.0.join("c.txt"), f.0.join("b.txt")]);
        });
    })
    .unwrap();
    // A later explicit selection wins over an unfinished preview read.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_preview_file(f.0.join("a.txt"), Some(f.0.clone()), false, window, cx);
            p.select_pane(Pane::Document(edited), window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Document(edited)));
}

#[gpui_kit::test]
async fn feature_encoding_views_decode_refresh_and_refuse_all_saves(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("encoding");
    let path = f.0.join("utf16.txt");
    let bytes = [0xff, 0xfe, 0x2d, 0x4e, 0x87, 0x65];
    std::fs::write(&path, bytes).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path.clone(), Some(f.0.clone()), window, cx)
        })
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.documents.is_empty())
    });
    let id = this.read_with(cx, |p, cx| {
        let doc = &p.documents[0];
        assert_eq!(doc.encoding, Some("UTF-16LE"));
        assert!(!doc.editor.read(cx).is_editable());
        assert_eq!(doc.editor.read(cx).text().to_string(), "中文");
        doc.id
    });
    let destination = f.0.join("must-not-write.txt");
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.save_document(id, true, window, cx).detach();
            p.write_as(id, destination.clone(), window, cx).detach();
            p.toggle_line_ending(window, cx);
        })
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(!destination.exists());
    std::fs::write(&path, [0xff, 0xfe, 0x2d, 0x4e, 0x0a, 0x00]).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.check_disk(None, window, cx))
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, cx| {
            p.document(id).unwrap().editor.read(cx).text() == "中\n"
        })
    });
    this.read_with(cx, |p, _| assert!(!p.document(id).unwrap().dirty));
}

#[gpui_kit::test]
async fn feature_old_encoding_refresh_cannot_replace_a_new_view(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("encoding-race");
    let path = f.0.join("bytes.txt");
    std::fs::write(&path, [0x41, 0, 0x42, 0]).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_encoded(path.clone(), encoding_rs::UTF_16LE, window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.documents.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.refresh_encoded(None, window, cx);
            // A new view installs before the older background read completes.
            let doc = &mut p.documents[0];
            doc.encoding = Some("UTF-16BE");
            doc.editor
                .update(cx, |state, cx| state.set_value("䄀䈀", window, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, cx| {
        assert_eq!(p.documents[0].editor.read(cx).text().to_string(), "䄀䈀")
    });
}

#[gpui_kit::test]
async fn feature_git_image_diff_uses_index_worktree_and_keeps_fixed_pixels(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let f = Fixture::new("image-diff");
    f.git(&["init", "-q", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    let path = f.0.join("image.png");
    std::fs::write(&path, crate::image_file::tests::png([255, 0, 0, 255])).unwrap();
    f.git(&["add", "image.png"]);
    f.git(&["-c", "commit.gpgsign=false", "commit", "-qm", "red"]);
    std::fs::write(&path, crate::image_file::tests::png([0, 255, 0, 255])).unwrap();
    f.git(&["add", "image.png"]);
    std::fs::write(&path, crate::image_file::tests::png([0, 0, 255, 255])).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.groups.first().is_some_and(
                |g| matches!(&g.status, Some(Ok(status)) if !status.changes.is_empty()),
            )
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_diff(0, 0, DiffSide::Staged, window, cx))
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.image_diff.as_ref().is_some_and(|image| !image.loading)
        })
    });
    this.read_with(cx, |p, _| {
        let image = p.image_diff.as_ref().unwrap();
        assert_eq!(
            &image
                .left
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()[..4],
            &[0, 0, 255, 255]
        );
        assert_eq!(
            &image
                .right
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()[..4],
            &[0, 255, 0, 255]
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.pin_current_diff(window, cx);
            p.open_diff(0, 0, DiffSide::Worktree, window, cx);
        })
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.image_diff.as_ref().is_some_and(|image| !image.loading)
        })
    });
    let saved = this.read_with(cx, |p, _| {
        assert_eq!(p.archived_diffs.len(), 1);
        let image = p.image_diff.as_ref().unwrap();
        assert_eq!(
            &image
                .left
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()[..4],
            &[0, 255, 0, 255]
        );
        assert_eq!(
            &image
                .right
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()[..4],
            &[255, 0, 0, 255]
        );
        p.archived_diffs[0].id
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("image-diff-left").bounds().size.width > px(0.));
        assert!(window.find("image-diff-right").bounds().size.height > px(0.));
        this.update(cx, |p, cx| p.activate_saved_diff(saved, window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.image_diff.as_ref().is_some_and(|image| !image.loading)
        })
    });
    this.read_with(cx, |p, _| {
        assert_eq!(
            &p.image_diff
                .as_ref()
                .unwrap()
                .left
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .image
                .as_bytes(0)
                .unwrap()[..4],
            &[0, 0, 255, 255]
        )
    });
}

#[gpui_kit::test]
async fn feature_git_images_handle_added_deleted_and_renamed_versions(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    // Large fixtures share the process-wide pixel budget, so run both entrypoint checks in sequence.
    image_versions_share_budget_by_actual_size(cx);
    let f = Fixture::new("image-kinds");
    f.git(&["init", "-q", "-b", "main"]);
    f.git(&["config", "user.name", "Fixture"]);
    f.git(&["config", "user.email", "fixture@example.invalid"]);
    // 48 MB decoded: valid under the shared 64 MiB limit, larger than half of it.
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4000,
        3000,
        image::Rgba([255, 0, 0, 255]),
    ))
    .write_to(&mut png, image::ImageFormat::Png)
    .unwrap();
    let png = png.into_inner();
    std::fs::write(f.0.join("original.png"), &png).unwrap();
    f.git(&["add", "original.png"]);
    f.git(&["-c", "commit.gpgsign=false", "commit", "-qm", "initial"]);
    let (handle, this) = open(cx, f.0.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.groups.is_empty())
    });
    for (kind, path, original) in [
        ("added", "added.png", None),
        ("deleted", "original.png", None),
        ("renamed", "renamed.png", Some("original.png")),
    ] {
        f.git(&["reset", "--hard", "-q", "HEAD"]);
        match kind {
            "added" => {
                std::fs::write(f.0.join(path), &png).unwrap();
                f.git(&["add", path]);
            }
            "deleted" => f.git(&["rm", "-q", path]),
            _ => {
                // Keep this a real rename; a 48 MB → one-pixel rewrite is an add/delete to Git.
                std::fs::write(
                    f.0.join("original.png"),
                    crate::image_file::tests::png([255, 0, 0, 255]),
                )
                .unwrap();
                f.git(&["add", "original.png"]);
                f.git(&[
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-qm",
                    "small rename baseline",
                ]);
                f.git(&["mv", "original.png", path]);
            }
        }
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let repo = p.groups[0].repo.clone();
                p.prepare_new_diff(window, cx);
                p.diff.tab = Some(DiffTab {
                    checkpoint: None,
                    path: repo.worktree.join(path),
                    label: kind.into(),
                    tooltip: kind.into(),
                    source: DiffSource::Git(Request {
                        repo,
                        generation: 0,
                        operation: Operation::Diff {
                            path: path.into(),
                            original_path: original.map(PathBuf::from),
                            side: DiffSide::Staged,
                        },
                    }),
                });
                p.show_pane(Pane::Diff, window, cx);
                p.load_diff(window, cx);
            })
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.image_diff.as_ref().is_some_and(|image| !image.loading)
            })
        });
        this.read_with(cx, |p, _| {
            let image = p.image_diff.as_ref().unwrap();
            assert_eq!(
                image.left.as_ref().unwrap().is_none(),
                kind == "added",
                "{kind}: left"
            );
            assert_eq!(
                image.right.as_ref().unwrap().is_none(),
                kind == "deleted",
                "{kind}: right"
            );
        });
    }
}

#[gpui_kit::test]
async fn feature_preview_replaces_at_count_and_byte_limits(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("preview-limits");
    let (handle, this) = open(cx, f.0.clone());
    for i in 0..20 {
        let path = f.0.join(format!("{i}.txt"));
        std::fs::write(&path, "small file\n").unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_preview_file(path.clone(), Some(f.0.clone()), i != 19, window, cx)
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| p.documents.len() == i + 1)
        });
    }
    let replacement = f.0.join("replacement.txt");
    std::fs::write(&replacement, "replacement\n").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_preview_file(replacement.clone(), Some(f.0.clone()), false, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(
            p.documents.iter().any(|doc| doc.path == replacement),
            "{}",
            p.message
        );
        assert_eq!(p.documents.len(), 20);
        assert!(!p.documents.iter().any(|doc| doc.path == f.0.join("19.txt")));
    });
    // Exercise byte accounting at the install boundary without allocating 20 MiB of editor state.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let preview = p.tabs.preview.unwrap();
            let Pane::Document(id) = preview else {
                panic!("expected text preview")
            };
            p.documents.retain(|doc| doc.id == id);
            p.document_mut(id).unwrap().bytes = files::MAX_OPEN_BYTES;
            p.open_preview_file(
                f.0.join("missing.txt"),
                Some(f.0.clone()),
                false,
                window,
                cx,
            );
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert_eq!(p.documents[0].path, replacement));
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_preview_file(f.0.join("0.txt"), Some(f.0.clone()), false, window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents.len(), 1);
        assert_eq!(p.documents[0].path, f.0.join("0.txt"), "{}", p.message);
    });
}

#[gpui_kit::test]
async fn feature_image_preview_replaces_at_tab_limit(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("image-preview-limit");
    let (handle, this) = open(cx, f.0.clone());
    for i in 0..9 {
        let path = f.0.join(format!("{i}.png"));
        std::fs::write(&path, crate::image_file::tests::png([i, 0, 0, 255])).unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_preview_file(path.clone(), Some(f.0.clone()), i < 7, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        this.read_with(cx, |p, _| {
            assert!(
                p.images.iter().any(|image| image.path == path),
                "{}",
                p.message
            );
            assert_eq!(p.images.len(), (usize::from(i) + 1).min(8));
        });
    }
    this.read_with(cx, |p, _| {
        assert!(!p.images.iter().any(|image| image.path == f.0.join("7.png")))
    });
}

fn image_versions_share_budget_by_actual_size(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let f = Fixture::new("image-budget");
    let large = f.0.join("large.png");
    let small = f.0.join("small.png");
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4000,
        3000,
        image::Rgba([1, 2, 3, 255]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    std::fs::write(&large, bytes.into_inner()).unwrap();
    std::fs::write(&small, crate::image_file::tests::png([4, 5, 6, 255])).unwrap();
    let (handle, this) = open(cx, f.0.clone());
    use image_view::ImageVersion;
    for (left, right) in [
        (
            ImageVersion::Empty,
            ImageVersion::Disk(f.0.clone(), large.clone()),
        ),
        (
            ImageVersion::Disk(f.0.clone(), large.clone()),
            ImageVersion::Empty,
        ),
        (
            ImageVersion::Disk(f.0.clone(), large.clone()),
            ImageVersion::Disk(f.0.clone(), small.clone()),
        ),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_image_comparison(
                    DiffTab {
                        label: "budget".into(),
                        tooltip: "budget".into(),
                        path: large.clone(),
                        source: DiffSource::Local("".into(), "images"),
                        checkpoint: None,
                    },
                    left,
                    right,
                    window,
                    cx,
                )
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.image_diff.as_ref().is_some_and(|image| !image.loading)
            })
        });
        this.read_with(cx, |p, _| {
            let comparison = p.image_diff.as_ref().unwrap();
            assert!(
                comparison.left.is_ok(),
                "{:?}",
                comparison.left.as_ref().err()
            );
            assert!(
                comparison.right.is_ok(),
                "{:?}",
                comparison.right.as_ref().err()
            );
            assert!(comparison.decoded_bytes() >= 48_000_000);
        });
    }
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, _| p.clear_image_diff(window));
        window.remove_window();
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn a_failed_save_keeps_the_reason_in_one_dialog(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = Fixture::new("save-failure");
    let file = root.0.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    let (window, this) = open(cx, root.0.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(file.clone(), Some(root.0.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1 && !p.loading)
    });
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    // A directory where the atomic write wants its temp file makes the write fail with
    // something other than a permission error, so it is a real I/O failure.
    let blocker = root
        .0
        .join(format!(".a.txt.zj-save-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            // A typed edit marks the buffer edited; `set_value` would not.
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..3), "two", window, cx)
            });
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(!saved.await, "the write must fail");
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        let notice = p.notice.as_ref().expect("a failed save opens its reason");
        assert_eq!(notice.title, "无法保存「a.txt」");
        assert!(
            notice.detail.contains(&file.display().to_string()),
            "the reason names the file that was not written: {}",
            notice.detail
        );
    });
    std::fs::remove_dir(&blocker).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(p.notice.is_none(), "Escape closes the reason");
        assert!(p.documents[0].dirty, "the edit is still unsaved");
    });
}

#[gpui_kit::test]
async fn an_auto_save_failure_leaves_the_keyboard_in_the_editor(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = Fixture::new("auto-save-failure");
    let file = root.0.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    let (window, this) = open(cx, root.0.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(file.clone(), Some(root.0.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1 && !p.loading)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    let blocker = root
        .0
        .join(format!(".a.txt.zj-save-{}", std::process::id()));
    std::fs::create_dir(&blocker).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..3), "two", window, cx)
        });
        editor.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    // No ⌘S: the one-second auto-save writes it on its own, and fails.
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.deferred_notices.is_empty())
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        this.read_with(cx, |p, cx| {
            assert!(
                p.notice.is_none(),
                "an unasked-for save must not open a dialog over the editor"
            );
            assert_eq!(p.deferred_notices[0].title, "无法保存「a.txt」");
            assert!(p.documents[0].editor.focus_handle(cx).is_focused(window));
            assert!(
                window.find("status-save-failure").visible(),
                "the status bar carries the failure"
            );
        });
        // Typing still reaches the buffer while the failure waits.
        window.input("x", cx);
        this.read_with(cx, |p, cx| {
            assert_eq!(p.documents[0].editor.read(cx).text(), "twox\n");
            assert!(p.notice.is_none(), "typing does not open it");
        });
        window.click("status-save-failure", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        this.read_with(cx, |p, _| {
            let notice = p.notice.as_ref().expect("the entry opens the reason");
            assert_eq!(notice.title, "无法保存「a.txt」");
            assert!(
                p.notice_focus.is_focused(window),
                "opening it on purpose hands it the keyboard"
            );
        });
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.read_with(cx, |p, cx| {
            assert!(p.notice.is_none(), "Escape closes it");
            assert!(p.deferred_notices.is_empty(), "the entry is used up");
            assert!(p.documents[0].dirty, "the edit is still unsaved");
            assert!(p.documents[0].editor.focus_handle(cx).is_focused(window));
        });
    })
    .unwrap();
    std::fs::remove_dir(&blocker).unwrap();
}

#[gpui_kit::test]
async fn an_auto_save_that_meets_a_changed_file_does_not_prompt(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = Fixture::new("auto-save-conflict");
    let file = root.0.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    let (window, this) = open(cx, root.0.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(file.clone(), Some(root.0.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1 && !p.loading)
    });
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    // Another program writes the file and the auto-save runs before the watcher can
    // update the snapshot it compares against.
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            std::fs::write(&file, "外部写入\n").unwrap();
            this.update(cx, |p, cx| p.save_automatically(id, window, cx))
        })
        .unwrap();
    assert!(!saved.await, "the auto-save must not overwrite the file");
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "an unasked-for save must not open a native prompt"
    );
    this.read_with(cx, |p, _| {
        assert!(p.notice.is_none());
        let notice = p
            .deferred_notices
            .first()
            .expect("the conflict waits in the status bar");
        assert_eq!(notice.title, "无法保存「a.txt」");
        assert!(notice.detail.contains("磁盘上"), "{}", notice.detail);
    });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "外部写入\n");
}
