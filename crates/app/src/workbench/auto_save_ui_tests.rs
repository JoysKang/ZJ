//! 保存与自动保存，在无头窗口里、临时文件夹上：自动保存不会把磁盘上已删除的文件重新写回来；
//! 手动保存写入磁盘并经过保存钩子（钩子延后执行，不在保存窗口更新期间重入）。

use super::test_support::{TICK, empty_store, loaded, open_window, settle, wait};
use super::*;
use crate::save::AutoSave;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn second_window(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workbench>) {
    cx.update(|cx| {
        let owners = cx.global::<OpenDocuments>().0.clone();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        super::test_support::new_window(cx, None, owners, bounds)
    })
}

fn edited_untitled(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    this: &Entity<Workbench>,
) -> DocumentId {
    let (id, editor) = cx
        .update_window(window, |_, window, cx| {
            this.update(cx, |p, cx| {
                let id = p.new_untitled(window, cx);
                (id, p.document(id).unwrap().editor.clone())
            })
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "keep", window, cx)
        });
    })
    .unwrap();
    assert!(this.read_with(cx, |p, _| p.document(id).unwrap().dirty));
    id
}

/// Opens `path` as the workspace's only tab and types at its start, leaving it dirty with
/// auto-save scheduled for the configured mode.
fn open_edited_buffer(
    cx: &mut TestAppContext,
    root: &std::path::Path,
    path: &std::path::Path,
    mode: AutoSave,
) -> (WindowHandle<Root>, Entity<Workbench>, Entity<EditorState>) {
    let settings = Settings {
        auto_save: mode,
        ..Settings::default()
    };
    let (window, this) = open_window(cx, Some(root.to_path_buf()), settings, empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "the folder never finished loading".into(),
    );
    let (open_path, folder) = (path.to_path_buf(), Some(root.to_path_buf()));
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "two", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(this.read_with(cx, |p, _| p.documents[0].dirty));
    (window, this, editor)
}

#[gpui_kit::test]
async fn a_save_result_for_the_previous_path_keeps_the_buffer_dirty(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-moved-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let id = edited_untitled(cx, window.into(), &this);
    let old = root.join("old.txt");
    let task = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(id, old.clone(), window, cx))
        })
        .unwrap();
    assert!(task.await);
    let new = root.join("new.txt");
    std::fs::write(&new, "keep").unwrap();
    let disk = this.read_with(cx, |p, _| p.document(id).unwrap().disk);
    let editor = this.read_with(cx, |p, _| p.document(id).unwrap().editor.clone());
    let task = cx
        .update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..4), "later", window, cx)
            });
            this.update(cx, |p, cx| {
                let task = p.save_document(id, false, window, cx);
                // The move completion can update the path before a pending save result
                // reaches the UI. Keep the old file so that write deterministically succeeds.
                p.document_mut(id).unwrap().path = new.clone();
                p.owners.borrow_mut().get_mut(&id).unwrap().path = new.clone();
                task
            })
        })
        .unwrap();
    assert!(
        !task.await,
        "a save of the previous path was treated as current"
    );
    this.read_with(cx, |p, _| {
        let doc = p.document(id).unwrap();
        assert!(doc.dirty);
        assert_eq!(doc.disk, disk);
        assert_eq!(doc.path, new);
        assert!(p.message.contains("路径已改变"));
    });
    assert_eq!(std::fs::read_to_string(old).unwrap(), "later");
    assert_eq!(std::fs::read_to_string(new).unwrap(), "keep");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn save_as_refuses_a_hard_link_to_another_windows_open_file(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-as-hardlink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let first = edited_untitled(cx, window.into(), &this);
    let path = root.join("original.txt");
    let task = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(first, path.clone(), window, cx))
        })
        .unwrap();
    assert!(task.await);
    let alias = root.join("alias.txt");
    std::fs::hard_link(&path, &alias).unwrap();
    let (other_window, other) = second_window(cx);
    let second = edited_untitled(cx, other_window, &other);
    let task = cx
        .update_window(other_window, |_, window, cx| {
            other.update(cx, |p, cx| p.write_as(second, alias, window, cx))
        })
        .unwrap();
    assert!(
        !task.await,
        "Save As overwrote a hard link owned by another window"
    );
    assert!(other.read_with(cx, |p, _| p.document(second).unwrap().untitled));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn quit_rechecks_earlier_discard_approvals_and_new_windows(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let first = edited_untitled(cx, window.into(), &this);
    let approval = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.confirm_close_approval(vec![first], window, cx)
            })
        })
        .unwrap();
    cx.simulate_prompt_answer("不保存");
    let mut approvals =
        std::collections::HashMap::from([(this.entity_id(), approval.await.unwrap())]);
    assert!(
        cx.update(|cx| documents::quit_is_approved(&approvals, cx)),
        "explicit 不保存 must authorize that version"
    );
    let (other_window, other) = second_window(cx);
    let second = edited_untitled(cx, other_window, &other);
    assert!(
        !cx.update(|cx| documents::quit_is_approved(&approvals, cx)),
        "a new window's edits were never confirmed"
    );
    let approval = cx
        .update_window(other_window, |_, window, cx| {
            other.update(cx, |p, cx| {
                p.confirm_close_approval(vec![second], window, cx)
            })
        })
        .unwrap();
    cx.simulate_prompt_answer("不保存");
    approvals.insert(other.entity_id(), approval.await.unwrap());
    assert!(cx.update(|cx| documents::quit_is_approved(&approvals, cx)));
    let editor = this.read_with(cx, |p, _| p.document(first).unwrap().editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "later", window, cx)
        });
    })
    .unwrap();
    assert!(
        !cx.update(|cx| documents::quit_is_approved(&approvals, cx)),
        "editing a previously confirmed window must cancel quit"
    );
}

