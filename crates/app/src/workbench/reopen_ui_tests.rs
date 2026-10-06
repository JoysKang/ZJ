//! ⇧⌘T 重新打开关闭的标签，在无头窗口里、临时文件夹上。

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

fn names(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
    this.read_with(cx, |p, _| {
        p.documents.iter().map(|doc| doc.name()).collect()
    })
}

#[gpui_kit::test]
async fn shift_cmd_t_reopens_the_last_closed_tab_with_its_cursor(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), format!("{name} 内容 12345\n")).unwrap();
    }
    let (window, this) = open(cx, root.clone());
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("cmd-w", CloseEditor, Some("WorkspaceEditor")),
            KeyBinding::new("cmd-shift-t", ReopenClosedEditor, Some("WorkspaceEditor")),
        ])
    });
    for (count, name) in ["a.txt", "b.txt", "c.txt"].into_iter().enumerate() {
        let path = root.join(name);
        let folder = Some(root.clone());
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
        })
        .unwrap();
        settle(cx, Some(window), |cx| {
            this.read_with(cx, |p, _| p.documents.len() == count + 1)
        });
    }
    // The cursor in b.txt sits at UTF-8 offset 5, then c.txt is the active tab.
    let b = this.read_with(cx, |p, _| p.documents[1].id);
    let b_editor = this.read_with(cx, |p, _| p.documents[1].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.select_pane(Pane::Document(b), window, cx));
        b_editor.update(cx, |state, cx| state.set_selected_range(5..5, cx));
        let c = this.read(cx).documents[2].id;
        this.update(cx, |p, cx| p.select_pane(Pane::Document(c), window, cx));
    })
    .unwrap();

    // ⌘W closes c.txt; the file then disappears, so ⇧⌘T must skip it later.
    press(cx, window, "cmd-w");
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    std::fs::remove_file(root.join("c.txt")).unwrap();

    // ⌘W closes b.txt (active after c.txt closed), remembering its cursor.
    press(cx, window, "cmd-w");
    assert_eq!(names(cx, &this), ["a.txt"]);

    // ⇧⌘T skips the deleted c.txt and reopens b.txt with the cursor restored.
    press(cx, window, "cmd-shift-t");
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    assert_eq!(b_editor.read_with(cx, |state, _| state.cursor()), 5);
    assert_eq!(this.read_with(cx, |p, _| p.active), Pane::Document(b));

    // The stack is empty now; ⇧⌘T says so and changes nothing.
    press(cx, window, "cmd-shift-t");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    assert_eq!(
        this.read_with(cx, |p, _| p.message.clone()),
        "没有可以重新打开的编辑器"
    );

    // Untitled tabs do not enter the stack.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.new_untitled(window, cx));
    })
    .unwrap();
    assert_eq!(names(cx, &this).len(), 3);
    press(cx, window, "cmd-w");
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    press(cx, window, "cmd-shift-t");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn a_closed_file_from_outside_the_folder_reopens(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = std::env::temp_dir().join(format!("zj-reopen-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("work")).unwrap();
    let base = std::fs::canonicalize(base).unwrap();
    let outside = base.join("notes.md");
    std::fs::write(&outside, "# notes\n").unwrap();
    let (window, this) = open(cx, base.join("work"));
    // Opened from elsewhere (⌘O): read without the workspace folder.
    let path = outside.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, None, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_document(id, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.is_empty())
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.reopen_closed_tab(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.documents.first().is_some_and(|d| d.path == outside)
        })
    });
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui_kit::test]
async fn a_background_open_finishing_does_not_cancel_the_users_open(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-open-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    for name in ["restored.txt", "clicked.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let (window, this) = open(cx, root.clone());
    // The last session's active tab opens in the background while the user clicks another.
    let (restored, clicked) = (root.join("restored.txt"), root.join("clicked.txt"));
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_now(restored.clone(), window, cx);
            p.open_file(clicked.clone(), folder, window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    let _ = std::fs::remove_dir_all(&root);
}
