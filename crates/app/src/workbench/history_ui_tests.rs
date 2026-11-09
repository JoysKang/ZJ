use super::super::{
    test_support::{open, settle},
    *,
};
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn search_histories_are_independent_and_keep_drafts_and_result_navigation(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-input-history-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "alpha beta alpha beta\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update(|cx| cx.bind_keys(search_view::key_bindings()));
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    for workspace in [false, true] {
        for replacement in [false, true] {
            let first = format!(
                "{} {}",
                if workspace { "workspace" } else { "document" },
                if replacement { "replacement" } else { "query" }
            );
            for value in [&first, &"beta".to_string()] {
                cx.update_window(handle.into(), |_, window, cx| {
                    this.update(cx, |p, cx| {
                        if workspace {
                            p.open_search(window, cx);
                            p.search.replace.expanded = replacement;
                        } else {
                            p.open_find(replacement, window, cx);
                        }
                        let input = match (workspace, replacement) {
                            (false, false) => &p.find.query,
                            (false, true) => &p.find.replacement,
                            (true, false) => &p.search.query,
                            (true, true) => &p.search.replace.input,
                        };
                        input.update(cx, |input, cx| {
                            input.focus(window, cx);
                            input.select_all(window, cx);
                        });
                    });
                    window.render_frame(cx);
                    window.input(value, cx);
                    window.press("enter", cx);
                })
                .unwrap();
                cx.run_until_parked();
                // Enter confirms the value; returning to the editor exercises the next invocation.
                cx.update_window(handle.into(), |_, window, cx| {
                    this.update(cx, |p, cx| p.focus_active_editor(window, cx));
                    window.render_frame(cx);
                })
                .unwrap();
                cx.run_until_parked();
            }
            cx.update_window(handle.into(), |_, window, cx| {
                this.update(cx, |p, cx| {
                    let input = match (workspace, replacement) {
                        (false, false) => &p.find.query,
                        (false, true) => &p.find.replacement,
                        (true, false) => &p.search.query,
                        (true, true) => &p.search.replace.input,
                    };
                    input.update(cx, |input, cx| {
                        input.focus(window, cx);
                        input.select_all(window, cx);
                    });
                });
                window.render_frame(cx);
                window.press("alt-up", cx);
            })
            .unwrap();
            cx.run_until_parked();
            let value = |cx: &mut TestAppContext| {
                this.read_with(cx, |p, cx| match (workspace, replacement) {
                    (false, false) => p.find.query.read(cx).value().to_string(),
                    (false, true) => p.find.replacement.read(cx).value().to_string(),
                    (true, false) => p.search.query.read(cx).value().to_string(),
                    (true, true) => p.search.replace.input.read(cx).value().to_string(),
                })
            };
            assert_eq!(
                value(cx),
                first,
                "workspace={workspace}, replacement={replacement}"
            );
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.input("unfinished", cx);
            })
            .unwrap();
            cx.run_until_parked();
            assert_eq!(
                value(cx),
                "unfinished",
                "draft input: workspace={workspace}, replacement={replacement}"
            );
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.press("alt-up", cx);
            })
            .unwrap();
            cx.run_until_parked();
            assert_eq!(
                value(cx),
                "beta",
                "recall draft: workspace={workspace}, replacement={replacement}, history={}",
                this.read_with(cx, |p, _| format!("{:?}", p.find.query_history))
            );
            cx.update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.press("alt-down", cx);
            })
            .unwrap();
            cx.run_until_parked();
            assert_eq!(value(cx), "unfinished");
        }
    }
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_search(window, cx);
            p.search
                .query
                .update(cx, |input, cx| input.set_value("alpha", window, cx));
            p.schedule_search(Duration::ZERO, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.search.matches == 2)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("down", cx);
        assert!(
            !this
                .read(cx)
                .search
                .query
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        assert_eq!(
            this.read(cx).search.query.read(cx).value().to_string(),
            "alpha"
        );
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(root);
}
