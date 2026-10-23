//! The terminal panel in a headless window, running `/bin/sh` on a real pty.

use super::test_support::{TICK, empty_store, open_window, wait};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn open(cx: &mut TestAppContext, root: PathBuf) -> (WindowHandle<Root>, Entity<Workbench>) {
    let (window, this) = open_window(cx, Some(root), Default::default(), empty_store());
    this.update(cx, |p, _| {
        p.terminals.set_shell(Some(crate::terminal::Shell::new(
            "/bin/sh".into(),
            Vec::new(),
        )));
    });
    (window, this)
}

/// Renders and waits until `done` holds (the shell answers on its own thread).
fn settle(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    this: &Entity<Workbench>,
    done: impl FnMut(&mut TestAppContext) -> bool,
) {
    wait(cx, Some(TICK), Some(window), done, |cx| {
        let screen = this.read_with(cx, |p, cx| {
            p.terminals
                .target(None)
                .and_then(|(group, pane)| p.terminals.pane(group, pane))
                .map(|view| view.read(cx).screen_text())
        });
        format!("the terminal never settled; focused screen: {screen:?}")
    });
}

fn panes(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<usize> {
    this.read_with(cx, |p, _| p.terminals.groups().map(<[_]>::len).collect())
}

fn focused_text(cx: &mut TestAppContext, this: &Entity<Workbench>) -> String {
    this.read_with(cx, |p, cx| {
        let (group, pane) = p.terminals.target(None).unwrap();
        p.terminals
            .pane(group, pane)
            .unwrap()
            .read(cx)
            .screen_text()
    })
}

fn act(cx: &mut TestAppContext, window: WindowHandle<Root>, action: impl Action) {
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_action(Box::new(action), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
async fn many_terminal_groups_keep_header_actions_inside_the_panel(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-terminal-header-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            for _ in 0..8 {
                p.new_terminal(window, cx);
            }
        });
    })
    .unwrap();
    assert_eq!(panes(cx, &this), vec![1; 8]);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("printf '\\033]0;very-long-terminal-title-for-a-deep-project-directory-and-running-command\\007'", cx);
        window.press("enter", cx);
    }).unwrap();
    settle(cx, handle, &this, |cx| {
        this.read_with(cx, |p, cx| {
            p.terminals
                .pane(7, 0)
                .unwrap()
                .read(cx)
                .title()
                .starts_with("very-long-terminal")
        })
    });
    cx.simulate_window_resize(handle.into(), size(px(720.), px(600.)));
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let right = window.viewport_size().width;
        assert_eq!(right, px(720.));
        let actions_left = window.find("terminal-new").bounds().left();
        for group in 0usize..8 {
            let bounds = window.find(("terminal-group", group)).bounds();
            assert!(
                bounds.size.height <= theme::SECTION_HEIGHT,
                "terminal group wraps outside its header: {bounds:?}"
            );
            assert!(bounds.size.width <= theme::TERMINAL_TAB_MAX_WIDTH);
            assert!(window.find("terminal-groups").bounds().right() <= actions_left);
        }
        for id in [
            "terminal-new",
            "terminal-split",
            "terminal-kill",
            "terminal-hide",
        ] {
            let button = window.find(id);
            assert!(button.visible());
            assert!(
                button.bounds().right() <= right,
                "{id} escapes the window: {:?}",
                button.bounds()
            );
        }
        for group in [0usize, 7] {
            this.update(cx, |p, cx| p.select_terminal_group(group, window, cx));
            window.render_frame(cx);
            assert!(window.find(("terminal-group", group)).visible());
        }
        window.click("terminal-hide", cx);
        this.read_with(cx, |p, _| assert!(!p.terminals.is_visible()));
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            for _ in 0..8 {
                p.kill_terminal(window, cx);
            }
        });
    })
    .unwrap();
    assert!(panes(cx, &this).is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn terminals_open_split_run_commands_and_close(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-terminal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open(cx, root.clone());

    // ⌃` with no terminal opens the panel with one, focused, in the workspace folder.
    act(cx, window, ToggleTerminal);
    assert_eq!(panes(cx, &this), [1]);
    this.read_with(cx, |p, _| assert!(p.terminals.is_visible()));

    // Cancelling a command palette returns input to the same terminal.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_quick_open_with(">", window, cx));
        window.render_frame(cx);
        window.press("escape", cx);
        this.read_with(cx, |p, cx| {
            assert!(p.quick_open.is_none());
            assert!(
                p.terminals
                    .pane(0, 0)
                    .unwrap()
                    .focus_handle(cx)
                    .is_focused(window)
            );
        });
        window.render_frame(cx);
        window.input("echo zj-$((40+2)); pwd", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let cwd = root.display().to_string();
    settle(cx, window, &this, |cx| {
        let text = focused_text(cx, &this);
        text.contains("zj-42") && text.contains(&cwd)
    });

    // ⌘\ splits beside it; ⌃⇧` starts a second group.
    act(cx, window, SplitTerminal);
    assert_eq!(panes(cx, &this), [2]);
    act(cx, window, NewTerminal);
    assert_eq!(panes(cx, &this), [2, 1]);
    this.read_with(cx, |p, _| assert_eq!(p.terminals.active(), 1));

    // A shell that exits closes its terminal; the trash button's command kills one.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("exit", cx);
        window.press("enter", cx);
    })
    .unwrap();
    settle(cx, window, &this, |cx| panes(cx, &this) == [2]);
    act(cx, window, KillTerminal);
    assert_eq!(panes(cx, &this), [1]);

    // ⌃` on a focused terminal hides the panel; again shows it; the last kill hides it.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.select_terminal_group(0, window, cx));
    })
    .unwrap();
    act(cx, window, ToggleTerminal);
    this.read_with(cx, |p, _| assert!(!p.terminals.is_visible()));
    act(cx, window, ToggleTerminal);
    this.read_with(cx, |p, _| assert!(p.terminals.is_visible()));
    act(cx, window, KillTerminal);
    assert!(panes(cx, &this).is_empty());
    this.read_with(cx, |p, _| assert!(!p.terminals.is_visible()));
    let _ = std::fs::remove_dir_all(&root);
}