#[gpui_kit::test]
async fn closing_tabs_rechecks_files_edited_while_the_prompt_is_open(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let (first, second) = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                (p.new_untitled(window, cx), p.new_untitled(window, cx))
            })
        })
        .unwrap();
    let edit = |cx: &mut TestAppContext, id| {
        let editor = this.read_with(cx, |p, _| p.document(id).unwrap().editor.clone());
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..0), "new", window, cx)
            });
        })
        .unwrap();
    };
    edit(cx, first);
    let close = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.confirm_close(vec![first, second], true, window, cx)
            })
        })
        .unwrap();
    edit(cx, second);
    cx.simulate_prompt_answer("不保存");
    assert!(
        !close.await,
        "the close discarded a file the prompt never asked about"
    );
    assert_eq!(this.read_with(cx, |p, _| p.documents.len()), 2);
}

#[gpui_kit::test]
async fn closing_a_window_rechecks_new_dirty_tabs(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let add_edited = |cx: &mut TestAppContext| {
        let editor = cx
            .update_window(window.into(), |_, window, cx| {
                this.update(cx, |p, cx| {
                    let id = p.new_untitled(window, cx);
                    p.document(id).unwrap().editor.clone()
                })
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..0), "keep", window, cx)
            });
        })
        .unwrap();
    };
    add_edited(cx);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_window_after_confirm(window, cx));
    })
    .unwrap();
    add_edited(cx);
    cx.simulate_prompt_answer("不保存");
    cx.run_until_parked();
    assert!(
        cx.update_window(window.into(), |_, _, _| ()).is_ok(),
        "the new dirty tab was closed without confirmation"
    );
    assert!(this.read_with(cx, |p, _| p.documents.iter().all(|doc| doc.dirty)));
}

