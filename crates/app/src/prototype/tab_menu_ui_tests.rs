//! The editor tab's right-click menu in a headless window, on a temporary folder.

use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn fixture(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!("zj-tab-menu-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("sub")).unwrap();
    for name in ["a.txt", "b.txt", "sub/c.txt"] {
        std::fs::write(base.join(name), format!("{name}\n")).unwrap();
    }
    std::fs::canonicalize(base).unwrap()
}

fn open_files(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    this: &Entity<Prototype>,
    root: &std::path::Path,
) {
    for (count, name) in ["a.txt", "b.txt", "sub/c.txt"].into_iter().enumerate() {
        let path = root.join(name);
        let root = Some(root.to_path_buf());
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.open_file(path, root, window, cx));
        })
        .unwrap();
        settle(cx, None, |cx| {
            this.read_with(cx, |p, _| p.documents.len() == count + 1)
        });
    }
}

fn names(cx: &mut TestAppContext, this: &Entity<Prototype>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.documents
            .iter()
            .map(|doc| doc.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    })
}

/// Right-clicks tab `tab`, then clicks menu item `item` (separators count).
fn menu_click(
    cx: &mut TestAppContext,
    window: WindowHandle<Root>,
    this: &Entity<Prototype>,
    tab: usize,
    item: usize,
) {
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click(("tab-label", tab), cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        // The open menu holds focus, which keeps the tab's tooltip from covering it.
        let focus = this.read(cx).tab_menu_focus.clone().unwrap();
        assert!(focus.is_focused(window));
        window.within("popup-menu").click(item, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

const CLOSE: usize = 0;
const CLOSE_OTHERS: usize = 1;
const CLOSE_ALL: usize = 2;
const COPY_PATH: usize = 4;
const COPY_RELATIVE_PATH: usize = 5;
const REVEAL_IN_EXPLORER: usize = 8;

#[gpui_kit::test]
async fn the_tab_menu_closes_copies_and_reveals_the_clicked_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("menu");
    let (window, this) = open(cx, root.clone());
    open_files(cx, window, &this, &root);
    assert_eq!(names(cx, &this), ["a.txt", "b.txt", "c.txt"]);

    // The menu acts on the right-clicked tab, not the active one (c.txt).
    menu_click(cx, window, &this, 2, COPY_RELATIVE_PATH);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("sub/c.txt".into())
    );
    menu_click(cx, window, &this, 0, COPY_PATH);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(root.join("a.txt").display().to_string())
    );

    // 在资源管理器视图中显示 selects the file in the Explorer and makes its tab active.
    this.update(cx, |p, _| p.sidebar = Sidebar::Search);
    menu_click(cx, window, &this, 1, REVEAL_IN_EXPLORER);
    this.read_with(cx, |p, _| {
        assert!(p.sidebar == Sidebar::Explorer);
        assert_eq!(
            p.explorer.selection.as_deref(),
            Some(root.join("b.txt").as_path())
        );
        assert_eq!(p.active, Pane::Document(p.documents[1].id));
    });

    menu_click(cx, window, &this, 0, CLOSE);
    assert_eq!(names(cx, &this), ["b.txt", "c.txt"]);

    // 关闭其他 asks about edited documents first; 取消 keeps every tab.
    this.update(cx, |p, _| p.documents[1].dirty = true);
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["b.txt", "c.txt"]);
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["b.txt"]);
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Document(p.documents[0].id))
    });

    menu_click(cx, window, &this, 0, CLOSE_ALL);
    assert!(names(cx, &this).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn tab_shortcuts_act_on_the_active_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("keys");
    let (window, this) = open(cx, root.clone());
    open_files(cx, window, &this, &root);
    let dispatch = |cx: &mut TestAppContext, action: Box<dyn Action>| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.focus_handle.focus(window, cx));
            window.dispatch_action(action, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };

    dispatch(cx, Box::new(CopyActiveRelativePath));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("sub/c.txt".into())
    );
    dispatch(cx, Box::new(CloseOtherEditors));
    assert_eq!(names(cx, &this), ["c.txt"]);
    dispatch(cx, Box::new(CloseAllEditors));
    assert!(names(cx, &this).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
