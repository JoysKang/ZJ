//! Diff navigation uses the displayed version, not the on-disk file.
use super::diff_navigation::Caret;
use super::test_support::{open_window, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{
    InputEvent as _, TestAppContext, base::Root, component::Theme, test::TestWindowExt,
};

fn show(
    cx: &mut TestAppContext,
    handle: WindowHandle<Root>,
    this: &Entity<Workbench>,
    patch: &str,
) {
    let patch: Arc<str> = patch.into();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let colors = theme::colors(cx);
            let doc = crate::diff_doc::DiffDoc::parse(
                &patch,
                Some("python"),
                &Theme::global(cx).highlight_theme,
                crate::diff_doc::ChangeColors {
                    inserted_text: colors.diff_added_text,
                    removed_text: colors.diff_deleted_text,
                },
            )
            .unwrap();
            p.diff.generation += 1;
            p.diff.tab = Some(DiffTab {
                checkpoint: None,
                label: "a.py".into(),
                tooltip: "a.py".into(),
                path: PathBuf::from("/private/tmp/zj-diff-navigation-a.py"),
                source: DiffSource::Local(patch.clone(), "原文 → 修改后"),
            });
            p.diff.source = Some((patch.clone(), Theme::global(cx).is_dark()));
            p.diff.raw = crate::partial_patch::parse(&patch).map(Arc::new);
            p.diff.doc = Some(Arc::new(doc));
            p.diff.caret = None;
            p.diff.inline = false;
            p.diff.selection = None;
            p.diff.focus.focus(window, cx);
            p.active = Pane::Diff;
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

const PATCH: &str = "--- a/a.py\n+++ b/a.py\n@@ -1,4 +1,5 @@\n+value = 1\n def helper(x):\n     return x\n \n-helper(0)\n+helper(1)\n";

#[gpui_kit::test]
async fn diff_mouse_drag_copies_only_selected_text(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "secondary-c",
            diff_ops::CopyDiff,
            Some("DiffEditor"),
        )]);
    });
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find(("diff-modified", 0usize)).bounds();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let start = point(
            bounds.left() + theme::DIFF_GUTTER + theme::DIFF_INDICATOR,
            bounds.center().y,
        );
        window.drag(start, start + point(metrics.column * 5., px(0.)), cx);
        window.press("cmd-c", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("value".into()),
            "dragging five characters must copy five characters, not the whole line"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_selection_copies_each_side_inline_and_original_unicode_tabs(cx: &mut TestAppContext) {
    use diff_view::DiffList;
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("secondary-c", diff_ops::CopyDiff, Some("DiffEditor")),
            KeyBinding::new("secondary-a", diff_ops::SelectAllDiff, Some("DiffEditor")),
        ]);
    });
    let patch = "--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,2 @@\n-alpha旧🙂\tend\n-second old\n+bravo新🙂\tend\n+second new\n";
    show(cx, handle, &this, patch);
    for (list, expected, suffix) in [
        (DiffList::Original, "alpha", "旧🙂\tend"),
        (DiffList::Modified, "bravo", "新🙂\tend"),
        (DiffList::Inline, "bravo", "新🙂\tend"),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff.inline = list == DiffList::Inline;
                p.diff.selection = None;
                cx.notify();
            });
            window.render_frame(cx);
            let doc = this.read(cx).diff.doc.as_ref().unwrap();
            let row = if list == DiffList::Inline {
                doc.inline
                    .iter()
                    .position(|r| r.kind == crate::diff_doc::LineKind::Added)
                    .unwrap()
            } else {
                0
            };
            let id = match list {
                DiffList::Original => "diff-original",
                DiffList::Modified => "diff-modified",
                DiffList::Inline => "diff-inline",
            };
            let bounds = window.find((id, row)).bounds();
            let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
            let start = point(
                bounds.left()
                    + theme::DIFF_GUTTER * if list == DiffList::Inline { 2. } else { 1. }
                    + theme::DIFF_INDICATOR,
                bounds.center().y,
            );
            let fifth = start + point(metrics.column * 5.25, px(0.));
            // Backward drag also stays within the exact character range.
            window.drag(fifth, start, cx);
            window.press("cmd-c", cx);
            assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), expected);
            window.drag(fifth, start + point(metrics.column * 40., px(0.)), cx);
            // This is the same Copy action as the native Edit menu.
            window.dispatch_action(Box::new(gpui_kit::component::input::Copy), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), suffix);
        cx.update_window(handle.into(), |_, window, cx| {
            let before = this.read(cx).diff.doc.as_ref().unwrap().new.text.clone();
            window.input("should stay read only", cx);
            window.press("backspace", cx);
            assert_eq!(this.read(cx).diff.doc.as_ref().unwrap().new.text, before);
            window.press("cmd-a", cx);
            window.press("cmd-c", cx);
            let copied = cx.read_from_clipboard().unwrap().text().unwrap();
            assert!(copied.starts_with(if list == DiffList::Modified {
                "bravo"
            } else {
                "alpha"
            }));
            assert!(copied.contains('\t'));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn diff_selection_crosses_lines_and_keeps_gutter_line_selection(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "secondary-c",
            diff_ops::CopyDiff,
            Some("DiffEditor"),
        )]);
    });
    show(
        cx,
        handle,
        &this,
        "--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,2 @@\n-alpha old\n-second old\n+alpha new\n+second new\n",
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let first = window.find(("diff-modified", 0usize)).bounds();
        let second = window.find(("diff-modified", 1usize)).bounds();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let x = first.left() + theme::DIFF_GUTTER + theme::DIFF_INDICATOR;
        let from = point(x + metrics.column * 2.25, first.center().y);
        let to = point(x + metrics.column * 6.25, second.center().y);
        for (from, to) in [(from, to), (to, from)] {
            window.drag(from, to, cx);
            window.press("cmd-c", cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "pha new\nsecond"
            );
        }
        // Shift-click extends the original character anchor across lines.
        for (position, shift) in [(from, false), (to, true)] {
            let modifiers = Modifiers {
                shift,
                ..Default::default()
            };
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        }
        window.press("cmd-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "pha new\nsecond"
        );
        // A plain text click leaves no selection and must not overwrite the clipboard.
        window.drag(from, from, cx);
        window.press("cmd-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "pha new\nsecond"
        );
        let gutter = |bounds: Bounds<Pixels>| {
            point(bounds.left() + theme::DIFF_GUTTER / 2., bounds.center().y)
        };
        window.drag(gutter(first), gutter(second), cx);
        window.press("cmd-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "alpha new\nsecond new"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_selection_layout_switch_then_select_all_uses_visible_side(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("secondary-c", diff_ops::CopyDiff, Some("DiffEditor")),
            KeyBinding::new("secondary-a", diff_ops::SelectAllDiff, Some("DiffEditor")),
        ]);
    });
    show(
        cx,
        handle,
        &this,
        "--- a/a.py\n+++ b/a.py\n@@ -1 +1 @@\n-x = 1\n+x = 2\n",
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.click("diff-layout", cx);
        window.render_frame(cx);
        let bounds = window.find(("diff-inline", 1usize)).bounds();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let from = point(
            bounds.left() + theme::DIFF_GUTTER * 2. + theme::DIFF_INDICATOR,
            bounds.center().y,
        );
        window.drag(from, from + point(metrics.column * 3.25, px(0.)), cx);
        assert!(this.read(cx).diff.selection.is_some());
        cx.write_to_clipboard(ClipboardItem::new_string("layout-sentinel".into()));
        window.click("diff-layout", cx);
        this.read(cx).diff.focus.clone().focus(window, cx);
        window.press("cmd-c", cx);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "layout-sentinel",
            "changing layout must not copy an invisible selection"
        );
        window.press("cmd-a", cx);
        window.press("cmd-c", cx);
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "x = 2");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_selection_single_line_drag_keeps_definition_navigation(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "f12",
            gpui_kit::component::input::GoToDefinition,
            Some("DiffEditor"),
        )])
    });
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        let row = this.read(cx).diff.doc.as_ref().unwrap().rows.len() - 1;
        let bounds = window.find(("diff-modified", row)).bounds();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let from = point(
            bounds.left() + theme::DIFF_GUTTER + theme::DIFF_INDICATOR + metrics.column * 0.25,
            bounds.center().y,
        );
        window.drag(from, from + point(metrics.column * 6., px(0.)), cx);
        let generation = this.read(cx).nav.generation;
        window.press("f12", cx);
        assert!(
            this.read(cx).nav.generation > generation,
            "F12 must navigate after selecting a name"
        );
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.diff
                .caret
                .is_some_and(|caret| !caret.old && caret.line == 1)
        })
    });
}

