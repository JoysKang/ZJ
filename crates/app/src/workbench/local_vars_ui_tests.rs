//! Local variable tinting in a headless window on a temporary folder: the tint only appears
//! after typing pauses, and a language without local bindings records that it ran painting
//! nothing.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

/// Opens `source` as `name` in a fresh folder and settles until the tint has run.
fn tint(cx: &mut TestAppContext, name: &str, source: &str) -> (Option<u64>, bool) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-locals-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join(name);
    std::fs::write(&path, source).unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    let tint = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.active_document_id()
                .and_then(|id| p.document(id))
                .map(|doc| doc.locals_tint())
                .unwrap_or_default()
        })
    };
    settle(cx, Some(window), |cx| tint(cx).0.is_some());
    tint(cx)
}

#[gpui_kit::test]
async fn locals_are_tinted_after_the_pause(cx: &mut TestAppContext) {
    let (version, painted) = tint(
        cx,
        "main.rs",
        "fn main() {\n    let total = 1;\n    let sum = total + total;\n    sum\n}\n",
    );
    assert!(version.is_some(), "the tint never ran");
    assert!(painted, "a file with locals got no decorations");
}

#[gpui_kit::test]
async fn text_files_are_left_alone(cx: &mut TestAppContext) {
    let (version, painted) = tint(cx, "notes.txt", "let total = 1;\n");
    assert!(version.is_some(), "the tint never ran");
    assert!(!painted, "plain text has no bindings to tint");
}
