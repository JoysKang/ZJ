//! Edit recovery in headless windows, with snapshots in a temporary directory.

use super::super::test_support::{TICK, empty_store, loaded, open_window, settle, wait};
use super::super::*;
use crate::recovery::{self, Op, Record};
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::TestAppContext;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zj-recovery-ui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

/// Waits in real time: the snapshot queue runs on a background thread.
fn until_disk(cx: &mut TestAppContext, what: &str, done: impl Fn() -> bool) {
    let what = what.to_string();
    wait(
        cx,
        None,
        None,
        |_| done(),
        move |_| format!("never: {what}"),
    );
}

#[gpui_kit::test]
async fn an_edit_is_snapshotted_after_a_pause_and_forgotten_once_saved(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = temp("snapshot");
    let (root, store) = (base.join("work"), base.join("recovery"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "one").unwrap();
    cx.update(|cx| super::install(store.clone(), cx));
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let open_path = path.clone();
    let folder = Some(root.clone());
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
            state.replace_text_in_range(Some(0..3), "two", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    let snapshot = store.join(recovery::file_name(&recovery::file_key(&path)));
    // Nothing before the pause.
    assert!(!snapshot.exists());
    cx.executor()
        .advance_clock(recovery::SNAPSHOT_DELAY + std::time::Duration::from_millis(10));
    cx.run_until_parked();
    until_disk(cx, "the snapshot is written", || snapshot.exists());
    let record = recovery::decode(&std::fs::read(&snapshot).unwrap()).unwrap();
    assert_eq!(record.text, "two");
    assert_eq!(record.root.as_deref(), Some(root.as_path()));

    // Saving makes it pointless.
    let save = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(save.await);
    cx.run_until_parked();
    until_disk(cx, "the snapshot is removed after saving", || {
        !snapshot.exists()
    });
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui_kit::test]
async fn snapshots_left_by_a_crash_come_back_and_can_be_discarded(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = temp("restore");
    let (root, store) = (base.join("work"), base.join("recovery"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "on disk").unwrap();
    // A file outside the window's folder (it was open in a window that is gone).
    let elsewhere = base.join("elsewhere.txt");
    std::fs::write(&elsewhere, "outside").unwrap();
    let outside = Record {
        key: recovery::file_key(&elsewhere),
        path: elsewhere.clone(),
        untitled: false,
        root: Some(base.join("gone")),
        text: "outside, edited".into(),
        written_at: 3,
    };
    recovery::apply(&store, &Op::Write(outside)).unwrap();
    let file = Record {
        key: recovery::file_key(&path),
        path: path.clone(),
        untitled: false,
        root: Some(root.clone()),
        text: "unsaved".into(),
        written_at: 1,
    };
    let untitled = Record {
        key: "untitled:1:2:3".into(),
        path: "Untitled-1".into(),
        untitled: true,
        root: None,
        text: "scratch".into(),
        written_at: 2,
    };
    recovery::apply(&store, &Op::Write(file.clone())).unwrap();
    recovery::apply(&store, &Op::Write(untitled.clone())).unwrap();
    cx.update(|cx| super::install(store.clone(), cx));
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 3)
    });
    let state = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, cx| {
            p.documents
                .iter()
                .map(|doc| {
                    (
                        doc.untitled,
                        doc.editor.read(cx).text().to_string(),
                        doc.dirty,
                        doc.recovered,
                    )
                })
                .collect::<Vec<_>>()
        })
    };
    let mut docs = state(cx);
    docs.sort();
    assert_eq!(
        docs,
        [
            (false, "outside, edited".to_string(), true, true),
            (false, "unsaved".to_string(), true, true),
            (true, "scratch".to_string(), true, true),
        ]
    );
    // 放弃，用磁盘上的版本: the file's text comes back and its snapshot goes.
    let id = this.read_with(cx, |p, _| {
        p.documents.iter().find(|d| !d.untitled).unwrap().id
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.reload_from_disk(id, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.document(id).is_some_and(|d| !d.dirty))
    });
    let snapshot = store.join(recovery::file_name(&file.key));
    until_disk(cx, "the discarded snapshot is removed", || {
        !snapshot.exists()
    });
    assert!(
        state(cx)
            .iter()
            .any(|(untitled, text, _, recovered)| !untitled && text == "on disk" && !recovered)
    );
    // Quitting after the answers removes what is left, right away.
    cx.update(super::forget_everything);
    assert!(recovery::load_all(&store).0.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

#[gpui_kit::test]
async fn restored_tabs_open_the_active_one_and_read_the_rest_when_chosen(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp("tabs");
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let tabs: Vec<PathBuf> = ["a.txt", "b.txt", "c.txt"].map(|n| root.join(n)).to_vec();
    let active = Some(root.join("b.txt"));
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.restore_tabs(tabs, active, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let names = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            let open: Vec<String> = p.documents.iter().map(|d| d.name()).collect();
            let pending: Vec<String> = p
                .pending_tabs
                .iter()
                .map(|t| t.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            (open, pending)
        })
    };
    // Only the active tab was read; the others wait in the tab bar.
    assert_eq!(
        names(cx),
        (vec!["b.txt".into()], vec!["a.txt".into(), "c.txt".into()])
    );
    let a = root.join("a.txt");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_pending_tab(a, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 2)
    });
    assert_eq!(
        names(cx),
        (vec!["b.txt".into(), "a.txt".into()], vec!["c.txt".into()])
    );
    let c = root.join("c.txt");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.drop_pending_tab(&c, window, cx));
    })
    .unwrap();
    assert_eq!(names(cx).1, Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&root);
}
