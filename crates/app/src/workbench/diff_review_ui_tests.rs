//! Reading filters and find keep the underlying patch/line indexes intact.
use super::super::test_support::{open_window, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn diff_selection_partial_first_row_does_not_highlight_unselected_prefix(
    cx: &mut TestAppContext,
) {
    let (handle, _) = open_window(cx, None, crate::settings::Settings::default());
    cx.update_window(handle.into(), |_, _, cx| {
        let paint = Paint {
            colors: theme::colors(cx),
            font: Theme::global(cx).mono_font_family.clone(),
            metrics: theme::diff_metrics(Theme::global(cx).mono_font_size),
            width: theme::DIFF_GUTTER,
        };
        for range in [2..usize::MAX, 2..6] {
            let (mut row, _) = line_row(&paint, Fill::Plain, &[Some(1)], None, Some(range));
            assert!(
                row.style().background.is_none(),
                "partial text selection must not fill the whole row"
            );
        }
        let (mut row, _) = line_row(&paint, Fill::Plain, &[Some(1)], None, Some(0..usize::MAX));
        assert!(row.style().background.is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_visible_top_uses_content_row_height_in_both_layouts(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let old = format!("{}old();\n", "context\n".repeat(500));
    let patch: Arc<str> = crate::save::text_patch(&old, &old.replace("old();", "new();")).into();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "top.txt".into(),
                    tooltip: "top.txt".into(),
                    path: PathBuf::from("/tmp/top.txt"),
                    source: DiffSource::Local(patch, "old → new"),
                    checkpoint: None,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    for inline in [false, true] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff.inline = inline;
                // This test measures raw row heights; default folding would change the row count.
                p.diff.review.collapse = false;
                p.diff_review_changed(window, cx);
                p.diff.scroll = UniformListScrollHandle::new();
                cx.notify();
            });
            window.render_frame(cx);
            this.update(cx, |p, cx| {
                let row =
                    theme::diff_metrics(gpui_kit::component::Theme::global(cx).mono_font_size).row;
                let mut scroll = p.diff.scroll.0.borrow_mut();
                scroll.deferred_scroll_to_item = None;
                scroll
                    .base_handle
                    .set_offset(point(Pixels::ZERO, -row * 50.));
            });
            this.read_with(cx, |p, _| {
                assert_eq!(p.diff.visible_top_row(), 50, "inline={inline}");
                assert_eq!(p.diff.source_row(p.diff.visible_top_row()), 50);
            });
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn a_fresh_diff_folds_unchanged_regions(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let old = format!("{}old();\n", "context\n".repeat(40));
    let patch: Arc<str> = crate::save::text_patch(&old, &old.replace("old();", "new();")).into();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "folded.txt".into(),
                    tooltip: "folded.txt".into(),
                    path: PathBuf::from("/tmp/folded.txt"),
                    source: DiffSource::Local(patch, "old → new"),
                    checkpoint: None,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && p.diff_review_current())
    });
    this.read_with(cx, |p, _| {
        assert!(p.diff.review.collapse, "a fresh diff starts folded");
        let rows = p.diff_display_rows(false).unwrap();
        assert!(rows.iter().any(|row| row.folded.is_some()));
        assert!(rows.len() < p.diff.doc.as_ref().unwrap().rows.len());
    });
}

#[gpui_kit::test]
async fn a_folded_diff_opens_at_its_first_change(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let mut old = String::new();
    for n in 0..40 {
        old.push_str(&"context\n".repeat(100));
        old.push_str(&format!("old_{n}();\n"));
    }
    let patch: Arc<str> = crate::save::text_patch(&old, &old.replace("old_", "new_")).into();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "first.txt".into(),
                    tooltip: "first.txt".into(),
                    path: PathBuf::from("/tmp/first.txt"),
                    source: DiffSource::Local(patch, "old → new"),
                    checkpoint: None,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff.doc.is_some() && p.diff_display_rows(false).is_some()
        })
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.diff.source_row(p.diff.visible_top_row()),
            0,
            "the first change must open near the top of the folded document"
        );
        assert_eq!(p.diff.change, Some(0));
    });
}

#[gpui_kit::test]
async fn diff_find_reveals_folded_matches_and_repeated_shortcuts_replace_query(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "cmd-f",
            super::super::find_widget::FindInFile,
            Some("WorkspaceEditor"),
        )])
    });
    let old = format!(
        "{}needle_in_context\n{}value = 1;  \nreal_old();\n",
        "context\n".repeat(15),
        "context\n".repeat(15)
    );
    let new = old
        .replace("value = 1;  ", "value = 1;")
        .replace("real_old", "real_new");
    let patch: Arc<str> = crate::save::text_patch(&old, &new).into();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "reading.txt".into(),
                    tooltip: "reading.txt".into(),
                    path: PathBuf::from("/tmp/reading.txt"),
                    source: DiffSource::Local(patch.clone(), "old → new"),
                    checkpoint: None,
                },
                window,
                cx,
            );
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff.inline = false;
            p.diff.review.collapse = true;
            p.diff.review.whitespace = Whitespace::Trailing;
            p.diff_review_changed(window, cx);
            p.diff.focus.focus(window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff_review_current())
    });
    this.read_with(cx, |p, _| {
        assert!(p.diff.review.pair.iter().any(|r| r.folded.is_some()))
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-f", cx);
        window.input("needle_in_context", cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| {
            p.diff_review_current() && p.diff.review.hits.len() == 2 && !p.diff.review.collapse
        })
    });
    this.read_with(cx, |p, _| {
        assert_eq!(p.diff.selection.unwrap().head, 15);
        assert_eq!(p.diff.source.as_ref().unwrap().0, patch);
        assert_eq!(
            p.diff.raw.as_ref().unwrap().lines.len(),
            crate::partial_patch::parse(&patch).unwrap().lines.len()
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-f", cx);
        window.input("real_new", cx);
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.diff.review.hits.len() == 1)
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(p.diff.review.find.read(cx).value().as_ref(), "real_new");
        assert!(p.diff.review.hits[0].0);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.toggle_diff_layout(window, cx));
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(p.diff.selection.unwrap().list, DiffList::Inline)
    });
}
