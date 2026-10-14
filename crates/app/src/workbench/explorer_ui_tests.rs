//! File selection and internal/Finder drops through GPUI, with filesystem operations in temporary folders.
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

pub(super) fn row_position(
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
async fn explorer_blank_space_opens_the_root_menu_without_reusing_the_selected_folder(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("blank-menu");
    for (root, rows) in [(base.join("workspace"), 6), (base.join("finder"), 1)] {
        let (handle, this) = open(cx, root.clone());
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| p.explorer.rows.len() == rows)
        });
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        for collapsed in [false, true] {
            let position = visual.update(|window, cx| {
                this.update(cx, |p, cx| {
                    p.explorer.collapsed = collapsed;
                    p.select_tree_path(root.join("dest"), window, cx);
                });
                window.render_frame(cx);
                let sidebar = window.find("workbench-sidebar").bounds();
                point(sidebar.center().x, sidebar.bottom() - theme::ROW_HEIGHT)
            });
            visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
            visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
            cx.run_until_parked();
            visual.update(|window, cx| {
                window.render_frame(cx);
                assert!(
                    window.try_find("popup-menu").is_some(),
                    "blank right-click did not open a menu, collapsed={collapsed}"
                );
                assert_eq!(
                    this.read(cx).explorer.selected,
                    BTreeSet::from([root.clone()])
                );
                window.within("popup-menu").click(0usize, cx);
            });
            cx.run_until_parked();
            this.read_with(cx, |p, _| {
                assert!(
                    matches!(p.explorer.edit.as_ref().map(|edit| &edit.kind),
                Some(explorer_ops::EditKind::NewFile(dir)) if dir == &root),
                    "blank menu created a file inside the previously selected folder"
                );
            });
            this.update(cx, |p, cx| p.cancel_tree_edit(cx));
            cx.run_until_parked();
        }
    }
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn the_explorer_blank_menu_does_not_take_space_from_a_scrolling_tree(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("blank-menu-scroll");
    let root = base.join("workspace");
    for index in 0..80 {
        fs::write(root.join(format!("z{index:03}.txt")), "test").unwrap();
    }
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 86)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        this.update(cx, |p, cx| {
            p.explorer.scroll.scroll_to_bottom();
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let position = row_position(cx, handle, &this, &root.join("z079.txt"));
    cx.update_window(handle.into(), |_, window, _| {
        let sidebar = window.find("workbench-sidebar").bounds();
        assert!(
            sidebar.contains(&position),
            "last row is outside the sidebar"
        );
        assert!(
            window.find("explorer-blank").bounds().size.height <= px(1.),
            "blank menu took space from the scrolling tree"
        );
    })
    .unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    cx.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.within("popup-menu").click(5usize, cx);
    });
    cx.run_until_parked();
    assert_eq!(selected(cx, &this), [root.join("z079.txt")]);
    cx.update(|cx| {
        assert_eq!(
            cx.global::<explorer_ops::FileClipboard>().paths,
            [root.join("z079.txt")]
        );
    });
    fs::remove_dir_all(base).unwrap();
}

pub(super) fn drag(
    cx: &mut TestAppContext,
    handle: WindowHandle<gpui_kit::base::Root>,
    from: Point<Pixels>,
    to: Point<Pixels>,
) {
    drag_between(cx, handle, handle, from, to);
}

fn drag_between(
    cx: &mut TestAppContext,
    source: WindowHandle<gpui_kit::base::Root>,
    target: WindowHandle<gpui_kit::base::Root>,
    from: Point<Pixels>,
    to: Point<Pixels>,
) {
    let mut visual = VisualTestContext::from_window(source.into(), cx);
    let modifiers = Modifiers::default();
    visual.simulate_mouse_down(from, MouseButton::Left, modifiers);
    visual.update(|window, cx| window.render_frame(cx));
    visual.simulate_mouse_move(
        from + point(theme::ICON_SIZE, Pixels::ZERO),
        MouseButton::Left,
        modifiers,
    );
    assert!(
        visual.update(|_, cx| cx.has_active_drag()),
        "dragging an Explorer entry did not start an internal drag"
    );
    visual.update(|window, cx| window.render_frame(cx));
    drop(visual);
    let mut visual = VisualTestContext::from_window(target.into(), cx);
    visual.simulate_mouse_move(to, MouseButton::Left, modifiers);
    visual.update(|window, cx| window.render_frame(cx));
    visual.simulate_mouse_up(to, MouseButton::Left, modifiers);
    visual.update(|window, cx| window.render_frame(cx));
    assert!(!visual.update(|_, cx| cx.has_active_drag()));
}

