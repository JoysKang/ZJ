//! The restricted viewer in a headless window, on a temporary folder: a file over 8 MB opens
//! read-only in the viewer tab, shows the lines on screen, follows a change on disk and closes;
//! a binary file is still refused.

use super::super::test_support::{open, settle};
use super::super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

fn held_line(this: &Entity<Workbench>, cx: &mut TestAppContext, index: usize) -> Option<String> {
    this.read_with(cx, |p, _| {
        let large = p.large.as_ref()?;
        let (first, lines) = &large.held;
        Some(lines.get(index.checked_sub(*first)?)?.text.clone())
    })
}

#[gpui_kit::test]
async fn a_large_file_opens_in_the_viewer_and_follows_the_disk(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-large-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("big.log");
    let line = format!("{}\n", "x".repeat(99));
    std::fs::write(&path, line.repeat(files::MAX_FILE_BYTES / 100 + 1)).unwrap();
    let lines = files::MAX_FILE_BYTES / 100 + 1;
    std::fs::write(root.join("blob.bin"), b"\0\x01\x02").unwrap();

    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| held_line(&this, cx, 0).is_some());
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Large);
        assert!(p.documents.is_empty());
        assert_eq!(
            p.large_status().unwrap(),
            format!("{lines} 行 · 8.4 MB · 只读")
        );
    });
    assert_eq!(held_line(&this, cx, 0).unwrap(), "x".repeat(99));

    // Another program appends: the watcher's check re-indexes and the new line shows.
    let mut grown = std::fs::read(&path).unwrap();
    grown.extend_from_slice(b"appended\n");
    std::fs::write(&path, grown).unwrap();
    let changed = std::collections::BTreeSet::from([path.clone()]);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.check_disk(Some(&changed), window, cx));
        this.update(cx, |p, cx| {
            p.large
                .as_ref()
                .unwrap()
                .scroll
                .scroll_to_item(lines, ScrollStrategy::Top);
            cx.notify();
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        held_line(&this, cx, lines).as_deref() == Some("appended")
    });

    // A binary file is not shown, and the viewer stays as it was.
    let folder = Some(root.clone());
    let blob = root.join("blob.bin");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(blob, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.message.starts_with("打开失败"))
    });
    this.read_with(cx, |p, _| assert!(p.message.contains("二进制")));

    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_editor(window, cx));
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.large.is_none());
        assert_eq!(p.active, Pane::Welcome);
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn the_viewer_goes_to_lines_copies_selected_lines_and_finds(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-large-ui2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("big.log");
    let lines = files::MAX_FILE_BYTES / 100 + 1;
    let mut text = String::new();
    for n in 0..lines {
        let body = if n == 50_000 { "NEEDLE" } else { "hay" };
        text.push_str(&format!("{n:08} {body:<90}\n"));
    }
    std::fs::write(&path, &text).unwrap();
    let (window, this) = open(cx, root.clone());
    let folder = Some(root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| held_line(&this, cx, 0).is_some());

    // ⌃G's target: line 1000 is selected and shown.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.go_to_line(999, 0, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| held_line(&this, cx, 999).is_some());
    this.read_with(cx, |p, _| {
        assert_eq!(p.large.as_ref().unwrap().selection, Some((999, 999)))
    });

    // ⇧-click extends the selection; ⌘C copies the whole lines.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.large_click(1001, true, window, cx);
            p.large_copy(cx);
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.message == "已复制 3 行")
    });
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert!(
        copied.starts_with("00000999 hay") && copied.ends_with("\n"),
        "{copied:?}"
    );
    assert_eq!(copied.lines().count(), 3);

    // ⌘F: the query is found below the selection (smart case: lowercase matches NEEDLE).
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_find(false, window, cx));
        window.render_frame(cx);
        window.input("needle", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.find_step(true, cx));
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            p.large.as_ref().unwrap().selection == Some((50_000, 50_000))
        })
    });
    let _ = std::fs::remove_dir_all(&root);
}
