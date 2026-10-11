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

#[gpui_kit::test]
async fn active_line_background_covers_the_editor_through_its_right_padding(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::{Theme, ThemeMode};
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-active-line-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("main.py");
    std::fs::write(&path, "from functools import lru_cache\n").unwrap();
    let (handle, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    let short = "from functools import lru_cache";
    let long = "from typing import Dict, Iterable, List, Optional, Set, Tuple ".repeat(32);
    for (mode, palette) in [
        (ThemeMode::Light, theme::LIGHT),
        (ThemeMode::Dark, theme::DARK),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            Theme::change(mode, Some(window), cx);
            // Changing the mode reloads Kit's defaults; restore ZJ's active-line
            // color as follow_appearance does in the application.
            Theme::update(cx, |theme| {
                std::sync::Arc::make_mut(&mut theme.highlight_theme)
                    .style
                    .editor_active_line = Some(rgb(palette.active_line).into());
            });
        })
        .unwrap();
        cx.run_until_parked();
        for (text, wrap, at_end) in [
            (short, false, false),
            (long.as_str(), false, false),
            (long.as_str(), false, true),
            (long.as_str(), true, false),
        ] {
            cx.update_window(handle.into(), |_, window, cx| {
                let offset = if at_end { text.len() } else { 0 };
                editor.update(cx, |state, cx| {
                    state.set_value(text, window, cx);
                    state.set_soft_wrap(wrap, window, cx);
                    state.set_cursor_position(
                        lsp_types::Position::new(0, offset as u32),
                        window,
                        cx,
                    );
                    state.focus(window, cx);
                });
                window.render_frame(cx);
                let frame = window.find("workbench-editor").bounds();
                let caret = editor.read(cx).range_to_bounds(&(offset..offset)).unwrap();
                let active: Hsla = rgb(palette.active_line).into();
                // Both padding edges belong to the current row, including beneath
                // the overlay scrollbar; the outer panel border remains separate.
                for edge in [
                    frame.left() + theme::INDICATOR + px(0.5),
                    frame.right() - theme::INDICATOR - px(0.5),
                ] {
                    let sample = point(edge, caret.center().y).scale(window.scale_factor());
                    let quads = window.painted_quads();
                    assert!(
                        quads.iter().any(|quad| {
                            quad.background == active.into()
                                && quad.bounds.contains(&sample)
                                && quad.content_mask.bounds.contains(&sample)
                        }),
                        "active-line padding is unpainted: mode={mode:?}, wrap={wrap}, at_end={at_end}, edge={edge:?}, frame={frame:?}, input={:?}",
                        editor.read(cx).input_bounds(),
                    );
                }
            })
            .unwrap();
        }
    }
    let _ = std::fs::remove_dir_all(root);
}