#[gpui_kit::test]
async fn save_as_serializes_the_same_buffer_and_reserves_its_destination(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-as-race-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let (first, second) = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                (p.new_untitled(window, cx), p.new_untitled(window, cx))
            })
        })
        .unwrap();
    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    let (one, collision) = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let one = p.write_as(first, a.clone(), window, cx);
                assert!(
                    p.document(first).unwrap().saving,
                    "Save As must exclude other writes immediately"
                );
                let collision = p.write_as(second, a.clone(), window, cx);
                (one, collision)
            })
        })
        .unwrap();
    let (one, collision) = (one.await, collision.await);
    assert_ne!(
        one, collision,
        "exactly one buffer must own the destination"
    );
    let winner = if one { first } else { second };
    let (two, save) = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let two = p.write_as(winner, b.clone(), window, cx);
                let save = p.save_document(winner, false, window, cx);
                (two, save)
            })
        })
        .unwrap();
    assert!(two.await);
    assert!(save.await);
    assert_eq!(
        this.read_with(cx, |p, _| p.document(winner).unwrap().path.clone()),
        b
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn saving_before_close_keeps_edits_made_after_an_earlier_file_was_saved(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-close-save-edit-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    let mut ids = Vec::new();
    for name in ["a.txt", "b.txt"] {
        let (id, task) = cx
            .update_window(window.into(), |_, window, cx| {
                this.update(cx, |p, cx| {
                    let id = p.new_untitled(window, cx);
                    (id, p.write_as(id, root.join(name), window, cx))
                })
            })
            .unwrap();
        assert!(task.await);
        let editor = this.read_with(cx, |p, _| p.document(id).unwrap().editor.clone());
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.replace_text_in_range(Some(0..0), "saved", window, cx)
            });
        })
        .unwrap();
        ids.push(id);
    }
    assert!(this.read_with(cx, |p, _| {
        p.documents.iter().all(|doc| doc.dirty && doc.version > 0)
    }));
    let edited = this.read_with(cx, |p, _| p.document(ids[0]).unwrap().editor.clone());
    let saved_path = root.join("a.txt");
    cx.update(|cx| {
        documents::on_buffer_saved(cx, move |path, cx| {
            if path == saved_path {
                let edited = edited.clone();
                cx.defer(move |cx| {
                    cx.update_window(window.into(), |_, window, cx| {
                        edited.update(cx, |state, cx| {
                            state.replace_text_in_range(Some(0..0), "later ", window, cx)
                        });
                    })
                    .unwrap();
                });
            }
        })
    });
    let close = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.confirm_close(ids.clone(), true, window, cx))
        })
        .unwrap();
    cx.simulate_prompt_answer("全部保存");
    assert!(
        !close.await,
        "later edits were discarded by the successful earlier save"
    );
    this.read_with(cx, |p, cx| {
        let doc = p.document(ids[0]).unwrap();
        assert!(doc.dirty);
        assert_eq!(doc.editor.read(cx).text().to_string(), "later saved");
    });
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "saved"
    );
    let _ = std::fs::remove_dir_all(root);
}

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

    // Losing editor focus to the Agent composer must not recreate it.
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent.visible = true;
            p.agent
                .composer
                .update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!path.exists());
    assert!(!root.join("src").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn editor_blur_auto_saves_without_a_tab_or_window_change(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-auto-editor-blur-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this, editor) = open_edited_buffer(cx, &root, &path, AutoSave::OnFocusChange);
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent.visible = true;
            p.agent
                .composer
                .update(cx, |state, cx| state.focus(window, cx));
        });
        window.render_frame(cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| {
            !p.document(id).unwrap().dirty && !p.document(id).unwrap().saving
        })
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "twoone\n");
    this.read_with(cx, |p, _| assert_eq!(p.active, Pane::Document(id)));
    cx.update_window(window.into(), |_, window, cx| {
        let composer = this.read(cx).agent.composer.read(cx).focus_handle(cx);
        assert!(composer.is_focused(window));
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn editor_blur_does_not_save_in_other_modes(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-auto-blur-modes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this, editor) = open_edited_buffer(cx, &root, &path, AutoSave::Off);
    for mode in [AutoSave::Off, AutoSave::AfterDelay] {
        cx.update(|cx| cx.global_mut::<Settings>().auto_save = mode);
        cx.update_window(window.into(), |_, window, cx| {
            window.activate_window();
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.replace_text_in_range(Some(0..0), "two", window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.agent.visible = true;
                p.agent
                    .composer
                    .update(cx, |state, cx| state.focus(window, cx));
            });
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
        this.read_with(cx, |p, _| {
            assert!(p.documents[0].dirty);
            assert!(!p.documents[0].saving);
        });
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn after_delay_writes_an_edited_file_on_its_own(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-auto-delay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this, editor) = open_edited_buffer(cx, &root, &path, AutoSave::AfterDelay);
    // No explicit save and no focus change: the delay alone writes the buffer.
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| !p.documents[0].dirty && !p.documents[0].saving)
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "twoone\n");
    assert_eq!(
        editor.read_with(cx, |s, _| s.text().to_string()),
        "twoone\n"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn switching_auto_save_off_cancels_the_pending_delay(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-auto-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.rs");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this, _) = open_edited_buffer(cx, &root, &path, AutoSave::AfterDelay);
    // 文件 → 自动保存: 关闭 while this edit's timer is still waiting.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.set_auto_save(AutoSave::Off, window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(crate::save::AUTO_SAVE_DELAY * 3);
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\n");
    assert!(
        this.read_with(cx, |p, _| p.documents[0].dirty),
        "the buffer was saved after auto-save was switched off"
    );
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
    this.read_with(cx, |p, _| {
        assert_ne!(p.documents[0].id, p.documents[1].id);
        assert_eq!(p.active_document().unwrap().0, a);
        assert_eq!(p.owners.borrow().len(), 2);
    });
    let close = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let original = p.documents.iter().find(|doc| doc.path == a).unwrap().id;
                p.confirm_close(vec![original], true, window, cx)
            })
        })
        .unwrap();
    assert!(close.await);
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents.len(), 1);
        assert_eq!(p.documents[0].path, b);
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn a_file_deleted_and_restored_unchanged_leaves_its_tab_clean(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-deleted-back-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("x.rs");
    std::fs::write(&path, "fn x() {}\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "never loaded".into(),
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
    let check = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.check_disk(None, window, cx));
        })
        .unwrap();
    };
    let state = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| (p.documents[0].deleted, p.documents[0].dirty))
    };
    // Switching to a branch without the file...
    std::fs::remove_file(&path).unwrap();
    check(cx);
    settle(cx, Some(window), |cx| state(cx) == (true, true));
    // ...and back: the same bytes in a new file.
    std::fs::write(&path, "fn x() {}\n").unwrap();
    check(cx);
    settle(cx, Some(window), |cx| state(cx) == (false, false));
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn a_change_on_disk_during_a_save_is_looked_at_afterwards(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-recheck-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "one\n").unwrap();
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default(), empty_store());
    wait(
        cx,
        Some(TICK),
        None,
        |cx| loaded(cx, &this),
        |_| "never loaded".into(),
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
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    // The watcher reports a change while a save is writing.
    std::fs::write(&path, "two\n").unwrap();
    let changed = std::collections::BTreeSet::from([path.clone()]);
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.document_mut(id).unwrap().saving = true;
            p.check_disk(Some(&changed), window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.documents[0].recheck));
    // The save is done: the change is looked at then, and the unedited tab follows it.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.document_mut(id).unwrap().saving = false;
            p.recheck_after_save(id, window, cx);
        });
    })
    .unwrap();
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    settle(cx, Some(window), |cx| {
        editor.read_with(cx, |s, _| s.text().to_string()) == "two\n"
    });
    let _ = std::fs::remove_dir_all(&root);
}

