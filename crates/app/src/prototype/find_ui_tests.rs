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
    cx.run_until_parked();
    assert_eq!(
        this.read_with(cx, |p, _| p.find.matches.clone()),
        [5..6, 7..8]
    );
    let _ = std::fs::remove_dir_all(&root);
}
