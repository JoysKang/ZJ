//! Line editing keys in a headless window, on a temporary `.rs` file.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{EntityInputHandler, TestAppContext, base::Root, test::TestWindowExt};

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

#[gpui_kit::test]
async fn alt_down_moves_and_shift_alt_down_copies_lines(cx: &mut TestAppContext) {
    let source = "fn main() {\n    let a = 1;\n    let b = 2;\n}";
    let (root, window, editor) = open_rust(cx, "move-lines", source);
    // The cursor inside `let a`.
    select(cx, window, &editor, 20..20);
    press(cx, window, "alt-down");
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    let b = 2;\n    let a = 1;\n}"
    );
    assert_eq!(
        editor.read_with(cx, |state, _| state.selected_range()),
        35..35
    );
    // Past the last line, which has no line break, nothing moves.
    press(cx, window, "alt-down");
    press(cx, window, "alt-down");
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    let b = 2;\n}\n    let a = 1;"
    );
    press(cx, window, "cmd-z");
    press(cx, window, "cmd-z");
    assert_eq!(text(cx, &editor), source);

    // ⇧⌥↓ copies, and the cursor lands on the copy.
    select(cx, window, &editor, 20..20);
    press(cx, window, "shift-alt-down");
    assert_eq!(
        text(cx, &editor),
        "fn main() {\n    let a = 1;\n    let a = 1;\n    let b = 2;\n}"
    );
    assert_eq!(
        editor.read_with(cx, |state, _| state.selected_range()),
        35..35
    );
    press(cx, window, "cmd-z");
    assert_eq!(text(cx, &editor), source);
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn cmd_d_selects_the_word_then_the_next_whole_word(cx: &mut TestAppContext) {
    let source = "fn main() {\n    let id = 1;\n    let idx = id;\n    id\n}\n";
    let (root, window, editor) = open_rust(cx, "select-next", source);
    let selected =
        |cx: &mut TestAppContext| editor.read_with(cx, |state, _| state.selected_range());
    // The cursor inside the first `id`.
    select(cx, window, &editor, 21..21);
    press(cx, window, "cmd-d");
    assert_eq!(selected(cx), 20..22);
    // Whole words: `idx` is skipped.
    press(cx, window, "cmd-d");
    assert_eq!(selected(cx), 42..44);
    press(cx, window, "cmd-d");
    assert_eq!(selected(cx), 50..52);
    press(cx, window, "cmd-d");
    assert_eq!(selected(cx), 50..52, "all occurrences are already selected");
    assert_eq!(
        editor.read_with(cx, |state, _| state.selected_ranges()),
        vec![(50..52, false), (42..44, false), (20..22, false)]
    );
    cx.update_window(window.into(), |_, window, cx| window.input("x", cx))
        .unwrap();
    assert_eq!(
        text(cx, &editor),
        source
            .replace("id =", "x =")
            .replace("= id", "= x")
            .replace("    id\n", "    x\n")
    );
    press(cx, window, "cmd-z");
    assert_eq!(
        editor
            .read_with(cx, |state, _| state.selected_ranges())
            .len(),
        3
    );
    assert_eq!(text(cx, &editor), source);
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn multi_edit_api_refuses_invalid_ranges_and_unconfirmed_input(cx: &mut TestAppContext) {
    let source = "αbc\nxyz";
    let (root, window, editor) = open_rust(cx, "multi-edit-validation", source);
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            let selections = vec![(0..2, true), (5..5, false)];
            assert!(state.set_selected_ranges(&selections, cx));
            for edits in [
                vec![(0..1, "x".into())],
                vec![(0..2, "x".into()), (0..2, "y".into())],
            ] {
                assert!(!state.replace_ranges_with_selections(&edits, &selections, window, cx));
                assert_eq!(state.text().to_string(), source);
                assert_eq!(state.selected_ranges(), selections);
            }
            assert!(!state.replace_ranges_with_selections(
                &[(0..2, "x".into())],
                &[(99..99, false)],
                window,
                cx
            ));
            assert_eq!(state.text().to_string(), source);
            state.replace_and_mark_text_in_range(None, "候", Some(1..1), window, cx);
            assert!(state.marked_text_range(window, cx).is_some());
            let composing = state.text().to_string();
            let before = state.selected_ranges();
            assert!(!state.set_selected_ranges(&[(0..0, false)], cx));
            assert!(!state.replace_ranges_with_selections(
                &[(0..0, "x".into())],
                &[(0..0, false)],
                window,
                cx
            ));
            assert_eq!(state.text().to_string(), composing);
            assert_eq!(state.selected_ranges(), before);
        });
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn multi_edit_commands_retain_all_cursors_and_undo_redo(cx: &mut TestAppContext) {
    let source = "let α = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;";
    let (root, window, editor) = open_rust(cx, "multi-line-edit", source);
    let original = vec![(4..6, true), (27..27, false)];
    for key in [
        "cmd-/",
        "alt-down",
        "alt-up",
        "shift-alt-down",
        "shift-alt-up",
    ] {
        cx.update_window(window.into(), |_, _, cx| {
            editor.update(cx, |state, cx| {
                assert!(state.set_selected_ranges(&original, cx));
            });
        })
        .unwrap();
        press(cx, window, key);
        let changed = text(cx, &editor);
        assert_ne!(changed, source, "{key}");
        let after = editor.read_with(cx, |state, _| state.selected_ranges());
        assert_eq!(after.len(), 2, "{key}");
        assert!(after[0].1, "selection direction: {key}");
        press(cx, window, "cmd-z");
        assert_eq!(text(cx, &editor), source, "one undo: {key}");
        assert_eq!(
            editor.read_with(cx, |state, _| state.selected_ranges()),
            original,
            "undo selections: {key}"
        );
        press(cx, window, "cmd-shift-z");
        assert_eq!(text(cx, &editor), changed, "redo text: {key}");
        assert_eq!(
            editor.read_with(cx, |state, _| state.selected_ranges()),
            after,
            "redo selections: {key}"
        );
        press(cx, window, "cmd-z");
    }
    let _ = std::fs::remove_dir_all(root);
}