#[gpui_kit::test]
async fn returning_a_drag_to_its_original_directory_is_handled_immediately(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("drag-return-frame");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    let places = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let blank = window.find("explorer-blank").bounds().center();
            let header = window.find("explorer-new-file").bounds();
            let sidebar = window.find("workbench-sidebar").bounds();
            (
                blank,
                point(sidebar.left() + theme::TREE_BASE, header.center().y),
            )
        })
        .unwrap();
    for source in [root.join("a.txt"), root.join("dest")] {
        let from = row_position(cx, handle, &this, &source);
        let sibling = row_position(cx, handle, &this, &root.join("b.txt"));
        for to in [from, sibling, places.0, places.1] {
            let generation = this.read_with(cx, |p, _| p.explorer.generation);
            let mut visual = VisualTestContext::from_window(handle.into(), cx);
            visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
            visual.update(|window, cx| window.render_frame(cx));
            visual.simulate_mouse_move(
                from + point(theme::ICON_SIZE, Pixels::ZERO),
                MouseButton::Left,
                Modifiers::default(),
            );
            assert!(visual.update(|_, cx| cx.has_active_drag()));
            visual.update(|window, cx| window.render_frame(cx));
            visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
            visual.update(|window, cx| window.render_frame(cx));
            // Do not force-render after release: the drop must itself schedule cleanup.
            let release = visual.update(|window, cx| {
                window.dispatch_event(
                    MouseUpEvent {
                        position: to,
                        button: MouseButton::Left,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                )
            });
            assert!(!visual.update(|_, cx| cx.has_active_drag()));
            assert!(
                !release.propagate,
                "returning a drag cleared its state without handling the drop and requesting cleanup"
            );
            visual.update(|_, cx| {
                this.read_with(cx, |p, _| {
                    assert_eq!(
                        p.explorer.generation, generation,
                        "a no-op drop reloaded the tree"
                    );
                    assert_eq!(p.explorer.selected, BTreeSet::from([source.clone()]));
                    assert!(p.documents.is_empty() && p.message.is_empty());
                })
            });
        }
    }
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a.txt");
    assert!(!root.join("a copy.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn explorer_internal_drag_moves_a_file_and_follows_its_unsaved_buffer(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("internal-drag");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    let from = row_position(cx, handle, &this, &root.join("a.txt"));
    let to = row_position(cx, handle, &this, &root.join("dest"));
    VisualTestContext::from_window(handle.into(), cx).simulate_click(from, Modifiers::default());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(Some(0..5), "unsaved text", window, cx)
        });
    })
    .unwrap();
    drag(cx, handle, from, to);
    let target = root.join("dest/a.txt");
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.documents[0].path == target
                && p.explorer.rows.iter().any(|row| row.entry.path == target)
        })
    });
    assert!(!root.join("a.txt").exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "a.txt");
    this.read_with(cx, |p, cx| {
        assert!(p.documents[0].dirty);
        assert_eq!(
            p.documents[0].editor.read(cx).text().to_string(),
            "unsaved text"
        );
        assert_eq!(p.explorer.selected, BTreeSet::from([target.clone()]));
        assert_eq!(
            p.owners.borrow().get(&p.documents[0].id).unwrap().path,
            target
        );
    });
    // Dropping on the workspace header moves the file back to the root.
    let from = row_position(cx, handle, &this, &target);
    let to = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let header = window.find("explorer-new-file").bounds();
            let sidebar = window.find("workbench-sidebar").bounds();
            point(sidebar.left() + theme::TREE_BASE, header.center().y)
        })
        .unwrap();
    drag(cx, handle, from, to);
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents[0].path == root.join("a.txt"))
    });
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a.txt");
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn explorer_internal_drag_moves_a_selection_once_and_can_move_it_back_to_root(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("drag-selection");
    let root = base.join("workspace");
    fs::create_dir_all(root.join("pkg/sub")).unwrap();
    fs::write(root.join("pkg/sub/lib.rs"), "fn example() {}").unwrap();
    fs::write(root.join("dest/a.txt"), "keep destination").unwrap();
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 7)
    });
    this.update(cx, |p, cx| {
        p.explorer.set_selected(vec![root.join("c.txt")]);
        p.copy_selection(false, cx);
    });
    for (folder, child) in [("pkg", "pkg/sub"), ("pkg/sub", "pkg/sub/lib.rs")] {
        let position = row_position(cx, handle, &this, &root.join(folder));
        VisualTestContext::from_window(handle.into(), cx)
            .simulate_click(position, Modifiers::default());
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.explorer
                    .rows
                    .iter()
                    .any(|row| row.entry.path == root.join(child))
            })
        });
    }
    let command = Modifiers {
        platform: true,
        ..Default::default()
    };
    for path in ["pkg", "pkg/sub/lib.rs", "a.txt"] {
        let position = row_position(cx, handle, &this, &root.join(path));
        VisualTestContext::from_window(handle.into(), cx).simulate_click(position, command);
    }
    assert_eq!(selected(cx, &this).len(), 4);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(root.join("pkg/sub/lib.rs"), Some(root.clone()), window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let from = row_position(cx, handle, &this, &root.join("a.txt"));
    let to = row_position(cx, handle, &this, &root.join("dest"));
    drag(cx, handle, from, to);
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected
                == BTreeSet::from([root.join("dest/a copy.txt"), root.join("dest/pkg")])
                && p.explorer
                    .rows
                    .iter()
                    .any(|row| row.entry.path == root.join("dest/pkg"))
        })
    });
    assert!(!root.join("a.txt").exists() && !root.join("pkg").exists());
    assert_eq!(
        fs::read_to_string(root.join("dest/a.txt")).unwrap(),
        "keep destination"
    );
    assert_eq!(
        fs::read_to_string(root.join("dest/a copy.txt")).unwrap(),
        "a.txt"
    );
    assert_eq!(
        fs::read_to_string(root.join("dest/pkg/sub/lib.rs")).unwrap(),
        "fn example() {}"
    );
    assert!(!root.join("dest/lib.rs").exists() && !root.join("dest/sub").exists());
    assert!(!root.join("dest/a copy 2.txt").exists());
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents[0].path, root.join("dest/pkg/sub/lib.rs"))
    });
    // The blank target is always the workspace root, regardless of the selected folder.
    let from = row_position(cx, handle, &this, &root.join("dest/pkg"));
    let to = cx
        .update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("explorer-blank").bounds().center()
        })
        .unwrap();
    drag(cx, handle, from, to);
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected == BTreeSet::from([root.join("a copy.txt"), root.join("pkg")])
                && p.explorer
                    .rows
                    .iter()
                    .any(|row| row.entry.path == root.join("pkg"))
        })
    });
    assert!(!root.join("dest/pkg").exists() && !root.join("dest/a copy.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("pkg/sub/lib.rs")).unwrap(),
        "fn example() {}"
    );
    this.read_with(cx, |p, cx| {
        assert_eq!(p.documents[0].path, root.join("pkg/sub/lib.rs"));
        let clipboard = cx.global::<explorer_ops::FileClipboard>();
        assert_eq!(clipboard.paths, [root.join("c.txt")]);
        assert!(!clipboard.cut);
    });
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn explorer_internal_drag_rejects_self_and_descendants_without_falling_back_to_root(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("drag-invalid");
    let root = base.join("workspace");
    fs::create_dir_all(root.join("source/pkg/sub")).unwrap();
    fs::write(root.join("source/file.txt"), "keep source").unwrap();
    fs::write(root.join("source/pkg/sub/child.txt"), "keep child").unwrap();
    let (handle, this) = open(cx, root.clone());
    for (folder, child) in [("source", "source/pkg"), ("source/pkg", "source/pkg/sub")] {
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.explorer
                    .rows
                    .iter()
                    .any(|row| row.entry.path == root.join(folder))
            })
        });
        let position = row_position(cx, handle, &this, &root.join(folder));
        VisualTestContext::from_window(handle.into(), cx)
            .simulate_click(position, Modifiers::default());
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.explorer
                    .rows
                    .iter()
                    .any(|row| row.entry.path == root.join(child))
            })
        });
    }
    let command = Modifiers {
        platform: true,
        ..Default::default()
    };
    let file = row_position(cx, handle, &this, &root.join("source/file.txt"));
    VisualTestContext::from_window(handle.into(), cx).simulate_click(file, command);
    for target in ["source/pkg", "source/pkg/sub"] {
        let from = row_position(cx, handle, &this, &root.join("source/pkg"));
        let to = row_position(cx, handle, &this, &root.join(target));
        drag(cx, handle, from, to);
        cx.run_until_parked();
        assert_eq!(
            fs::read_to_string(root.join("source/file.txt")).unwrap(),
            "keep source"
        );
        assert_eq!(
            fs::read_to_string(root.join("source/pkg/sub/child.txt")).unwrap(),
            "keep child"
        );
        assert!(!root.join("pkg").exists() && !root.join("file.txt").exists());
    }
    // Dragging an unselected row moves only that row and dropping on a file uses its parent.
    let from = row_position(cx, handle, &this, &root.join("source/file.txt"));
    let to = row_position(cx, handle, &this, &root.join("b.txt"));
    this.update(cx, |p, cx| {
        p.explorer.set_selected(vec![root.join("source/pkg")]);
        cx.notify();
    });
    row_position(cx, handle, &this, &root.join("source/file.txt"));
    drag(cx, handle, from, to);
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected == BTreeSet::from([root.join("file.txt")])
        })
    });
    assert_eq!(
        fs::read_to_string(root.join("file.txt")).unwrap(),
        "keep source"
    );
    assert!(root.join("source/pkg/sub/child.txt").exists());
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b.txt");
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn explorer_internal_drag_follows_a_dirty_buffer_owned_by_another_window(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("drag-other-owner");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    let owners = this.read_with(cx, |p, _| p.owners.clone());
    let (other_handle, other_this) = cx.update(|cx| {
        let bounds = Bounds::new(Point::default(), size(px(1400.), px(900.)));
        super::test_support::new_window(cx, Some(root.clone()), owners, bounds)
    });
    let other_handle = other_handle.downcast::<gpui_kit::base::Root>().unwrap();
    cx.update_window(other_handle.into(), |_, window, cx| {
        other_this.update(cx, |p, cx| {
            p.open_file(root.join("a.txt"), Some(root.clone()), window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(other_handle), |cx| {
        other_this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (id, editor) = other_this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    cx.update_window(other_handle.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(Some(0..5), "other unsaved text", window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    let from = row_position(cx, handle, &this, &root.join("a.txt"));
    let to = row_position(cx, handle, &this, &root.join("dest"));
    drag(cx, handle, from, to);
    let target = root.join("dest/a.txt");
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected == BTreeSet::from([target.clone()])
        })
    });
    other_this.read_with(cx, |p, cx| {
        let document = p.document(id).unwrap();
        assert_eq!(document.path, target);
        assert!(document.dirty);
        assert_eq!(
            document.editor.read(cx).text().to_string(),
            "other unsaved text"
        );
        assert_eq!(p.owners.borrow().get(&id).unwrap().path, target);
    });
    let saved = cx
        .update_window(other_handle.into(), |_, window, cx| {
            other_this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(saved.await);
    assert_eq!(fs::read_to_string(target).unwrap(), "other unsaved text");
    assert!(
        !root.join("a.txt").exists(),
        "saving recreated the original path"
    );
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn closing_the_move_window_still_follows_other_windows_buffers(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drag-closed-owner");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    let owners = this.read_with(cx, |p, _| p.owners.clone());
    let (other_handle, other_this) = cx.update(|cx| {
        let bounds = Bounds::new(Point::default(), size(px(1400.), px(900.)));
        super::test_support::new_window(cx, Some(root.clone()), owners, bounds)
    });
    let other_handle = other_handle.downcast::<gpui_kit::base::Root>().unwrap();
    cx.update_window(other_handle.into(), |_, window, cx| {
        other_this.update(cx, |p, cx| {
            p.open_file(root.join("a.txt"), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(other_handle), |cx| {
        other_this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.transfer_files(
                vec![root.join("a.txt")],
                root.join("dest"),
                explorer_ops::FileTransfer::Move,
                window,
                cx,
            );
        });
        window.remove_window();
    })
    .unwrap();
    settle(cx, Some(other_handle), |cx| {
        other_this.read_with(cx, |p, _| p.documents[0].path == root.join("dest/a.txt"))
    });
    assert!(!root.join("a.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("dest/a.txt")).unwrap(),
        "a.txt"
    );
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn saves_wait_for_moves_and_busy_moves_preserve_cut_files_for_retry(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drag-save");
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
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(Some(0..5), "save while moving", window, cx)
        });
    })
    .unwrap();
    let save = cx
        .update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.transfer_files(
                    vec![root.join("a.txt")],
                    root.join("dest"),
                    explorer_ops::FileTransfer::Move,
                    window,
                    cx,
                );
                let save = p.save_document(id, false, window, cx);
                assert!(
                    !p.document(id).unwrap().saving,
                    "a save started writing to the old path during a move"
                );
                save
            })
        })
        .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.document(id)
                .is_some_and(|doc| doc.path == root.join("dest/a.txt") && !doc.dirty)
        })
    });
    assert!(save.await);
    assert!(!root.join("a.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("dest/a.txt")).unwrap(),
        "save while moving"
    );
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(Some(0..17), "save before moving", window, cx)
        });
    })
    .unwrap();
    let save = cx
        .update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let save = p.save_document(id, false, window, cx);
                p.explorer.set_selected(vec![root.join("dest/a.txt")]);
                p.copy_selection(true, cx);
                p.select_tree_path(root.clone(), window, cx);
                p.paste_files(window, cx);
                assert!(p.message.contains("正在保存"));
                save
            })
        })
        .unwrap();
    assert!(save.await);
    this.read_with(cx, |p, cx| {
        assert_eq!(p.document(id).unwrap().path, root.join("dest/a.txt"));
        let clipboard = cx.global::<explorer_ops::FileClipboard>();
        assert_eq!(clipboard.paths, [root.join("dest/a.txt")]);
        assert!(clipboard.cut);
    });
    assert!(!root.join("a.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn an_old_path_disk_check_does_not_mark_the_moved_buffer_deleted(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drag-stale-check");
    let root = base.join("workspace");
    let old = root.join("a.txt");
    let target = root.join("dest/a.txt");
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(old.clone(), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    fs::rename(&old, &target).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.check_disk(Some(&BTreeSet::from([old.clone()])), window, cx);
            let id = p.documents[0].id;
            // Simulate path synchronization before the queued old-path check returns.
            p.documents[0].path = target.clone();
            p.owners.borrow_mut().get_mut(&id).unwrap().path = target.clone();
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents[0].path, target);
        assert!(!p.documents[0].deleted);
        assert!(!p.documents[0].dirty);
    });
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn a_pending_move_prevents_relocating_its_destination(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drag-pending-target");
    let root = base.join("workspace");
    fs::create_dir(root.join("other")).unwrap();
    let (handle, this) = open(cx, root.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.transfer_files(
                vec![root.join("a.txt")],
                root.join("dest"),
                explorer_ops::FileTransfer::Move,
                window,
                cx,
            );
            p.transfer_files(
                vec![root.join("dest")],
                root.join("other"),
                explorer_ops::FileTransfer::Move,
                window,
                cx,
            );
            assert!(p.message.contains("正在保存或移动"));
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected.contains(&root.join("dest/a.txt"))
        })
    });
    assert_eq!(
        fs::read_to_string(root.join("dest/a.txt")).unwrap(),
        "a.txt"
    );
    assert!(!root.join("other/dest").exists());
    assert!(!root.join("a.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn explorer_internal_drag_does_not_move_files_through_another_window(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("drag-window");
    let root = base.join("workspace");
    let other = base.join("finder");
    let (handle, this) = open(cx, root.clone());
    let owners = this.read_with(cx, |p, _| p.owners.clone());
    let (other_handle, other_this) = cx.update(|cx| {
        let bounds = Bounds::new(Point::default(), size(px(1400.), px(900.)));
        super::test_support::new_window(cx, Some(other.clone()), owners, bounds)
    });
    let other_handle = other_handle.downcast::<gpui_kit::base::Root>().unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    settle(cx, Some(other_handle), |cx| {
        other_this.read_with(cx, |p, _| p.explorer.rows.len() == 1)
    });
    let from = row_position(cx, handle, &this, &root.join("a.txt"));
    let to = cx
        .update_window(other_handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find("explorer-blank").bounds().center()
        })
        .unwrap();
    drag_between(cx, handle, other_handle, from, to);
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a.txt");
    assert!(!other.join("a.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn failed_internal_moves_do_not_restore_or_replace_the_clipboard(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("drag-failure");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    cx.update(|cx| cx.set_global(explorer_ops::FileClipboard::default()));
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.transfer_files(
                vec![root.join("missing.txt")],
                root.join("dest"),
                explorer_ops::FileTransfer::Move,
                window,
                cx,
            );
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.message.contains("移动失败") && p.message.contains("missing.txt")
        })
    });
    cx.update(|cx| assert!(cx.global::<explorer_ops::FileClipboard>().paths.is_empty()));
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn finder_clipboard_folder_pastes_into_the_selected_directory(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("finder-clipboard");
    let root = base.join("workspace");
    let source = base.join("finder/外部 folder");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/file.txt"), "external folder").unwrap();
    let (handle, this) = open(cx, root.clone());
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.explorer.rows.len() == 6)
    });
    cx.update(|cx| cx.bind_keys([KeyBinding::new("secondary-v", PasteFiles, Some("Explorer"))]));
    this.update(cx, |p, cx| {
        p.explorer.set_selected(vec![root.join("b.txt")]);
        p.copy_selection(true, cx);
    });
    // Finder replaces a pending ZJ cut and exposes native files alongside optional text.
    cx.write_to_clipboard(ClipboardItem {
        entries: vec![
            ClipboardEntry::ExternalPaths(ExternalPaths(vec![source.clone()].into())),
            ClipboardEntry::String(ClipboardString::new("外部 folder".into())),
        ],
    });
    let position = row_position(cx, handle, &this, &root.join("dest"));
    VisualTestContext::from_window(handle.into(), cx)
        .simulate_click(position, Modifiers::default());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-v", cx);
    })
    .unwrap();
    let target = root.join("dest/外部 folder");
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.selected.contains(&target)
                && p.explorer.rows.iter().any(|row| row.entry.path == target)
        })
    });
    assert_eq!(
        fs::read_to_string(target.join("nested/file.txt")).unwrap(),
        "external folder"
    );
    assert_eq!(
        fs::read_to_string(source.join("nested/file.txt")).unwrap(),
        "external folder"
    );
    this.read_with(cx, |p, _| assert_eq!(p.message, "已复制 1 项"));
    // Clicking empty Explorer space must reclaim focus from the editor and target the root.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(root.join("a.txt"), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let blank = cx
        .update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.explorer.set_selected(vec![root.join("dest")]);
                p.documents[0]
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
                assert!(!p.explorer.focus.is_focused(window));
            });
            window.render_frame(cx);
            window.find("explorer-blank").bounds().center()
        })
        .unwrap();
    VisualTestContext::from_window(handle.into(), cx).simulate_click(blank, Modifiers::default());
    cx.update_window(handle.into(), |_, window, cx| {
        this.read_with(cx, |p, _| {
            assert_eq!(p.explorer.selected, BTreeSet::from([root.clone()]));
            assert!(p.explorer.focus.is_focused(window));
        });
        window.render_frame(cx);
        window.press("cmd-v", cx);
    })
    .unwrap();
    let target = root.join("外部 folder");
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| {
            p.explorer.rows.iter().any(|row| row.entry.path == target) && p.message == "已复制 1 项"
        })
    });
    assert_eq!(
        fs::read_to_string(target.join("nested/file.txt")).unwrap(),
        "external folder"
    );
    assert!(source.join("nested/file.txt").exists());
    assert!(
        root.join("b.txt").exists(),
        "Finder paste must not consume the old internal cut"
    );
    assert!(!root.join("dest/b.txt").exists());
    for item in [
        ClipboardItem::new_string(root.join("b.txt").display().to_string()),
        ClipboardItem {
            entries: Vec::new(),
        },
    ] {
        this.update(cx, |p, cx| {
            p.explorer.set_selected(vec![root.join("b.txt")]);
            p.copy_selection(true, cx);
        });
        cx.write_to_clipboard(item.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.select_tree_path(root.join("dest"), window, cx);
                p.message = "clipboard unchanged".into();
            });
            window.render_frame(cx);
            window.press("cmd-v", cx);
            assert_eq!(this.read(cx).message, "clipboard unchanged");
        })
        .unwrap();
        cx.run_until_parked();
        assert!(root.join("b.txt").exists());
        assert!(!root.join("dest/b.txt").exists());
        assert_eq!(cx.read_from_clipboard(), Some(item));
    }
    fs::remove_dir_all(base).unwrap();
}

