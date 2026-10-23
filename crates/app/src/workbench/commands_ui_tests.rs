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
fn rows(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<(String, Vec<String>)> {
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

#[gpui_kit::test]
async fn review_current_session_command_reports_empty_states(cx: &mut TestAppContext) {
    use super::test_support::{empty_store, install_globals, new_window};
    let (handle, this) = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default(), empty_store());
        let (handle, this) = new_window(
            cx,
            None,
            owners,
            Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.))),
        );
        (handle.downcast::<Root>().unwrap(), this)
    });
    for has_session in [false, true] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                if has_session {
                    p.agent_ensure_session(cx);
                } else {
                    p.agent.current = None;
                }
                p.open_quick_open_with(">审阅当前会话修改", window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        let matching = rows(cx, &this);
        assert_eq!(matching, [("Agent: 审阅当前会话修改".into(), Vec::new())]);
        press(cx, handle, "enter");
        this.read_with(cx, |p, _| {
            assert!(p.quick_open.is_none());
            assert!(p.diff.tab.is_none());
            assert_eq!(
                p.message,
                if has_session {
                    "当前会话没有待审阅的修改"
                } else {
                    "没有当前 Agent 会话可审阅"
                }
            );
        });
    }
}

#[gpui_kit::test]
async fn command_results_and_footer_stay_visible_in_a_low_window(cx: &mut TestAppContext) {
    use super::test_support::{empty_store, install_globals, new_window};
    let (handle, this) = cx.update(|cx| {
        let owners = install_globals(cx, crate::settings::Settings::default(), empty_store());
        let (handle, this) = new_window(
            cx,
            None,
            owners,
            Bounds::new(point(px(0.), px(0.)), size(px(760.), px(480.))),
        );
        (handle.downcast::<Root>().unwrap(), this)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_quick_open_with(">", window, cx));
        window.render_frame(cx);
        let count = commands::COMMANDS.len();
        assert!(count > theme::QUICK_OPEN_ROWS);
        assert_eq!(
            this.read(cx)
                .quick_open
                .as_ref()
                .unwrap()
                .items
                .as_ref()
                .unwrap()
                .1
                .len(),
            count
        );
        assert!(
            window.try_find(("pick-row", 0usize)).is_some(),
            "first command missing: viewport={:?}, panel={:?}, footer={:?}",
            window.viewport_size(),
            window.find("quick-open").bounds(),
            window.find("quick-open-footer").bounds()
        );
        for _ in 1..count {
            window.press("down", cx);
            window.render_frame(cx);
        }
        let selected = window.find(("pick-row", count - 1)).bounds();
        let panel = window.find("quick-open").bounds();
        let footer = window.find("quick-open-footer").bounds();
        assert!(panel.bottom() <= window.viewport_size().height);
        assert!(selected.top() >= panel.top() + theme::QUICK_OPEN_INPUT_HEIGHT);
        assert!(selected.bottom() <= footer.top());
        assert!(footer.bottom() <= window.viewport_size().height);
        assert_eq!(selected.size.height, theme::QUICK_OPEN_ROW_HEIGHT);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn the_command_center_is_centered_in_the_window_and_still_opens_search(
    cx: &mut TestAppContext,
) {
    use super::test_support::{empty_store, install_globals, new_window};
    use gpui_kit::component::{Theme, ThemeMode};
    for width in [760., 1400., 1728.] {
        let (handle, this) = cx.update(|cx| {
            let documents =
                install_globals(cx, crate::settings::Settings::default(), empty_store());
            let bounds = Bounds::new(point(px(120.), px(80.)), size(px(width), px(900.)));
            let (window, this) = new_window(cx, None, documents, bounds);
            (window.downcast::<Root>().unwrap(), this)
        });
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            for fullscreen in [false, true] {
                cx.update_window(handle.into(), |_, window, cx| {
                    Theme::change(mode, Some(window), cx);
                    if window.is_fullscreen() != fullscreen {
                        window.toggle_fullscreen();
                    }
                    assert_eq!(window.is_fullscreen(), fullscreen);
                    for visible in [true, false] {
                        this.update(cx, |p, cx| {
                            p.sidebar_visible = visible;
                            p.agent.visible = visible;
                            cx.notify();
                        });
                        window.render_frame(cx);
                        let center = window.find("command-center").bounds();
                        let midpoint = window.viewport_size().width / 2.;
                        assert!((center.center().x - midpoint).abs() <= px(0.5),
                            "search is not centered: width={width}, fullscreen={fullscreen}, visible={visible}, bounds={center:?}, midpoint={midpoint:?}");
                        for id in ["new-window", "toggle-sidebar", "toggle-terminal", "toggle-agent"] {
                            let control = window.find(id).bounds();
                            assert!(control.left() >= center.right(), "{id} overlaps search");
                            assert!(control.right() <= window.viewport_size().width);
                        }
                    }
                }).unwrap();
            }
        }
        cx.update_window(handle.into(), |_, window, cx| {
            window.click("command-center", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            this.read_with(cx, |p, _| p.quick_open.is_some()),
            "search button stopped opening quick open"
        );
    }
}
