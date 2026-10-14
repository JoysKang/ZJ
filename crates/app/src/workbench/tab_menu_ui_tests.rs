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
    this: &Entity<Workbench>,
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

fn names(cx: &mut TestAppContext, this: &Entity<Workbench>) -> Vec<String> {
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
    this: &Entity<Workbench>,
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
async fn close_others_removes_restored_tabs_that_have_not_been_opened(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("restored-others");
    let (window, this) = open(cx, root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.restore_tabs(
                ["a.txt", "b.txt", "sub/c.txt"]
                    .map(|name| root.join(name))
                    .to_vec(),
                Some(root.join("a.txt")),
                window,
                cx,
            );
        });
    })
    .unwrap();
    settle(cx, None, |cx| names(cx, &this) == ["a.txt"]);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_pending_tab(root.join("b.txt"), window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| names(cx, &this) == ["a.txt", "b.txt"]);
    this.read_with(cx, |p, _| {
        assert_eq!(p.pending_tabs, [root.join("sub/c.txt")]);
    });

    this.update(cx, |p, _| p.documents[1].dirty = true);
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    this.read_with(cx, |p, _| {
        assert_eq!(p.pending_tabs, [root.join("sub/c.txt")]);
    });

    // Discard authorizes only the version shown in the prompt.
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    this.update(cx, |p, _| p.documents[1].version += 1);
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["a.txt", "b.txt"]);
    this.read_with(cx, |p, _| {
        assert_eq!(p.pending_tabs, [root.join("sub/c.txt")]);
    });

    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    assert_eq!(names(cx, &this), ["a.txt"]);
    this.read_with(cx, |p, _| {
        assert!(
            p.pending_tabs.is_empty(),
            "close others left restored tabs: {:?}",
            p.pending_tabs
        );
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn restored_tab_menus_close_and_keep_the_clicked_unloaded_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("restored-menu");
    let (window, this) = open(cx, root.clone());
    let restore = |cx: &mut TestAppContext, active: Option<PathBuf>| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.restore_tabs(
                    ["a.txt", "b.txt", "sub/c.txt"]
                        .map(|name| root.join(name))
                        .to_vec(),
                    active,
                    window,
                    cx,
                );
            });
        })
        .unwrap();
    };
    restore(cx, Some(root.join("a.txt")));
    settle(cx, None, |cx| names(cx, &this) == ["a.txt"]);
    // The item must be enabled even when every other tab is still unloaded.
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    this.read_with(cx, |p, _| assert!(p.pending_tabs.is_empty()));
    menu_click(cx, window, &this, 0, CLOSE_ALL);
    assert!(names(cx, &this).is_empty());

    restore(cx, None);
    let pending_key = usize::MAX / 2;
    menu_click(cx, window, &this, pending_key, CLOSE);
    this.read_with(cx, |p, _| {
        assert!(p.documents.is_empty());
        assert_eq!(p.pending_tabs, [root.join("b.txt"), root.join("sub/c.txt")]);
    });
    menu_click(cx, window, &this, pending_key + 1, CLOSE_OTHERS);
    settle(cx, None, |cx| names(cx, &this) == ["c.txt"]);
    this.read_with(cx, |p, _| {
        assert!(p.pending_tabs.is_empty());
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
    menu_click(cx, window, &this, 0, CLOSE_ALL);
    assert!(names(cx, &this).is_empty());

    restore(cx, None);
    menu_click(cx, window, &this, pending_key, CLOSE_ALL);
    this.read_with(cx, |p, _| {
        assert!(p.documents.is_empty());
        assert!(p.pending_tabs.is_empty());
        assert_eq!(p.active, Pane::Welcome);
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn batch_close_preserves_readonly_and_restored_tabs_on_cancel(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("batch-close");
    let path = root.join("long-line.txt");
    std::fs::write(&path, "x".repeat(256 * 1024 + 1)).unwrap();
    let (window, this) = open(cx, root.clone());
    open_files(cx, window, &this, &root);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path, Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| this.read_with(cx, |p, _| p.large.is_some()));
    this.update(cx, |p, cx| {
        p.documents[0].dirty = true;
        p.pending_tabs.push(root.join("unread.txt"));
        cx.notify();
    });
    menu_click(cx, window, &this, 1, CLOSE_ALL);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents.len(), 3);
        assert!(p.large.is_some());
        assert_eq!(p.pending_tabs, [root.join("unread.txt")]);
    });
    menu_click(cx, window, &this, 1, CLOSE_ALL);
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(p.documents.is_empty());
        assert!(p.large.is_none());
        assert!(p.pending_tabs.is_empty());
        assert_eq!(p.active, Pane::Welcome);
    });
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "a.txt\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn narrow_tabs_wrap_and_keep_their_close_behavior_in_both_themes(cx: &mut TestAppContext) {
    use super::test_support::{empty_store, install_globals, loaded, new_window};
    use gpui_kit::component::{Theme, ThemeMode};

    cx.executor().allow_parking();
    let root = fixture("wrapped");
    let (window, this) = cx.update(|cx| {
        let documents = install_globals(cx, crate::settings::Settings::default(), empty_store());
        let (handle, this) = new_window(
            cx,
            Some(root.clone()),
            documents,
            Bounds::new(point(px(0.), px(0.)), size(px(480.), px(640.))),
        );
        this.update(cx, |p, cx| {
            p.sidebar_visible = false;
            p.agent.visible = false;
            cx.notify();
        });
        (handle.downcast::<Root>().unwrap(), this)
    });
    settle(cx, None, |cx| loaded(cx, &this));
    open_files(cx, window, &this, &root);
    let long_name = format!("{}.txt", "long-file-name-".repeat(14));
    for name in (0..5)
        .map(|index| format!("additional-file-{index}.txt"))
        .chain(std::iter::once(long_name))
    {
        let path = root.join(name);
        std::fs::write(&path, "text\n").unwrap();
        let count = this.read_with(cx, |p, _| p.documents.len());
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.open_file(path, Some(root.clone()), window, cx)
            });
        })
        .unwrap();
        settle(cx, None, |cx| {
            this.read_with(cx, |p, _| p.documents.len() == count + 1)
        });
    }
    this.update(cx, |p, cx| {
        p.documents[1].dirty = true;
        cx.notify();
    });
    let count = this.read_with(cx, |p, _| p.documents.len());
    let active = this.read_with(cx, |p, _| p.active);
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update_window(window.into(), |_, window, cx| {
            Theme::change(mode, Some(window), cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let strip = window.find("tab-strip").bounds();
            let first = window.find(("tab", 0usize)).bounds();
            assert!(strip.size.height > px(36.), "tabs did not wrap");
            let mut wrapped = false;
            for index in 0..count {
                let tab = window.find(("tab", index)).bounds();
                let close = window.find(("tab-close", index)).bounds();
                let label = window.find(("tab-label", index)).bounds();
                assert_eq!(tab.size.height, px(36.));
                assert_eq!(close.size, size(px(20.), px(20.)));
                assert!(
                    tab.left() >= strip.left()
                        && tab.right() <= strip.right()
                        && tab.top() >= strip.top()
                        && tab.bottom() <= strip.bottom(),
                    "tab {index} overflows its strip: {tab:?}, {strip:?}"
                );
                assert!(close.left() >= tab.left() && close.right() <= tab.right());
                assert!(close.top() >= tab.top() && close.bottom() <= tab.bottom());
                assert!(label.right() <= close.left(), "label covers close slot");
                wrapped |= tab.top() > first.top();
            }
            assert!(wrapped);
            let active_bounds = window
                .find(("tab", count - 1))
                .bounds()
                .scale(window.scale_factor());
            let colors = theme::colors(cx);
            assert!(window.painted_quads().iter().any(|quad| {
                quad.bounds == active_bounds && quad.background == colors.input_bg.into()
            }));
            assert!(window.painted_quads().iter().any(|quad| {
                quad.background == colors.accent.into()
                    && quad.bounds.size == size(px(24.), px(2.)).scale(window.scale_factor())
                    && active_bounds.contains(&quad.bounds.center())
                    && quad.content_mask.bounds.contains(&quad.bounds.center())
            }));
        })
        .unwrap();
    }

    let click_close = |cx: &mut TestAppContext, index| {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("tab-close", index), cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    let dirty = this.read_with(cx, |p, _| p.documents[1].id);
    click_close(cx, 0usize);
    this.read_with(cx, |p, _| assert_eq!(p.active, active));
    assert_eq!(names(cx, &this).len(), count - 1);
    assert!(!names(cx, &this).contains(&"a.txt".to_string()));
    click_close(cx, 0usize);
    assert!(cx.has_pending_prompt());
    // Closing a dirty file selects it so the save prompt has visible context.
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Document(dirty)));
    cx.simulate_prompt_answer("取消");
    cx.run_until_parked();
    assert_eq!(names(cx, &this).len(), count - 1);
    assert_eq!(names(cx, &this)[0], "b.txt");
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Document(dirty)));
    click_close(cx, 0usize);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    assert_eq!(names(cx, &this).len(), count - 2);
    assert!(!names(cx, &this).contains(&"b.txt".to_string()));
    this.read_with(cx, |p, _| assert_eq!(p.active, active));
    assert_eq!(
        std::fs::read_to_string(root.join("b.txt")).unwrap(),
        "b.txt\n"
    );
    let keep_name = names(cx, &this)[0].clone();
    menu_click(cx, window, &this, 0, CLOSE_OTHERS);
    assert_eq!(names(cx, &this), [keep_name]);
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
    let _ = std::fs::remove_dir_all(&root);
}

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