#[gpui_kit::test]
async fn an_older_file_clipboard_item_cannot_reactivate_a_newer_cut(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let base = fixture("clipboard-identity");
    let root = base.join("workspace");
    let source = root.join("b.txt");
    let (handle, this) = open(cx, root.clone());
    cx.update(|cx| cx.bind_keys([KeyBinding::new("secondary-v", PasteFiles, Some("Explorer"))]));
    this.update(cx, |p, cx| {
        p.explorer.set_selected(vec![source.clone()]);
        p.copy_selection(false, cx);
    });
    let previous_copy = cx.read_from_clipboard().unwrap();
    this.update(cx, |p, cx| p.copy_selection(true, cx));
    // Clipboard history may restore an earlier copy of the same paths.
    cx.write_to_clipboard(previous_copy);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.select_tree_path(root.join("dest"), window, cx)
        });
        window.render_frame(cx);
        window.press("cmd-v", cx);
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        cx.read(|cx| !explorer_ops::file_is_moving(&source, cx))
    });
    assert!(
        source.exists(),
        "an old copy must not activate the pending cut"
    );
    assert!(!root.join("dest/b.txt").exists());
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
        ("dest", Some(root.join("dest")), root.join("dest")),
        ("a.txt", Some(root.join("a.txt")), root.clone()),
        ("blank.txt", None, root.clone()),
    ] {
        let source = base.join("finder").join(name);
        if name == "dest" {
            fs::create_dir_all(source.join("嵌套 folder")).unwrap();
            fs::write(source.join("嵌套 folder/file.txt"), "from Finder").unwrap();
            fs::create_dir_all(root.join("dest/dest")).unwrap();
            fs::write(root.join("dest/dest/keep.txt"), "existing").unwrap();
        } else {
            fs::write(&source, "from Finder").unwrap();
        }
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
        let target = destination.join(if name == "a.txt" {
            "a copy.txt"
        } else if name == "dest" {
            "dest copy"
        } else {
            name
        });
        settle(cx, Some(handle), |cx| {
            this.read_with(cx, |p, _| {
                p.explorer.selected.contains(&target)
                    && p.explorer.rows.iter().any(|row| row.entry.path == target)
            })
        });
        this.read_with(cx, |p, _| assert!(!p.explorer.collapsed));
        if name == "dest" {
            assert_eq!(
                fs::read_to_string(target.join("嵌套 folder/file.txt")).unwrap(),
                "from Finder"
            );
            assert!(source.join("嵌套 folder/file.txt").exists());
            assert_eq!(
                fs::read_to_string(root.join("dest/dest/keep.txt")).unwrap(),
                "existing"
            );
            assert!(
                !root.join("dest/dest copy 2").exists(),
                "folder drop copied twice"
            );
            assert!(
                !root.join("dest copy").exists(),
                "folder drop bubbled to root"
            );
        } else {
            assert_eq!(fs::read_to_string(&target).unwrap(), "from Finder");
            assert_eq!(fs::read_to_string(&source).unwrap(), "from Finder");
        }
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
    fs::write(root.join("missing.txt"), "retry").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.select_tree_path(root.join("dest"), window, cx);
            p.paste_files(window, cx);
            p.paste_files(window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message == "已移动 1 项")
    });
    assert_eq!(
        fs::read_to_string(root.join("dest/missing.txt")).unwrap(),
        "retry"
    );
    assert!(!root.join("dest/missing copy.txt").exists());
    cx.read(|cx| assert!(cx.global::<explorer_ops::FileClipboard>().paths.is_empty()));
    assert!(!root.join("a.txt").exists() && !root.join("b.txt").exists());
    assert!(root.join("dest/a.txt").exists() && root.join("dest/b.txt").exists());
    // A pending copy must not overwrite newer selection or a newer clipboard operation.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.transfer_files(
                vec![root.join("missing.txt")],
                root.clone(),
                explorer_ops::FileTransfer::Cut,
                window,
                cx,
            );
            p.explorer.set_selected(vec![root.join("c.txt")]);
            p.copy_selection(false, cx);
            p.transfer_files(
                vec![root.join("d.txt")],
                root.join("dest"),
                explorer_ops::FileTransfer::Copy,
                window,
                cx,
            );
            p.explorer
                .set_selected(vec![root.join("c.txt"), root.join("dest")]);
            p.message.clear();
        });
    })
    .unwrap();
    super::test_support::wait(
        cx,
        Some(super::test_support::TICK),
        Some(handle),
        |cx| {
            this.read_with(cx, |p, _| p.message.contains("missing.txt"))
                && root.join("dest/d.txt").exists()
        },
        |cx| {
            this.read_with(cx, |p, _| {
                format!(
                    "concurrent transfer error was lost: message={:?}, copied={}",
                    p.message,
                    root.join("dest/d.txt").exists()
                )
            })
        },
    );
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

