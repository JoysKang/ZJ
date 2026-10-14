//! Horizontal scrollbars must sit below the visible code viewport.
use super::test_support::{empty_store, open_window};
use super::*;
use crate::{
    diff_doc::{ChangeColors, DiffDoc},
    settings::Settings,
};
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{TestAppContext, test::TestWindowExt};

fn patch() -> String {
    let long = "result['needs_review'] = value + ".repeat(40);
    let mut patch = format!(
        "diff --git a/code.py b/code.py\n--- a/code.py\n+++ b/code.py\n@@ -1,160 +1,160 @@\n-old = {long}\n+new = {long}\n"
    );
    for _ in 1..160 {
        patch.push_str(&format!(" {long}\n"));
    }
    patch
}

#[gpui_kit::test]
async fn diff_horizontal_scrollbars_do_not_cover_visible_code(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, Settings::default(), empty_store());
    let patch: Arc<str> = patch().into();
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update_window(handle.into(), |_, window, cx| {
            Theme::change(mode, Some(window), cx);
            gpui_kit::base::Theme::global_mut(cx).scrollbar = gpui_kit::base::ScrollbarTheme::new()
                .with_mode(gpui_kit::base::ScrollbarMode::Always);
            this.update(cx, |p, cx| {
                let colors = theme::colors(cx);
                let doc = DiffDoc::parse(
                    &patch,
                    None,
                    &Theme::global(cx).highlight_theme,
                    ChangeColors {
                        inserted_text: colors.diff_added_text,
                        removed_text: colors.diff_deleted_text,
                    },
                )
                .unwrap();
                p.diff.tab = Some(DiffTab {
                    label: "code.py".into(),
                    tooltip: "code.py".into(),
                    path: "code.py".into(),
                    source: DiffSource::Local(patch.clone()),
                });
                p.diff.doc = Some(Arc::new(doc));
                p.active = Pane::Diff;
                cx.notify();
            });
        })
        .unwrap();
        cx.run_until_parked();
        for inline in [true, false] {
            for y in [-500., -100_000.] {
                cx.update_window(handle.into(), |_, window, cx| {
                    this.update(cx, |p, cx| {
                        p.diff.inline = inline;
                        p.diff.scroll = UniformListScrollHandle::new();
                        cx.notify();
                    });
                    window.render_frame(cx);
                    this.read(cx).diff.scroll.0.borrow().base_handle.set_offset(point(px(-120.), px(y)));
                    window.render_frame(cx);
                    window.render_frame(cx);
                    if y == -100_000. {
                        let pane = &this.read(cx).diff;
                        let handle = &pane.scroll.0.borrow().base_handle;
                        assert_eq!(handle.offset().y, -handle.max_offset().y);
                        let doc = pane.doc.as_ref().unwrap();
                        let rows = if inline {
                            vec![("diff-inline", doc.inline.len() - 1)]
                        } else {
                            vec![("diff-original", doc.rows.len() - 1), ("diff-modified", doc.rows.len() - 1)]
                        };
                        for (id, last) in rows {
                            let row = window.find((id, last)).bounds();
                            assert!(row.size.height > px(0.));
                            assert!(row.top() >= handle.bounds().top());
                            assert!(row.bottom() <= handle.bounds().bottom() + px(1.));
                        }
                    }
                    let scale = window.scale_factor();
                    let content = this.read(cx).diff.scroll.0.borrow().base_handle.bounds().scale(scale);
                    let spx = |value| px(value).scale(scale);
                    assert!(content.size.height > spx(100.) && content.size.width > spx(100.));
                    let bars: Vec<_> = window.painted_quads().into_iter().filter(|q| {
                        let b = q.bounds;
                        b.size.height >= spx(3.) && b.size.height <= spx(12.)
                            && b.size.width > spx(30.)
                            && b.origin.x >= content.origin.x - spx(1.)
                            && b.right() <= content.right() + spx(1.)
                            && b.origin.y >= content.bottom() - spx(20.)
                            && b.bottom() <= content.bottom() + spx(20.)
                    }).collect();
                    assert!(!bars.is_empty(), "horizontal thumb was not painted: {mode:?}/{inline}/{y}, content={content:?}");
                    for bar in bars {
                        assert!(content.bottom() <= bar.bounds.origin.y,
                            "horizontal scrollbar covers the visible code: {mode:?}/{inline}/{y}, content={content:?}, thumb={:?}", bar.bounds);
                    }
                }).unwrap();
            }
        }
    }
}
