//! Line editing keys in a headless window, on a temporary `.rs` file.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

/// Opens `main.rs` with `text` in a temporary folder, focused, with main.rs's line editing
/// bindings (the test app has none of its own).
fn open_rust(
    cx: &mut TestAppContext,
    name: &str,
    text: &str,
) -> (PathBuf, WindowHandle<Root>, Entity<EditorState>) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("main.rs");
    std::fs::write(&path, text).unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update(|cx| cx.bind_keys(edit_key_bindings()));
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    (root, window, editor)
}

fn select(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    editor: &Entity<EditorState>,
    range: std::ops::Range<usize>,
) {
    cx.update_window(window.into(), |_, _, cx| {
        editor.update(cx, |state, cx| state.set_selected_range(range, cx));
    })
    .unwrap();
}

fn press(cx: &mut TestAppContext, window: WindowHandle<Root>, key: &str) {
    cx.update_window(window.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

fn text(cx: &mut TestAppContext, editor: &Entity<EditorState>) -> String {
    editor.read_with(cx, |state, _| state.text().to_string())
}

#[gpui_kit::test]
async fn cmd_slash_toggles_line_comments_and_undoes_in_one_step(cx: &mut TestAppContext) {
    let source = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";
    let (root, window, editor) = open_rust(cx, "comment", source);
    // Both `let` lines, the selection ending at the start of the `}` line.
    select(cx, window, &editor, 12..42);
    press(cx, window, "cmd-/");
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    // let a = 1;\n    // let b = 2;\n}\n"
    );
    assert_eq!(
        editor.read_with(cx, |state, _| state.selected_range()),
        12..48
    );
    press(cx, window, "cmd-/");
    assert_eq!(text(cx, &editor), source);
    press(cx, window, "cmd-/");
    press(cx, window, "cmd-z");
    assert_eq!(text(cx, &editor), source);
    let _ = std::fs::remove_dir_all(&root);
}