#[gpui_kit::test]
async fn diff_ctrl_g_selects_the_clicked_side_and_history_returns(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "ctrl-g",
            navigation::GoToLine,
            Some("WorkspaceEditor"),
        )])
    });
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        let row = this.read(cx).diff.doc.as_ref().unwrap().rows.len() - 1;
        let bounds = window.find(("diff-original", row)).bounds();
        // Stay inside the line-number gutter, clear of the sidebar resize hit area.
        let position = point(bounds.left() + theme::DIFF_GUTTER / 2., bounds.center().y);
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        assert!(this.read(cx).diff.caret.is_none());
        assert_eq!(
            this.read(cx).diff.selection.unwrap().list,
            diff_view::DiffList::Original
        );
        window.press("ctrl-g", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("1:5", cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, cx| {
        let quick = p.quick_open.as_ref().unwrap();
        assert_eq!(quick.input.read(cx).value().as_ref(), ":1:5");
        assert_eq!(
            quick.items.as_ref().unwrap().0[0].label,
            "转到第 1 行第 5 个字符"
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
        this.read_with(cx, |p, _| {
            assert!(p.quick_open.is_none());
            assert_eq!(p.active, Pane::Diff);
            assert_eq!(
                p.diff.caret,
                Some(Caret {
                    old: true,
                    line: 0,
                    column: 4
                })
            );
            assert_eq!(
                p.diff.selection.unwrap().list,
                diff_view::DiffList::Original
            );
            assert_eq!(p.documents.len(), 0);
        });
        this.update(cx, |p, cx| p.navigate_back(window, cx));
        assert_eq!(this.read(cx).diff.caret.unwrap().line, 3);
        this.update(cx, |p, cx| p.navigate_forward(window, cx));
        assert_eq!(this.read(cx).diff.caret.unwrap().line, 0);
        // Inline mode maps new-version line numbers through the paired context row.
        this.update(cx, |p, cx| {
            p.diff.inline = true;
            p.diff.caret = None;
            p.diff.selection = None;
            p.go_to_line(1, 4, window, cx);
        });
        let p = this.read(cx);
        let selected = p.diff.selection.unwrap();
        assert_eq!(selected.list, diff_view::DiffList::Inline);
        assert_eq!(
            p.diff.doc.as_ref().unwrap().inline[selected.head].other,
            Some(1)
        );
        assert_eq!(
            p.diff.caret.unwrap(),
            Caret {
                old: false,
                line: 1,
                column: 4
            }
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_cmd_click_and_f12_use_each_versions_definition(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "f12",
            gpui_kit::component::input::GoToDefinition,
            Some("DiffEditor"),
        )])
    });
    for (old, cmd) in [(true, true), (false, false)] {
        show(cx, handle, &this, PATCH);
        cx.update_window(handle.into(), |_, window, cx| {
            let row = this.read(cx).diff.doc.as_ref().unwrap().rows.len() - 1;
            let id = if old {
                "diff-original"
            } else {
                "diff-modified"
            };
            let bounds = window.find((id, row)).bounds();
            let position = point(
                bounds.left() + theme::DIFF_GUTTER + theme::DIFF_INDICATOR + px(2.),
                bounds.center().y,
            );
            let modifiers = Modifiers {
                platform: cmd,
                ..Default::default()
            };
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers,
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            if !cmd {
                window.press("f12", cx);
            }
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.diff
                    .caret
                    .is_some_and(|caret| caret.old == old && caret.line == usize::from(!old))
            })
        });
        this.read_with(cx, |p, _| {
            assert_eq!(p.active, Pane::Diff);
            assert!(p.documents.is_empty());
        });
    }
    // Clicking beyond the shaped text must not snap to the last identifier.
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        let row = this.read(cx).diff.doc.as_ref().unwrap().rows.len() - 1;
        window.click(("diff-modified", row), cx);
        assert!(this.read(cx).diff.caret.is_none());
        let generation = this.read(cx).nav.generation;
        window.press("f12", cx);
        assert_eq!(this.read(cx).nav.generation, generation);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn diff_multiple_definitions_keep_version_routing_and_reject_stale_picker(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let patch = "--- a/a.py\n+++ b/a.py\n@@ -1,5 +1,5 @@\n helper()\n def helper():\n-    return 0\n+    return 1\n def helper():\n     return 2\n";
    for replace_doc in [false, true] {
        show(cx, handle, &this, patch);
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff.caret = Some(Caret {
                    old: true,
                    line: 0,
                    column: 1,
                });
                p.diff_go_to_definition(window, cx);
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| p.quick_open.is_some())
        });
        cx.update_window(handle.into(), |_, window, cx| {
            this.read_with(cx, |p, _| {
                assert_eq!(
                    p.quick_open
                        .as_ref()
                        .unwrap()
                        .items
                        .as_ref()
                        .unwrap()
                        .0
                        .len(),
                    2
                )
            });
            if replace_doc {
                this.update(cx, |p, _| p.diff.generation += 1);
            }
            window.press("enter", cx);
            this.read_with(cx, |p, _| {
                assert!(p.documents.is_empty());
                assert_eq!(p.diff.caret.unwrap().line, if replace_doc { 0 } else { 1 });
            });
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn diff_delayed_definition_does_not_change_a_new_comparison(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff.caret = Some(Caret {
                old: true,
                line: 3,
                column: 1,
            });
            p.diff_go_to_definition(window, cx);
        })
    })
    .unwrap();
    show(cx, handle, &this, PATCH);
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message.is_empty())
    });
    this.read_with(cx, |p, _| {
        assert!(p.diff.caret.is_none());
        assert!(p.quick_open.is_none());
        assert!(p.documents.is_empty());
    });
}