#[gpui_kit::test]
async fn save_as_keeps_both_windows_in_quit_validation(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-save-as-owners-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    std::fs::write(&a, "original").unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(a.clone(), None, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let id = this.read_with(cx, |p, _| p.documents[0].id);
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.write_as(id, b.clone(), window, cx))
        })
        .unwrap();
    assert!(saved.await);
    let (other_window, other) = second_window(cx);
    cx.update_window(other_window, |_, window, cx| {
        other.update(cx, |p, cx| p.open_file(a.clone(), None, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        other.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let editor = this.read_with(cx, |p, _| p.document(id).unwrap().editor.clone());
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "unsaved", window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert!(this.read_with(cx, |p, _| p.document(id).unwrap().dirty));
    assert!(!cx.update(|cx| documents::quit_is_approved(&std::collections::HashMap::new(), cx)));
    this.read_with(cx, |p, _| assert_eq!(p.owners.borrow().len(), 2));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn manual_reload_keeps_edits_made_before_its_completion(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-reload-later-edit-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "disk").unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, None, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    // The reload completion cannot enter the workbench until this foreground update ends.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.reload_from_disk(id, window, cx));
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(0..0), "later ", window, cx)
        });
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.message.contains("重新加载期间"))
    });
    this.read_with(cx, |p, cx| {
        let doc = p.document(id).unwrap();
        assert_eq!(doc.editor.read(cx).text().to_string(), "later disk");
        assert!(doc.dirty);
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn a_hard_link_opened_elsewhere_keeps_one_editable_owner(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = std::env::temp_dir().join(format!("zj-hardlink-owner-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let (path, alias) = (root.join("a.txt"), root.join("alias.txt"));
    std::fs::write(&path, "original").unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    let (window, this) = open_window(cx, None, Settings::default(), empty_store());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(path, None, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let (other_window, other) = second_window(cx);
    cx.update_window(other_window, |_, window, cx| {
        other.update(cx, |p, cx| p.open_file(alias, None, window, cx));
    })
    .unwrap();
    settle(cx, None, |cx| {
        other.read_with(cx, |p, _| p.message.contains("另一窗口"))
    });
    other.read_with(cx, |p, _| {
        assert!(p.documents.is_empty());
        assert_eq!(p.owners.borrow().len(), 1);
    });
    let _ = std::fs::remove_dir_all(root);
}
