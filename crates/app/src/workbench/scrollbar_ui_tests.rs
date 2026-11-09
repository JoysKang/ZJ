//! Horizontal scrollbars must sit below the visible code viewport.
use super::test_support::open_window;
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
async fn local_diff_context_names_both_sides_and_the_nested_repository(cx: &mut TestAppContext) {
    use super::test_support::{open, settle};
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-local-diff-context-{}", std::process::id()));
    let repo = root.join("nested-repo");
    let path = repo.join("src/sample.txt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "disk text\n").unwrap();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(&repo)
        .status()
        .unwrap();
    assert!(status.success());
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("nested-repo/src/sample.txt");
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let id = p.documents[0].id;
            p.documents[0].editor.update(cx, |editor, cx| {
                editor.set_value("unsaved buffer\n", window, cx)
            });
            p.compare_with_disk(id, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.active == Pane::Diff && p.diff.doc.is_some())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("diff-comparison").label(),
            Some("磁盘版本 → 未保存的修改")
        );
        assert_eq!(
            window.find("diff-file-path").label(),
            Some("nested-repo / src/sample.txt")
        );
    })
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("nested-repo/src/sample.txt")).unwrap(),
        "disk text\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn diff_context_keeps_actions_visible_in_narrow_windows(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, Settings::default());
    let patch: Arc<str> = patch().into();
    this.update(cx, |p, cx| {
        p.sidebar_visible = false;
        p.diff.tab = Some(DiffTab {
            checkpoint: None,
            label: "code.py (预览)".into(),
            tooltip: "project/deeply/nested/code.py".into(),
            path: "project/deeply/nested/code.py".into(),
            source: DiffSource::Local(patch.clone(), "磁盘版本 → 未保存的修改"),
        });
        p.diff.source = Some((patch.clone(), false));
        p.active = Pane::Diff;
        cx.notify();
    });
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        for width in [1400., 760., 360., 244.] {
            cx.simulate_window_resize(handle.into(), size(px(width), px(640.)));
            cx.update_window(handle.into(), |_, window, cx| {
                Theme::change(mode, Some(window), cx);
                window.render_frame(cx);
                window.render_frame(cx);
                window.render_frame(cx);
                assert_eq!(window.viewport_size().width, px(width));
                let context = window.find("diff-context").bounds();
                let tabs = window.find("editor-tabs").bounds();
                assert_eq!(tabs.size.height, theme::TAB_HEIGHT);
                assert_eq!(context.top(), tabs.bottom());
                for id in [
                    "diff-file-path",
                    "diff-comparison",
                    "diff-copy-patch",
                    "diff-layout",
                    "diff-open-file",
                    "diff-next",
                ] {
                    let bounds = window.find(id).bounds();
                    assert!(
                        bounds.size.width > px(0.)
                            && bounds.left() >= context.left()
                            && bounds.right() <= context.right()
                            && bounds.top() >= context.top()
                            && bounds.bottom() <= context.bottom(),
                        "{id} is outside context at {width}: {bounds:?}, {context:?}"
                    );
                }
                window.click("diff-copy-patch", cx);
            })
            .unwrap();
            cx.run_until_parked();
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(patch.to_string())
            );
        }
    }
}

#[gpui_kit::test]
async fn diff_horizontal_scrollbars_do_not_cover_visible_code(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, Settings::default());
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
                    checkpoint: None,
                    label: "code.py".into(),
                    tooltip: "code.py".into(),
                    path: "code.py".into(),
                    source: DiffSource::Local(patch.clone(), "磁盘版本 → 未保存的修改"),
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
