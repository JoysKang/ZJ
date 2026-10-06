//! 保存与自动保存，在无头窗口里、临时文件夹上：自动保存不会把磁盘上已删除的文件重新写回来；
//! 手动保存写入磁盘并经过保存钩子（钩子延后执行，不在保存窗口更新期间重入）。

use super::test_support::{TICK, empty_store, loaded, open_window, settle, wait};
use super::*;
use crate::save::AutoSave;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

#[gpui_kit::test]
async fn focus_change_auto_save_leaves_a_deleted_file_deleted(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-auto-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("src/a.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let settings = Settings {
        auto_save: AutoSave::OnFocusChange,
        ..Settings::default()
    };
    let (window, this) = open_window(cx, Some(root.clone()), settings, empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });

    // Another program removes the file (and its folder) while the tab stays open, unedited.
    std::fs::remove_dir_all(root.join("src")).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.check_disk(None, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents[0].deleted)
    });
    assert!(this.read_with(cx, |p, _| p.documents[0].dirty));

    // Losing focus must not recreate it; only an explicit save does.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.auto_save_on_focus_change(None, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!path.exists());
    assert!(!root.join("src").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn saving_an_edited_buffer_writes_it_and_runs_the_saved_hooks(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_value("fn main() { 1; }\n", window, cx)
        });
        this.update(cx, |p, cx| p.save_document(id, false, window, cx).detach());
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.documents[0].dirty)
    });
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "fn main() { 1; }\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn a_save_during_another_save_waits_and_writes_the_newer_text(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-twice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    // An auto-save is writing "two" when the user types again and quits with 保存.
    let (first, second) = cx
        .update_window(window.into(), |_, window, cx| {
            // Typed edits (they mark the buffer edited), replacing the whole line.
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..3), "two", window, cx)
            });
            let first = this.update(cx, |p, cx| p.save_document(id, false, window, cx));
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..3), "three", window, cx)
            });
            let second = this.update(cx, |p, cx| p.save_document(id, false, window, cx));
            (first, second)
        })
        .unwrap();
    assert!(first.await);
    assert!(second.await, "the second save gave up instead of waiting");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "three\n");
    assert!(!this.read_with(cx, |p, _| p.documents[0].dirty));
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn an_open_ignored_file_follows_changes_on_disk(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-ignored-open-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(status.success());
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
    let path = root.join(".env");
    std::fs::write(&path, "A=1\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    // Another program (a terminal) rewrites the ignored file; the clean tab reloads.
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(&path, "A=2\n").unwrap();
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| editor.read_with(cx, |state, _| state.text() == "A=2\n"),
        |_| "the ignored file's tab never reloaded".into(),
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn a_focused_editor_is_still_when_nothing_happens(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-steady-caret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        editor.update(cx, |state, cx| state.focus(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    // The steady caret (vendor/gpui-base ZJ patch): no blink timer repaints an idle editor.
    let repaints = std::rc::Rc::new(std::cell::Cell::new(0));
    let counter = repaints.clone();
    let _watch = cx.update(|cx| cx.observe(&editor, move |_, _| counter.set(counter.get() + 1)));
    for _ in 0..6 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
    }
    assert_eq!(repaints.get(), 0, "the idle editor repainted");
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn after_save_as_the_original_file_opens_in_its_own_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-as-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (a, b) = (root.join("a.rs"), root.join("b.rs"));
    std::fs::write(&a, "fn a() {}\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "never loaded".into(),
    );
    let open = |cx: &mut TestAppContext, path: PathBuf| {
        let folder = Some(root.clone());
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
        })
        .unwrap();
    };
    open(cx, a.clone());
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    let save = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(id, b.clone(), window, cx))
        })
        .unwrap();
    assert!(save.await);
    assert_eq!(this.read_with(cx, |p, _| p.documents[0].path.clone()), b);
    // a.rs is another file now: it opens beside b.rs instead of focusing it.
    open(cx, a.clone());
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    this.read_with(cx, |p, _| assert!(p.documents.iter().any(|d| d.path == a)));
    let _ = std::fs::remove_dir_all(&root);
}
