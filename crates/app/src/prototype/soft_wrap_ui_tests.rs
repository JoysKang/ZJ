//! Soft wrap defaults and ⌥Z in a headless window, on a temporary folder.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn code_does_not_wrap_prose_does_and_alt_z_toggles_one_buffer(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-soft-wrap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let files = [
        ("main.rs", "fn main() {}\n", false),
        ("notes.txt", "一行很长的文字\n", true),
        ("README.md", "# 标题\n", true),
    ];
    for (name, text, _) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    let (window, this) = open(cx, root.clone());
    for (count, (name, _, _)) in files.iter().enumerate() {
        let path = root.join(name);
        let root = Some(root.clone());
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.open_file(path, root, window, cx));
        })
        .unwrap();
        settle(cx, Some(window), |cx| {
            this.read_with(cx, |p, _| p.documents.len() == count + 1)
        });
    }
    let wraps = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.documents
                .iter()
                .map(|doc| doc.soft_wrap)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        wraps(cx),
        files.iter().map(|(.., wrap)| *wrap).collect::<Vec<_>>()
    );

    // ⌥Z in the code editor flips only that buffer and types nothing (main.rs's binding; the
    // test app has no key bindings of its own).
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "alt-z",
            ToggleSoftWrap,
            Some("WorkspaceEditor"),
        )])
    });
    let rust = this.read_with(cx, |p, _| p.documents[0].id);
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.select_pane(Pane::Document(rust), window, cx));
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
        window.press("alt-z", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(wraps(cx), [true, true, true]);
    let text = editor.read_with(cx, |state, _| state.text().to_string());
    assert_eq!(text, "fn main() {}\n");
    cx.update_window(window.into(), |_, window, cx| window.press("alt-z", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(wraps(cx), [false, true, true]);
    let _ = std::fs::remove_dir_all(&root);
}
