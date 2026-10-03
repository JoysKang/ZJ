//! ⌃G and `:N:C` in the command center, in a headless window on a temporary folder.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn press(cx: &mut TestAppContext, window: WindowHandle<Root>, key: &str) {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn input(cx: &mut TestAppContext, window: WindowHandle<Root>, text: &str) {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// The query and the labels of the rows the panel lists.
fn panel(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Option<(String, Vec<String>)> {
    this.read_with(cx, |p, cx| {
        let quick = p.quick_open.as_ref()?;
        let (items, filtered) = quick.items.as_ref()?;
        let labels = filtered.iter().map(|i| items[*i].label.clone()).collect();
        Some((quick.input.read(cx).value().to_string(), labels))
    })
}

#[gpui_kit::test]
async fn ctrl_g_goes_to_a_line_of_the_active_file(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-go-to-line-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("main.rs");
    std::fs::write(&path, "fn a() {}\nfn b() {}\n    fn c() {}\nfn d() {}\n").unwrap();
    let (window, this) = open(cx, root.clone());
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new(
            "ctrl-g",
            navigation::GoToLine,
            Some("WorkspaceEditor"),
        )])
    });

    // No file open: the row says so, and Enter keeps the panel open.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.focus_handle.focus(window, cx));
    })
    .unwrap();
    press(cx, window, "ctrl-g");
    assert_eq!(
        panel(cx, &this),
        Some((":".into(), vec!["请先打开一个文件".into()]))
    );
    press(cx, window, "enter");
    assert!(this.read_with(cx, |p, _| p.quick_open.is_some()));
    press(cx, window, "escape");

    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    let cursor = |cx: &mut TestAppContext| {
        editor.read_with(cx, |state, _| {
            let position = state.cursor_position();
            (position.line, position.character)
        })
    };

    // ⌃G types `:`; out of range explains the bounds (5 lines, the last one empty).
    press(cx, window, "ctrl-g");
    input(cx, window, "9");
    let (query, labels) = panel(cx, &this).unwrap();
    assert_eq!(query, ":9");
    assert_eq!(labels, ["当前行: 1，字符: 1。请输入 1 到 5 之间的行号。"]);
    press(cx, window, "enter");
    assert!(this.read_with(cx, |p, _| p.quick_open.is_some()));

    // `:3` and Enter: line 3, column 1, the editor focused, the jump in the back history.
    press(cx, window, "backspace");
    input(cx, window, "3");
    assert_eq!(panel(cx, &this).unwrap().1, ["转到第 3 行"]);
    press(cx, window, "enter");
    assert!(this.read_with(cx, |p, _| p.quick_open.is_none()));
    assert_eq!(cursor(cx), (2, 0));
    cx.update_window(window.into(), |_, window, cx| {
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();
    assert_eq!(this.read_with(cx, |p, _| p.nav_back.len()), 1);

    // Typed into ⌘P: `:2:4` puts the cursor at line 2, character 4.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_quick_open(window, cx));
    })
    .unwrap();
    input(cx, window, ":2:4");
    assert_eq!(panel(cx, &this).unwrap().1, ["转到第 2 行第 4 个字符"]);
    press(cx, window, "enter");
    assert_eq!(cursor(cx), (1, 3));
    // Deleting the `:` goes back to the file search.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_quick_open(window, cx));
    })
    .unwrap();
    input(cx, window, ":");
    assert!(panel(cx, &this).is_some());
    press(cx, window, "backspace");
    assert!(this.read_with(cx, |p, _| {
        p.quick_open
            .as_ref()
            .is_some_and(|quick| quick.items.is_none())
    }));
    press(cx, window, "escape");

    // A line far down is scrolled to the middle of the editor.
    let long = root.join("long.rs");
    let text: String = (1..=300).map(|n| format!("// line {n}\n")).collect();
    std::fs::write(&long, text).unwrap();
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(long, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    let editor = this.read_with(cx, |p, _| p.documents[1].editor.clone());
    press(cx, window, "ctrl-g");
    input(cx, window, "200");
    press(cx, window, "enter");
    settle(cx, Some(window), |cx| {
        editor.read_with(cx, |state, _| {
            state.visible_row_range().is_some_and(|rows| {
                let middle = (rows.start + rows.end) / 2;
                rows.contains(&199) && middle.abs_diff(199) <= 2
            })
        })
    });
    let _ = std::fs::remove_dir_all(&root);
}
