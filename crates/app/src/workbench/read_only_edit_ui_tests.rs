use super::super::{
    test_support::{open, settle},
    *,
};
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

#[gpui_kit::test]
async fn read_only_find_and_line_commands_cannot_modify_the_buffer(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-readonly-edit-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    let source = "let alpha = 1;\nlet beta = alpha;\n";
    std::fs::write(&path, source).unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path.clone(), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let editor = p.documents[0].editor.clone();
            editor.update(cx, |state, cx| state.set_readonly(true, cx));
            p.find
                .query
                .update(cx, |input, cx| input.set_value("alpha", window, cx));
            p.open_find(true, window, cx);
            assert!(
                p.find.query.read(cx).focus_handle(cx).is_focused(window),
                "find must focus its visible query in a read-only buffer"
            );
            assert!(
                !p.find.replace_open,
                "read-only buffers must not enter replace mode"
            );
            p.find
                .query
                .update(cx, |state, cx| state.set_value("alpha", window, cx));
            p.find
                .replacement
                .update(cx, |state, cx| state.set_value("changed", window, cx));
            p.find_update(true, cx);
            assert_eq!(p.find.matches.len(), 2, "read-only find remains available");
            p.find_replace_one(window, cx);
            p.find_replace_all(window, cx);
            p.toggle_line_comment(window, cx);
            p.move_lines(true, window, cx);
            p.copy_lines(true, window, cx);
            assert_eq!(editor.read(cx).text().to_string(), source);
            assert!(!p.documents[0].dirty);
            p.select_next_occurrence(cx);
            assert_eq!(
                editor.read(cx).selected_ranges().len(),
                2,
                "selection remains available"
            );
        });
    })
    .unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), source);
    let _ = std::fs::remove_dir_all(root);
}