#[gpui_kit::test]
async fn diff_definition_in_another_file_opens_the_file_and_returns_to_diff(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-diff-cross-{}", std::process::id()));
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let target = root.join("pkg/helpers.py");
    std::fs::write(&target, "def helper(x):\n    return x\n").unwrap();
    let index =
        crate::symbol_index::SymbolIndex::build(vec![target.clone()], &AtomicBool::new(false));
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    show(
        cx,
        handle,
        &this,
        "--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,2 @@\n from pkg.helpers import helper\n-helper(0)\n+helper(1)\n",
    );
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.nav.symbols = Some(Arc::new(index));
            p.diff.caret = Some(Caret {
                old: false,
                line: 1,
                column: 1,
            });
            p.diff_go_to_definition(window, cx);
        })
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| matches!(p.active, Pane::Document(_)))
    });
    this.read_with(cx, |p, _| assert_eq!(p.documents[0].path, target));
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.navigate_back(window, cx))
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Diff);
        assert_eq!(
            p.diff.caret.unwrap(),
            Caret {
                old: false,
                line: 1,
                column: 1
            }
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui_kit::test]
async fn deleted_file_definition_stays_in_original_inline_version(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let patch = "--- a/a.py\n+++ /dev/null\n@@ -1,4 +0,0 @@\n-def helper(x):\n-    return x\n-\n-helper(0)\n";
    show(cx, handle, &this, patch);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            assert!(p.diff_is_inline());
            p.diff.caret = Some(Caret {
                old: true,
                line: 3,
                column: 1,
            });
            // Local comparisons also work when only the source patch is retained.
            p.diff.raw = None;
            p.diff_go_to_definition(window, cx);
        })
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.diff.caret.is_some_and(|caret| caret.line == 0))
    });
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Diff);
        assert!(p.documents.is_empty());
        assert!(p.diff.caret.unwrap().old);
        assert_eq!(p.diff.selection.unwrap().list, diff_view::DiffList::Inline);
    });
}

