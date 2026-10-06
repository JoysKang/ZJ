//! Replacing from the search view in a file whose buffer has unsaved edits: exactly the
//! results shown (ignored lines stay), and nothing when the buffer no longer matches them.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

#[gpui_kit::test]
async fn replacing_in_an_edited_buffer_follows_the_results(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-search-replace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "x 1\nx 2\nx 3\n").unwrap();
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
    let search = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.search
                    .query
                    .update(cx, |input, cx| input.set_value("x", window, cx));
                p.search
                    .replace
                    .input
                    .update(cx, |input, cx| input.set_value("y", window, cx));
                p.schedule_search(Duration::ZERO, window, cx);
            });
        })
        .unwrap();
        settle(cx, None, |cx| {
            this.read_with(cx, |p, _| {
                p.search.results.first().is_some_and(|f| f.lines.len() == 3)
            })
        });
    };
    let buffer = |cx: &mut TestAppContext| editor.read_with(cx, |s, _| s.text().to_string());

    // An edit after the results: they still line up. Ignore "x 2", then replace the file.
    search(cx);
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |s, cx| {
            s.replace_text_in_range(Some(12..12), "x 4\n", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.ignore_line(0, 1, cx);
            p.replace_file(0, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(buffer(cx), "y 1\nx 2\ny 3\nx 4\n");
    // The file on disk is left alone.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "x 1\nx 2\nx 3\n");

    // An edit above the results moves them: nothing is replaced, and it says why.
    search(cx);
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |s, cx| {
            s.replace_text_in_range(Some(0..0), "top\n", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.replace_file(0, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.search.replace.summary.is_some())
    });
    assert_eq!(buffer(cx), "top\ny 1\nx 2\ny 3\nx 4\n");
    let summary = this.read_with(cx, |p, _| p.search.replace.summary.clone().unwrap().0);
    assert!(summary.contains("对不上"), "{summary}");
    let _ = std::fs::remove_dir_all(&root);
}