#[gpui_kit::test]
async fn closing_the_last_open_tab_shows_the_next_restored_one(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("close-last");
    let (window, this) = open(cx, root.clone());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.restore_tabs(
                ["a.txt", "b.txt"].map(|name| root.join(name)).to_vec(),
                Some(root.join("a.txt")),
                window,
                cx,
            );
        });
    })
    .unwrap();
    settle(cx, None, |cx| names(cx, &this) == ["a.txt"]);
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_document(id, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| names(cx, &this) == ["b.txt"]);
    this.read_with(cx, |p, _| {
        assert!(p.pending_tabs.is_empty());
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn restored_tabs_keep_their_saved_order(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = fixture("order");
    let (window, this) = open(cx, root.clone());
    let saved = ["a.txt", "b.txt", "sub/c.txt"]
        .map(|name| root.join(name))
        .to_vec();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.restore_tabs(saved.clone(), Some(root.join("b.txt")), window, cx);
        });
    })
    .unwrap();
    settle(cx, None, |cx| names(cx, &this) == ["b.txt"]);
    this.read_with(cx, |p, _| {
        let mut shown: Vec<_> = p.documents.iter().map(|d| d.path.clone()).collect();
        shown.extend(p.pending_tabs.iter().cloned());
        shown.sort_by_key(|path| p.tab_rank(path));
        assert_eq!(shown, saved);
    });
    let _ = std::fs::remove_dir_all(&root);
}
