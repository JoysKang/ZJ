//! Complete image files must reach the real open path and the rendered image element.
use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn closing_the_last_text_tab_keeps_an_image_reachable(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-image-close-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("a.txt"), "text").unwrap();
    std::fs::write(
        root.join("a.png"),
        crate::image_file::tests::png([0, 0, 0, 255]),
    )
    .unwrap();
    let (handle, this) = open(cx, root.clone());
    for name in ["a.png", "a.txt"] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_file(root.join(name), Some(root.clone()), window, cx)
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                if name.ends_with("png") {
                    !p.images.is_empty()
                } else {
                    !p.documents.is_empty()
                }
            })
        });
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find(("tab-close", 0usize)).label(),
            Some("关闭 a.txt")
        );
        window.click(("tab-close", 0usize), cx);
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            matches!(this.read(cx).active, Pane::Image(_)),
            "closing text must return to the remaining image"
        );
        assert!(window.find("image-pixels").visible());
    })
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn closing_all_mixed_tabs_does_not_reactivate_a_fixed_diff(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-close-mixed-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("a.txt"), "disk\n").unwrap();
    std::fs::write(
        root.join("a.png"),
        crate::image_file::tests::png([0, 0, 0, 255]),
    )
    .unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.write_to_clipboard(ClipboardItem::new_string("clipboard\n".into()));
    for _ in 0..2 {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.compare_with_clipboard(root.join("a.txt"), window, cx)
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| !p.diff.loading && p.diff.doc.is_some())
        });
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.pin_current_diff(window, cx));
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(root.join("a.png"), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.images.is_empty())
    });
    assert_eq!(this.read_with(cx, |p, _| p.archived_diffs.len()), 1);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_all_panes(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(
            p.diff.tab.is_none(),
            "close all must not restore an archived comparison"
        );
        assert!(p.archived_diffs.is_empty() && p.images.is_empty());
        assert_eq!(p.active, Pane::Welcome);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn close_others_keeps_the_requested_fixed_diff_after_discarding_text(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-close-fixed-dirty-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("a.txt"), "disk\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    for text in ["comparison A\n", "comparison B\n"] {
        cx.write_to_clipboard(ClipboardItem::new_string(text.into()));
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.compare_with_clipboard(root.join("a.txt"), window, cx)
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| !p.diff.loading && p.diff.doc.is_some())
        });
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.pin_current_diff(window, cx));
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(root.join("a.txt"), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.documents.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("unsaved", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(this.read_with(cx, |p, _| p.documents[0].dirty));
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let keep = p.archived_diffs[0].id;
            p.close_other_panes(Pane::SavedDiff(keep), window, cx);
        });
    })
    .unwrap();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(p.documents.is_empty() && p.archived_diffs.is_empty());
        assert_eq!(
            p.diff.doc.as_ref().unwrap().old.text,
            "comparison A\n",
            "close others must preserve the requested comparison"
        );
    });
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "disk\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn zoom_out_from_fit_uses_the_displayed_image_scale(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-image-fit-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("wide.png");
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2000,
        1000,
        image::Rgba([0, 0, 0, 255]),
    ))
    .save(&path)
    .unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.images.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let canvas = window.find("image-canvas").bounds().size;
        let fit = (f32::from(canvas.width) / 2000.).min(f32::from(canvas.height) / 1000.);
        assert!(
            fit > 0.125 && fit < 0.8,
            "fixture must fit below 80%: {fit}"
        );
        window.click("image-zoom-out", cx);
        window.render_frame(cx);
        let zoom = this.read(cx).images[0].zoom.unwrap();
        assert!(
            (zoom - fit / 1.25).abs() < 0.001,
            "zoom-out must start from fit {fit}, got {zoom}"
        );
        window.click("image-fit", cx);
        window.render_frame(cx);
        window.click("image-zoom-in", cx);
        assert!((this.read(cx).images[0].zoom.unwrap() - fit * 1.25).abs() < 0.001);
    })
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn project_images_open_zoom_refresh_and_close_without_text_buffers(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-image-open-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("actual.png");
    std::fs::write(&path, crate::image_file::tests::png([255, 0, 0, 255])).unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path.clone(), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| matches!(p.active, Pane::Image(_)))
    });
    let id = this.read_with(cx, |p, _| {
        assert!(p.documents.is_empty());
        assert_eq!(p.images.len(), 1);
        let image = &p.images[0];
        assert_eq!(image.path, path);
        assert_eq!((image.decoded.width, image.decoded.height), (4, 3));
        assert_eq!(
            &image.decoded.image.as_bytes(0).unwrap()[..4],
            &[0, 0, 255, 255]
        );
        image.id
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("image-pixels").bounds();
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(window.find("image-info").label().unwrap().contains("4 × 3"));
        window.click("image-original", cx);
        window.render_frame(cx);
        assert_eq!(this.read(cx).images[0].zoom, Some(1.));
        window.click("image-zoom-in", cx);
        window.render_frame(cx);
        assert_eq!(this.read(cx).images[0].zoom, Some(1.25));
    })
    .unwrap();
    std::fs::write(&path, crate::image_file::tests::png([0, 255, 0, 255])).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.refresh_images(None, window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.images[0].decoded.image.as_bytes(0).unwrap()[1] == 255
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_image(id, window, cx));
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.images.is_empty());
        assert_eq!(p.active, Pane::Welcome);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn damaged_images_do_not_install_a_successful_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-image-invalid-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("broken.png");
    std::fs::write(&path, b"\x89PNG\r\n\x1a\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message.contains("图片打开失败"))
    });
    this.read_with(cx, |p, _| {
        assert!(p.images.is_empty());
        assert_eq!(p.active, Pane::Welcome);
    });
    std::fs::remove_dir_all(root).unwrap();
}