#[gpui_kit::test]
async fn diff_ctrl_g_counts_original_characters_before_f12_with_or_without_raw(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-g", navigation::GoToLine, Some("WorkspaceEditor")),
            KeyBinding::new(
                "f12",
                gpui_kit::component::input::GoToDefinition,
                Some("DiffEditor"),
            ),
        ])
    });
    let patch = "--- a/a.py\n+++ b/a.py\n@@ -1,5 +1,5 @@\n def helper(x):\n \treturn x\n def caller():\n-\t图 = helper(0)\r\n+\t图 = helper(1)\r\n \treturn 图\n";
    for raw in [true, false] {
        show(cx, handle, &this, patch);
        cx.update_window(handle.into(), |_, window, cx| {
            if !raw {
                this.update(cx, |p, _| p.diff.raw = None);
            }
            window.press("ctrl-g", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.input("4:7", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
            this.read_with(cx, |p, _| {
                assert_eq!(
                    p.diff.caret,
                    Some(Caret {
                        old: false,
                        line: 3,
                        column: 11
                    })
                );
                assert_eq!(p.diff_line_info(), Some((5, 3, 6)));
            });
            window.press("f12", cx);
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.diff
                    .caret
                    .is_some_and(|caret| !caret.old && caret.line == 0)
            })
        });
        this.read_with(cx, |p, _| assert!(p.documents.is_empty()));
    }
}