#[gpui_kit::test]
async fn a_failed_cut_does_not_restore_files_after_the_system_clipboard_changes(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let base = fixture("replaced-cut");
    let root = base.join("workspace");
    let (handle, this) = open(cx, root.clone());
    let replacement = ClipboardItem::new_string("new clipboard".into());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.explorer.set_selected(vec![root.join("missing.txt")]);
            p.copy_selection(true, cx);
            p.select_tree_path(root.join("dest"), window, cx);
            p.paste_files(window, cx);
            assert!(p.message.starts_with("正在移动"));
            cx.write_to_clipboard(replacement.clone());
        });
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.message.contains("missing.txt"))
    });
    cx.read(|cx| assert!(cx.global::<explorer_ops::FileClipboard>().paths.is_empty()));
    assert_eq!(cx.read_from_clipboard(), Some(replacement));
    fs::write(root.join("missing.txt"), "must stay").unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.paste_files(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert!(root.join("missing.txt").exists());
    assert!(!root.join("dest/missing.txt").exists());
    fs::remove_dir_all(base).unwrap();
}

/// Renders the production sidebar in a native window at its minimum width.
struct NarrowSidebar(Entity<Workbench>);

impl Render for NarrowSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.0.update(cx, |p, cx| p.render_sidebar(cx))
    }
}

