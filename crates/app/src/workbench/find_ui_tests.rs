//! The find widget in a headless window, on a temporary folder.

use super::super::test_support::{open, settle};
use super::super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

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
