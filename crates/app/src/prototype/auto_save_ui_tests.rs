//! 自动保存不会把磁盘上已删除的文件重新写回来，在无头窗口里、临时文件夹上。

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
