//! Per-file indentation in a headless window, on a temporary folder.

use super::test_support::{open, settle};
use super::*;
use crate::indent::Indent;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn fixture(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zj-indent-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(
        base.join("main.go"),
        "package main\n\nfunc main() {\n\tif true {\n\t\tprintln()\n\t}\n}\n",
    )
    .unwrap();
    std::fs::write(base.join("app.py"), "x = 1\n").unwrap();
    std::fs::write(base.join("web.ts"), "f(\n    a,\n    b,\n)\n").unwrap();
    std::fs::write(base.join(".editorconfig"), "[*.ts]\nindent_style = tab\n").unwrap();
    std::fs::canonicalize(base).unwrap()
}

fn open_doc(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    this: &Entity<Workbench>,
    root: &std::path::Path,
    name: &str,
) -> DocumentId {
    let count = this.read_with(cx, |p, _| p.documents.len());
    let path = root.join(name);
    let root = Some(root.to_path_buf());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, root, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == count + 1)
    });
    this.read_with(cx, |p, _| p.documents[count].id)
}

fn indent_of(cx: &mut TestAppContext, this: &Entity<Workbench>, id: DocumentId) -> Indent {
    this.read_with(cx, |p, _| p.document(id).unwrap().indent)
}

fn status_label(cx: &mut TestAppContext, window: WindowHandle<Root>) -> String {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window
            .find("status-indent")
            .label()
            .unwrap_or_default()
            .to_owned()
    })
    .unwrap()
}

/// Opens the status bar's indentation menu and clicks item `item` (separators count).
fn menu_click(cx: &mut TestAppContext, window: WindowHandle<Root>, item: usize) {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("status-indent", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(item, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

const USE_SPACES: usize = 0;
const WIDTH_8: usize = 5;

#[gpui_kit::test]
async fn indentation_follows_the_file_and_the_status_bar_menu(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("menu");
    let (window, this) = open(cx, root.clone());

    // Tabs found in the text; the language gives the width.
    let go = open_doc(cx, window, &this, &root, "main.go");
    assert_eq!(indent_of(cx, &this, go), Indent::tabs(4));
    assert_eq!(status_label(cx, window), "制表符长度: 4");

    // Nothing to detect: the language default.
    let py = open_doc(cx, window, &this, &root, "app.py");
    assert_eq!(indent_of(cx, &this, py), Indent::spaces(4));
    assert_eq!(status_label(cx, window), "空格: 4");

    // `.editorconfig` says tabs, the text gives the width.
    let ts = open_doc(cx, window, &this, &root, "web.ts");
    assert_eq!(indent_of(cx, &this, ts), Indent::tabs(4));

    // The menu changes only this buffer's setting, not its text.
    menu_click(cx, window, USE_SPACES);
    assert_eq!(indent_of(cx, &this, ts), Indent::spaces(4));
    menu_click(cx, window, WIDTH_8);
    assert_eq!(indent_of(cx, &this, ts), Indent::spaces(8));
    assert_eq!(status_label(cx, window), "空格: 8");
    assert_eq!(indent_of(cx, &this, go), Indent::tabs(4));
    let (text, dirty) = this.read_with(cx, |p, cx| {
        let doc = p.document(ts).unwrap();
        (doc.editor.read(cx).text().to_string(), doc.dirty)
    });
    assert_eq!(text, "f(\n    a,\n    b,\n)\n");
    assert!(!dirty);

    // The editor indents with it: Tab at the start of the buffer inserts 8 spaces.
    let editor = this.read_with(cx, |p, _| p.document(ts).unwrap().editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..0, cx);
        });
        window.render_frame(cx);
        window.press("tab", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let text = editor.read_with(cx, |state, _| state.text().to_string());
    assert_eq!(text, "        f(\n    a,\n    b,\n)\n");
    let _ = std::fs::remove_dir_all(&root);
}
