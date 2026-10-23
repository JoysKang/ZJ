//! The find widget in a headless window, on a temporary folder.

use super::super::test_support::{open, settle};
use super::super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn find_controls_fit_a_narrow_editor(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-find-width-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "needle needle\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_find(true, window, cx);
            p.find
                .query
                .update(cx, |input, cx| input.set_value("needle", window, cx));
            p.find
                .replacement
                .update(cx, |input, cx| input.set_value("done", window, cx));
            p.find_update(false, cx);
        });
    })
    .unwrap();
    for width in [1400., 560., 244., 1400., 244.] {
        this.update(cx, |p, cx| {
            p.sidebar_visible = width > 244.;
            cx.notify();
        });
        cx.simulate_window_resize(handle.into(), size(px(width), px(650.)));
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let editor = window.find("workbench-editor").bounds();
            assert_eq!(window.viewport_size().width, px(width));
            let query = window.find("find-query-field").bounds();
            let next = window.find("find-next").bounds();
            if width == 1400. {
                assert!(
                    next.top() < query.bottom(),
                    "wide find controls should share a row"
                );
            } else {
                assert!(editor.size.width <= px(360.));
                assert!(
                    next.top() >= query.bottom(),
                    "narrow find controls should wrap together"
                );
            }
            this.read_with(cx, |p, _| assert_eq!(p.find.label(), "1 / 2"));
            assert!(window.find("find-count").bounds().size.width > theme::FIND_BUTTON);
            for id in [
                "find-toggle-replace",
                "find-case",
                "find-word",
                "find-regex",
                "find-previous",
                "find-next",
                "find-selection",
                "find-close",
                "replace-one",
                "replace-all",
            ] {
                let control = window.find(id);
                assert!(control.visible(), "{id} is clipped");
                let bounds = control.bounds();
                assert!(
                    bounds.left() >= editor.left() && bounds.right() <= editor.right(),
                    "{id} escapes editor: {bounds:?}, editor={editor:?}"
                );
            }
        })
        .unwrap();
    }
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("replace-all", cx);
        this.read_with(cx, |p, cx| {
            assert_eq!(
                p.documents[0].editor.read(cx).text().to_string(),
                "done done\n"
            )
        });
        window.click("find-close", cx);
        this.read_with(cx, |p, _| assert!(!p.find.open));
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn find_in_selection_follows_edits_above_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-find-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "x\nx x\nx\n").unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    // Search "x" only in the second line ("x x", bytes 2..5).
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_find(false, window, cx);
            p.find
                .query
                .update(cx, |input, cx| input.set_value("x", window, cx));
        });
        editor.update(cx, |state, cx| state.set_selected_range(2..5, cx));
        this.update(cx, |p, cx| p.toggle_find_in_selection(cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        this.read_with(cx, |p, _| p.find.matches.clone()),
        [2..3, 4..5]
    );
    // Typing above the selection moves it; the matches move with it.
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "yy\n", window, cx)
        });
    })
    .unwrap();
    // The widget searches again once typing pauses.
    cx.executor()
        .advance_clock(super::FIND_REFRESH_DELAY + std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert_eq!(
        this.read_with(cx, |p, _| p.find.matches.clone()),
        [5..6, 7..8]
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn replacing_right_after_an_edit_uses_the_current_text(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-find-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "padding padding padding\nfoo 中文 foo\n").unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_find(true, window, cx);
            p.find
                .query
                .update(cx, |input, cx| input.set_value("foo", window, cx));
            p.find
                .replacement
                .update(cx, |input, cx| input.set_value("bar", window, cx));
            p.find_update(false, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(this.read_with(cx, |p, _| p.find.matches.len()), 2);
    // The first line goes; the old match offsets now point past "中" or past the end.
    // Replacing before the debounced search must use the current text.
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..24), "", window, cx)
        });
        this.update(cx, |p, cx| p.find_replace_all(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let text = editor.read_with(cx, |state, _| state.text().to_string());
    assert_eq!(text, "bar 中文 bar\n");
    let _ = std::fs::remove_dir_all(&root);
}
