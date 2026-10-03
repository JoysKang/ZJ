//! The terminal panel in a headless window, running `/bin/sh` on a real pty.

use super::agent::AgentStore;
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions, base::Root, test::TestWindowExt};

fn open(cx: &mut TestAppContext, root: PathBuf) -> (WindowHandle<Root>, Entity<Prototype>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::settings::Settings::default());
        cx.set_global(crate::watch::WatchService::default());
        cx.set_global(AgentStore {
            history: None,
            default_workspace: None,
        });
        let documents: DocumentOwners = Rc::new(RefCell::new(Default::default()));
        cx.set_global(OpenDocuments(documents.clone()));
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut this = Prototype::new(Some(root), service, documents, 1, window, cx);
                    this.terminals.set_shell(Some(crate::terminal::Shell::new(
                        "/bin/sh".into(),
                        Vec::new(),
                    )));
                    this
                })
            },
        )
        .map(|(window, this)| (window.downcast::<Root>().unwrap(), this))
        .unwrap()
    })
}

/// Renders and waits until `done` holds (the shell answers on its own thread).
fn settle(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    for _ in 0..300 {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the terminal never settled");
}

fn panes(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Vec<usize> {
    this.read_with(cx, |p, _| p.terminals.groups().map(<[_]>::len).collect())
}

fn focused_text(cx: &mut TestAppContext, this: &Entity<Prototype>) -> String {
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

    // Typed text goes through the input handler, Enter through the key path.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.input("echo zj-$((40+2)); pwd", cx);
        window.press("enter", cx);
    })
    .unwrap();
    let cwd = root.display().to_string();
    settle(cx, window, |cx| {
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
    settle(cx, window, |cx| panes(cx, &this) == [2]);
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
