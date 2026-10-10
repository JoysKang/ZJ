//! File selection and Finder drops through GPUI, with filesystem operations in temporary folders.
use super::test_support::{open, settle};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{
    FileDropEvent, InputEvent as _, TestAppContext, VisualTestContext, test::TestWindowExt,
};
use std::{collections::BTreeSet, fs, path::Path};

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("zj-explorer-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("workspace/dest")).unwrap();
    fs::create_dir_all(root.join("finder")).unwrap();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
        fs::write(root.join("workspace").join(name), name).unwrap();
    }
    fs::canonicalize(root).unwrap()
}

fn row_position(
    cx: &mut TestAppContext,
    handle: WindowHandle<gpui_kit::base::Root>,
    this: &Entity<Workbench>,
    path: &Path,
) -> Point<Pixels> {
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let index = this
            .read(cx)
            .explorer
            .rows
            .iter()
            .position(|r| r.entry.path == path)
            .unwrap();
        window.find(("tree-row", index)).bounds().center()
    })
    .unwrap()
}

fn selected(cx: &TestAppContext, this: &Entity<Workbench>) -> Vec<PathBuf> {
    this.read_with(cx, |p, _| p.explorer.selected.iter().cloned().collect())
}

#[gpui_kit::test]
async fn explorer_clicks_select_ranges_and_batch_commands_use_the_whole_selection(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("selection");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    let command = Modifiers {
        platform: true,
        ..Default::default()
    };
    let shift = Modifiers {
        shift: true,
        ..Default::default()
    };
    let a = row_position(cx, handle, &this, &root.join("a.txt"));
    let b = row_position(cx, handle, &this, &root.join("b.txt"));
    let c = row_position(cx, handle, &this, &root.join("c.txt"));
    let d = row_position(cx, handle, &this, &root.join("d.txt"));
    // A normal click previews; modified clicks only select and do not open more documents.
    visual.simulate_click(a, Modifiers::default());
    visual.simulate_click(c, command);
    assert_eq!(
        selected(cx, &this),
        [root.join("a.txt"), root.join("c.txt")]
    );
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    assert_eq!(selected(cx, &this).len(), 2);
    visual.simulate_click(
        d,
        Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(
        selected(cx, &this),
        [root.join("a.txt"), root.join("c.txt"), root.join("d.txt")]
    );
    visual.simulate_click(b, shift);
    assert_eq!(
        selected(cx, &this),
        [root.join("b.txt"), root.join("c.txt")]
    );
    visual.update(|window, cx| {
        window.render_frame(cx);
        let index = this
            .read(cx)
            .explorer
            .rows
            .iter()
            .position(|r| r.entry.path == root.join("b.txt"))
            .unwrap();
        assert_eq!(window.find(("tree-row", index)).selected(), Some(true));
        this.update(cx, |p, cx| {
            p.copy_selection(false, cx);
            p.copy_selection_path(true, cx);
        });
        assert_eq!(
            cx.global::<explorer_ops::FileClipboard>().paths,
            [root.join("b.txt"), root.join("c.txt")]
        );
        window.dispatch_action(Box::new(RenameFile), cx);
        assert!(this.read(cx).explorer.edit.is_none());
    });
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "b.txt\nc.txt"
    );
    // Toggling off the last selection must not resurrect the active-document highlight.
    visual.simulate_click(b, command);
    visual.simulate_click(c, command);
    assert!(selected(cx, &this).is_empty());
    visual.update(|window, cx| {
        window.render_frame(cx);
        for index in 1..this.read(cx).explorer.rows.len() {
            assert_eq!(window.find(("tree-row", index)).selected(), Some(false));
        }
        window.dispatch_action(Box::new(SelectAllFiles), cx);
    });
    assert_eq!(selected(cx, &this).len(), 5);
    // Right-clicking a selected row preserves the batch; an unselected row replaces it.
    visual.simulate_mouse_down(c, MouseButton::Right, Modifiers::default());
    assert_eq!(selected(cx, &this).len(), 5);
    visual.update(|window, cx| {
        this.update(cx, |p, cx| {
            p.explorer
                .set_selected(vec![root.join("a.txt"), root.join("c.txt")]);
            p.delete_selection(window, cx);
        });
    });
    assert!(cx.has_pending_prompt());
    assert!(cx.pending_prompt().unwrap().0.contains("2 个"));
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert!(root.join("a.txt").exists() && root.join("c.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn finder_drops_copy_into_folder_file_parent_and_blank_space(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drop");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    for (name, row, destination) in [
        ("folder.txt", Some(root.join("dest")), root.join("dest")),
        ("a.txt", Some(root.join("a.txt")), root.clone()),
        ("blank.txt", None, root.clone()),
    ] {
        let source = base.join("finder").join(name);
        fs::write(&source, "from Finder").unwrap();
        let position = if let Some(row) = row {
            row_position(cx, handle, &this, &row)
        } else {
            cx.update_window(handle.into(), |_, window, _| {
                let header = window.find("explorer-new-file").bounds();
                point(header.center().x, header.bottom() + theme::ROW_HEIGHT * 10.)
            })
            .unwrap()
        };
        if name == "blank.txt" {
            this.update(cx, |p, _| p.explorer.collapsed = true);
        }
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_event(
                FileDropEvent::Entered {
                    position,
                    paths: ExternalPaths(vec![source.clone()].into()),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx);
            window.dispatch_event(FileDropEvent::Ended.to_platform_input(), cx);
        })
        .unwrap();
        let target = destination.join(if name == "a.txt" { "a copy.txt" } else { name });
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.explorer.selected.contains(&target)
                    && p.explorer.rows.iter().any(|row| row.entry.path == target)
            })
        });
        this.read_with(cx, |p, _| assert!(!p.explorer.collapsed));
        assert_eq!(fs::read_to_string(&target).unwrap(), "from Finder");
        assert_eq!(fs::read_to_string(&source).unwrap(), "from Finder");
        if name == "a.txt" {
            assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a.txt");
            assert!(
                !root.join("a copy 2.txt").exists(),
                "drop bubbled to parent and copied twice"
            );
        }
    }
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn batch_cut_follows_open_buffers_and_keeps_failed_paths_for_retry(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("cut");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(root.join("a.txt"), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.documents[0].dirty = true;
            p.explorer.set_selected(vec![
                root.join("a.txt"),
                root.join("b.txt"),
                root.join("missing.txt"),
            ]);
            p.copy_selection(true, cx);
            p.select_tree_path(root.join("dest"), window, cx);
            p.paste_files(window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message.contains("missing.txt"))
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(p.documents[0].path, root.join("dest/a.txt"));
        assert!(p.documents[0].dirty);
        assert_eq!(p.explorer.selected.len(), 2);
        assert_eq!(
            cx.global::<explorer_ops::FileClipboard>().paths,
            [root.join("missing.txt")]
        );
    });
    assert!(!root.join("a.txt").exists() && !root.join("b.txt").exists());
    assert!(root.join("dest/a.txt").exists() && root.join("dest/b.txt").exists());
    // A pending copy must not overwrite newer selection or a newer clipboard operation.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.transfer_files(
                vec![root.join("missing.txt")],
                root.clone(),
                true,
                window,
                cx,
            );
            p.explorer.set_selected(vec![root.join("c.txt")]);
            p.copy_selection(false, cx);
            p.transfer_files(
                vec![root.join("d.txt")],
                root.join("dest"),
                false,
                window,
                cx,
            );
            p.explorer
                .set_selected(vec![root.join("c.txt"), root.join("dest")]);
            p.message.clear();
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message.contains("missing.txt"))
            && root.join("dest/d.txt").exists()
    });
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.explorer.selected,
            BTreeSet::from([root.join("c.txt"), root.join("dest")])
        );
        assert_eq!(
            cx.global::<explorer_ops::FileClipboard>().paths,
            [root.join("c.txt")]
        );
        assert!(!cx.global::<explorer_ops::FileClipboard>().cut);
    });
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn failed_cut_restores_the_clipboard_after_its_window_closes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("closed-cut");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.explorer.set_selected(vec![root.join("missing.txt")]);
            p.copy_selection(true, cx);
            p.explorer.set_selected(vec![root.clone()]);
            p.paste_files(window, cx);
        });
        window.remove_window();
    })
    .unwrap();
    drop(this);
    settle(cx, None, |cx| {
        cx.read(|cx| cx.global::<explorer_ops::FileClipboard>().paths == [root.join("missing.txt")])
    });
    assert!(cx.read(|cx| cx.global::<explorer_ops::FileClipboard>().cut));
    fs::remove_dir_all(base).unwrap();
}