#[gpui_kit::test]
async fn narrow_sidebar_keeps_navigation_badge_actions_and_search_focus(cx: &mut TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    cx.executor().allow_parking();
    let base = fixture("narrow-navigation");
    let root = base.join("workspace");
    let output = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for index in 0..100 {
        fs::write(root.join(format!("change-{index}.txt")), "change").unwrap();
    }
    let (handle, this) = cx.update(|cx| {
        let documents = super::test_support::install_globals(
            cx,
            crate::settings::Settings::default(),
            super::test_support::empty_store(),
        );
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let mut this = None;
        let (handle, _) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(theme::SIDEBAR_MIN, px(640.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                let workbench =
                    cx.new(|cx| Workbench::new(Some(root), service, documents, 1, window, cx));
                this = Some(workbench.clone());
                cx.new(|_| NarrowSidebar(workbench))
            },
        )
        .unwrap();
        (handle, this.unwrap())
    });
    settle(cx, None, |cx| super::test_support::loaded(cx, &this));
    assert!(this.read_with(cx, |p, _| p.change_count()) > 99);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        visual.update(|window, cx| {
            Theme::change(mode, Some(window), cx);
            window.render_frame(cx);
            let sidebar = window.find("workbench-sidebar").bounds();
            assert_eq!(sidebar.size.width, theme::SIDEBAR_MIN);
            let mut previous = sidebar.left();
            for label in ["资源管理器", "搜索", "源代码管理"] {
                let tab = window.find(label);
                let bounds = tab.bounds();
                assert!(tab.visible() && bounds.size.width > px(0.));
                assert!(bounds.left() >= previous && bounds.right() <= sidebar.right());
                previous = bounds.right();
            }
            let source = window.find("源代码管理").bounds();
            let badge = window.find("activity-source-badge");
            let bounds = badge.bounds();
            assert_eq!(badge.label(), Some("99+"));
            assert!(badge.visible() && source.contains(&bounds.origin));
            assert!(bounds.right() <= source.right() && bounds.bottom() <= source.bottom());
        });
        let search = visual.update(|window, _| window.find("搜索").bounds().center());
        visual.simulate_click(search, Modifiers::default());
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("搜索").selected(), Some(true));
            let p = this.read(cx);
            assert!(p.sidebar == Sidebar::Search);
            assert!(p.search.query.read(cx).focus_handle(cx).is_focused(window));
        });
        let source = visual.update(|window, _| window.find("源代码管理").bounds().center());
        visual.simulate_click(source, Modifiers::default());
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("源代码管理").selected(), Some(true));
            let sidebar = window.find("workbench-sidebar").bounds();
            for id in ["refresh-git", "scm-more-button"] {
                let button = window.find(id);
                let bounds = button.bounds();
                assert!(button.visible() && bounds.size.width > px(0.));
                assert!(sidebar.contains(&bounds.origin) && bounds.right() <= sidebar.right());
            }
        });
    }
    let _ = fs::remove_dir_all(base);
}
