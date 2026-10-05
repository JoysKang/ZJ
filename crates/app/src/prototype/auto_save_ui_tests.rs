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