#[gpui_kit::test]
async fn diff_history_survives_generation_change_and_skips_replaced_docs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    show(cx, handle, &this, PATCH);
    let stale = cx
        .update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff_place(
                    Caret {
                        old: true,
                        line: 3,
                        column: 0,
                    },
                    window,
                    cx,
                );
                p.go_to_line(0, 0, window, cx);
                p.diff.generation += 1;
                p.navigate_back(window, cx);
                assert_eq!(p.diff.caret.unwrap().line, 3);
                p.navigate_forward(window, cx);
                assert_eq!(p.diff.caret.unwrap().line, 0);
                p.nav.back.pop().unwrap()
            })
        })
        .unwrap();
    show(cx, handle, &this, PATCH);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff_place(
                Caret {
                    old: false,
                    line: 2,
                    column: 0,
                },
                window,
                cx,
            );
            p.go_to_line(0, 0, window, cx);
            p.nav.back.push(stale.clone());
            p.navigate_back(window, cx);
            assert_eq!(p.diff.caret.unwrap().line, 2);
            assert_eq!(p.nav.forward.len(), 1);
            p.nav.forward.push(stale.clone());
            p.navigate_forward(window, cx);
            assert_eq!(p.diff.caret.unwrap().line, 0);
            assert_eq!(p.nav.back.len(), 1);
            p.nav.back.clear();
            p.nav.forward.clear();
            p.nav.back.push(stale.clone());
            p.navigate_back(window, cx);
            assert_eq!(p.diff.caret.unwrap().line, 0);
            assert!(p.nav.forward.is_empty());
            p.nav.forward.push(stale);
            p.navigate_forward(window, cx);
            assert!(p.nav.back.is_empty());
        })
    })
    .unwrap();
}

