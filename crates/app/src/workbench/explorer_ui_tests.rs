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
