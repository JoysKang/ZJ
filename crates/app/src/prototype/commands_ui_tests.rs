//! ⇧⌘P 命令面板在无头窗口里的行为：预填 `>`、模糊匹配、执行、快捷键显示和最近使用。

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

/// The rows the panel lists: (label, keycap labels).
fn rows(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Vec<(String, Vec<String>)> {
    this.read_with(cx, |p, _| {
        let Some(quick) = p.quick_open.as_ref() else {
            return Vec::new();
        };
        let Some((items, filtered)) = &quick.items else {
            return Vec::new();
        };
        filtered
            .iter()
            .map(|i| (items[*i].label.clone(), items[*i].keys.clone()))
            .collect()
    })
}

#[gpui_kit::test]
async fn shift_cmd_p_runs_commands_and_remembers_recent(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-commands-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("main.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let (window, this) = open(cx, root.clone());
    // The test app has no key bindings of its own; register main.rs's.
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new(
                "cmd-shift-p",
                commands::ShowAllCommands,
                Some("WorkspaceEditor"),
            ),
            KeyBinding::new("cmd-p", QuickOpenFile, Some("WorkspaceEditor")),
            KeyBinding::new("alt-z", ToggleSoftWrap, Some("WorkspaceEditor")),
        ])
    });
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    assert!(!this.read_with(cx, |p, _| p.documents[0].soft_wrap));

    // ⇧⌘P pre-fills `>`; the whole table is listed in order, shortcuts queried live.
    press(cx, window, "cmd-shift-p");
    let all = rows(cx, &this);
    assert_eq!(all.len(), commands::COMMANDS.len());
    assert_eq!(all[0].0, "文件: 新建文件");
    let wrap = all.iter().find(|(name, _)| name == "视图: 切换自动换行");
    let expected = ["⌥".to_string(), "Z".to_string()];
    assert_eq!(
        wrap.map(|(_, keys)| keys.as_slice()),
        Some(expected.as_slice())
    );
    assert_eq!(
        this.read_with(cx, |p, cx| p.quick_open.as_ref().map(|q| q
            .input
            .read(cx)
            .value()
            .to_string())),
        Some(">".into())
    );

    // Fuzzy match, Enter runs it: the active document wraps, focus returns to the editor.
    input(cx, window, "自动换行");
    let matching = rows(cx, &this);
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].0, "视图: 切换自动换行");
    press(cx, window, "enter");
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.quick_open.is_none() && p.documents[0].soft_wrap
        })
    });
    cx.update_window(window.into(), |_, window, cx| {
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
    })
    .unwrap();

    // The just-run command leads the empty query (in memory only).
    press(cx, window, "cmd-shift-p");
    assert_eq!(rows(cx, &this)[0].0, "视图: 切换自动换行");
    press(cx, window, "escape");

    // Typing `>` into ⌘P enters command mode; deleting it returns to files.
    press(cx, window, "cmd-p");
    assert!(
        rows(cx, &this).is_empty()
            || this.read_with(cx, |p, _| p
                .quick_open
                .as_ref()
                .is_some_and(|q| q.items.is_none()))
    );
    input(cx, window, ">");
    assert_eq!(rows(cx, &this).len(), commands::COMMANDS.len());
    press(cx, window, "backspace");
    assert!(this.read_with(cx, |p, _| {
        p.quick_open.as_ref().is_some_and(|q| q.items.is_none())
    }));
    let _ = std::fs::remove_dir_all(&root);
}