#[gpui_kit::test]
async fn renamed_diff_original_side_uses_original_language_and_current_tab_route(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    for commit in [false, true] {
        show(cx, handle, &this, PATCH);
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let tab = p.diff.tab.as_mut().unwrap();
                tab.path.set_extension("txt");
                let path = PathBuf::from("renamed.txt");
                let original_path = Some(PathBuf::from("original.py"));
                tab.source = DiffSource::Git(Request {
                    repo: Repository {
                        id: RepoId(PathBuf::from("/private/tmp/zj-diff-rename/.git")),
                        common_dir: PathBuf::from("/private/tmp/zj-diff-rename/.git"),
                        worktree: PathBuf::from("/private/tmp/zj-diff-rename"),
                    },
                    generation: p.diff.generation,
                    operation: if commit {
                        workspace_editor_git::Operation::CommitDiff {
                            commit: "head".into(),
                            parent: None,
                            path,
                            original_path,
                        }
                    } else {
                        workspace_editor_git::Operation::Diff {
                            side: DiffSide::Worktree,
                            path,
                            original_path,
                        }
                    },
                });
                p.diff.caret = Some(Caret {
                    old: true,
                    line: 3,
                    column: 1,
                });
                p.diff_go_to_definition(window, cx);
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.diff
                    .caret
                    .is_some_and(|caret| caret.old && caret.line == 0)
            })
        });
        this.read_with(cx, |p, _| {
            assert_eq!(p.active, Pane::Diff);
            assert!(p.documents.is_empty());
            assert_eq!(
                p.diff.tab.as_ref().unwrap().path.extension().unwrap(),
                "txt"
            );
        });
    }
}

#[gpui_kit::test]
async fn diff_navigation_reveals_columns_and_returns_from_horizontal_scroll(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let (handle, this) = open_window(cx, None, crate::settings::Settings::default());
    let long = format!("{}界🙂", "x".repeat(300));
    let patch = format!(
        "--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,2 @@\n def helper(): return 1 # {long}\n-helper() # {long}\n+helper() # {long} changed\n"
    );
    for inline in [false, true] {
        show(cx, handle, &this, &patch);
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.diff.inline = inline;
                cx.notify();
            });
            window.render_frame(cx);
            this.update(cx, |p, cx| p.go_to_line(1, 200, window, cx));
            window.render_frame(cx);
            let p = this.read(cx);
            assert!(p.diff.scroll.0.borrow().base_handle.offset().x < px(-500.));
            assert_eq!(p.diff.caret.unwrap().column, 200);
            // Verify the column is visible through the same shaped-text hit test as clicks.
            let p = this.read(cx);
            let selection = p.diff.selection.unwrap();
            let id = if inline {
                "diff-inline"
            } else {
                "diff-modified"
            };
            let row = window.find((id, selection.head)).bounds();
            let font = gpui_kit::component::Theme::global(cx);
            let text = p.diff.doc.as_ref().unwrap().new.line_text(1);
            let prefix = &text[..200];
            let run = TextRun {
                len: prefix.len(),
                font: Font {
                    family: font.mono_font_family.clone(),
                    ..Default::default()
                },
                color: font.foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let x = window
                .text_system()
                .shape_line(prefix.to_owned().into(), font.mono_font_size, &[run], None)
                .width;
            let caret_x = row.left()
                + theme::DIFF_GUTTER * if inline { 2. } else { 1. }
                + theme::DIFF_INDICATOR
                + x;
            let viewport = p.diff.scroll.0.borrow().base_handle.bounds();
            assert!(caret_x >= viewport.left() && caret_x <= viewport.right());
            let position = point(caret_x + px(1.), row.center().y);
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Default::default(),
                    click_count: 1,
                }
                .to_platform_input(),
                cx,
            );
            assert_eq!(this.read(cx).diff.caret.unwrap().column, 200);
            this.update(cx, |p, cx| {
                p.diff.caret = Some(Caret {
                    old: false,
                    line: 1,
                    column: 1,
                });
                p.diff_go_to_definition(window, cx);
            });
        })
        .unwrap();
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| p.diff.caret.is_some_and(|caret| caret.line == 0))
        });
        this.read_with(cx, |p, _| {
            let offset = p.diff.scroll.0.borrow().base_handle.offset().x;
            assert!(
                offset >= -theme::DIFF_GUTTER && offset <= Pixels::ZERO,
                "definition is still horizontally hidden: {offset:?}"
            );
            assert_eq!(p.active, Pane::Diff);
        });
    }
}
