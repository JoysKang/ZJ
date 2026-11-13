//! ⌘, opens the Settings tab; its controls change the settings file (in tests a temporary
//! file) and take effect at once. The JSON file still opens in a tab, and saving it puts it
//! into effect; a file that does not parse leaves the settings in use.

use super::test_support::{loaded, new_window, open_window, settle};
use super::*;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, test::TestWindowExt};

#[gpui_kit::test]
async fn the_settings_file_opens_and_applies_when_saved(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    assert!(path.exists());
    let created: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        created["hidden_files"],
        serde_json::json!(Settings::default().hidden_files.patterns())
    );
    this.read_with(cx, |p, _| assert!(!p.documents[0].dirty));

    let save = |cx: &mut TestAppContext, text: &str| {
        let (id, editor) = this.read_with(cx, |p, _| {
            (p.documents[0].id, p.documents[0].editor.clone())
        });
        let text = text.to_string();
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| state.set_value(text, window, cx));
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap()
    };
    // A parallel UI test may persist its own settings while this editor is open.
    let other_path = std::thread::spawn(|| {
        let path = Settings::path().unwrap();
        Settings {
            editor_font_size: 11.,
            ..Settings::default()
        }
        .save_to(&path)
        .unwrap();
        path
    })
    .join()
    .unwrap();
    let saved = save(cx, r#"{ "editor_font_size": 19 }"#);
    cx.run_until_parked();
    assert!(
        !cx.has_pending_prompt(),
        "another test's settings write caused an unexpected save-conflict prompt"
    );
    assert!(saved.await);
    cx.read(|cx| assert_eq!(cx.global::<Settings>().editor_font_size, 19.));
    this.read_with(cx, |p, _| assert_eq!(p.message, "设置已生效"));

    // A typo is saved as typed, but the settings in use stay.
    assert!(save(cx, r#"{ "editor_font_size": 12, "#).await);
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(cx.global::<Settings>().editor_font_size, 19.));
    this.read_with(cx, |p, _| {
        let notice = p.notice.as_ref().expect("a bad file opens the reason");
        assert_eq!(notice.title, "设置文件有误");
        assert!(
            notice.detail.contains("不是有效的 JSON"),
            "{}",
            notice.detail
        );
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    // Changes to this test's own file still require a save-conflict decision.
    let external = r#"{ "editor_font_size": 14 }"#;
    std::fs::write(&path, external).unwrap();
    let saved = save(cx, r#"{ "editor_font_size": 20 }"#);
    cx.run_until_parked();
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer("取消");
    assert!(!saved.await);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
    cx.read(|cx| assert_eq!(cx.global::<Settings>().editor_font_size, 19.));
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(other_path);
}

fn another_settings_window(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
) -> (WindowHandle<gpui_kit::base::Root>, Entity<Workbench>) {
    cx.update(|cx| {
        let owners = cx.global::<OpenDocuments>().0.clone();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        let (window, this) = new_window(cx, root, owners, bounds);
        (window.downcast::<gpui_kit::base::Root>().unwrap(), this)
    })
}

#[gpui_kit::test]
async fn old_settings_are_completed_as_an_undoable_unsaved_edit(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "{\n  \"未知设置\": {\"说明\": \"保留原值\"},\n  \"editor_font_size\": 17\n}\n";
    std::fs::write(&path, original).unwrap();
    let (window, this) = open_window(cx, None, Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1 && p.documents[0].dirty)
    });
    let editor = this.read_with(cx, |p, cx| {
        assert_eq!(p.message, "已补充隐藏规则，保存后生效");
        let editor = p.documents[0].editor.clone();
        let text = editor.read(cx).text().to_string();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["未知设置"]["说明"], "保留原值");
        assert_eq!(
            value["hidden_files"],
            serde_json::json!(Settings::default().hidden_files.patterns())
        );
        assert!(text.starts_with(original.trim_end().trim_end_matches('}')));
        editor
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    cx.read(|cx| {
        assert_eq!(
            cx.global::<Settings>().editor_font_size,
            crate::settings::EDITOR_FONT_DEFAULT
        )
    });
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.render_frame(cx);
        window.press("cmd-z", cx);
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |state, _| state.text().to_string()),
        original
    );

    // Reopening uses the draft already in memory, including its selection.
    let draft = r#"{"未知设置":"未保存草稿", "editor_font_size": 21}"#;
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_value(draft, window, cx);
            state.set_selected_range(2..8, cx);
        });
    })
    .unwrap();
    let (other_window, other) = another_settings_window(cx, None);
    cx.update_window(other_window.into(), |_, window, cx| {
        other.update(cx, |p, cx| {
            p.message.clear();
            p.open_settings_file(window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(other_window), |cx| {
        editor.read_with(cx, |state, _| {
            state.text().to_string().contains("hidden_files")
        })
    });
    let completed = editor.read_with(cx, |state, _| {
        assert_eq!(state.selected_range(), 2..8);
        state.text().to_string()
    });
    let value: serde_json::Value = serde_json::from_str(&completed).unwrap();
    assert_eq!(value["未知设置"], "未保存草稿");
    assert_eq!(value["editor_font_size"], 21);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(other.read_with(cx, |p, _| p.documents.is_empty()));
    assert!(this.read_with(cx, |p, _| p.documents[0].dirty));

    // An explicit empty array in a dirty buffer must survive either window reopening it.
    let empty = r#"{"hidden_files": [], "未知设置":"继续编辑"}"#;
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_value(empty, window, cx);
            state.set_selected_range(1..4, cx);
        });
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.message == "已定位到打开的标签；保留缓冲区内容")
    });
    cx.update_window(other_window.into(), |_, window, cx| {
        other.update(cx, |p, cx| {
            p.message.clear();
            p.open_settings_file(window, cx);
        });
    })
    .unwrap();
    settle(cx, Some(other_window), |cx| {
        other.read_with(cx, |p, _| {
            p.message == "文件已在另一窗口打开，已定位到原窗口"
        })
    });
    editor.read_with(cx, |state, _| {
        assert_eq!(state.text().to_string(), empty);
        assert_eq!(state.selected_range(), 1..4);
    });
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    // Programmatic completion also preserves reversed selections and extra cursors.
    let multi = "{\n  \"a\": \"one\",\n  \"b\": \"two\"\n}\n";
    let at = multi.find("one").unwrap() + 3;
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_value(multi, window, cx);
            state.set_selected_range(at..at, cx);
            state.focus(window, cx);
        });
        window.render_frame(cx);
        window.press("cmd-alt-down", cx);
        window.press("shift-left", cx);
    })
    .unwrap();
    let before = editor.read_with(cx, |state, _| {
        let range = state.selected_range();
        assert!(!range.is_empty());
        assert_eq!(state.cursor(), range.start);
        (range, state.cursor())
    });
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        editor.read_with(cx, |state, _| {
            state.text().to_string().contains("hidden_files")
        })
    });
    let completed = editor.read_with(cx, |state, _| {
        assert_eq!((state.selected_range(), state.cursor()), before);
        state.text().to_string()
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |state, _| {
        assert_eq!(state.text().to_string(), multi);
        assert_eq!((state.selected_range(), state.cursor()), before);
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.press("cmd-shift-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |state, _| {
        assert_eq!(state.text().to_string(), completed);
        assert_eq!((state.selected_range(), state.cursor()), before);
    });
    cx.update_window(window.into(), |_, window, cx| window.input("Z", cx))
        .unwrap();
    let edited: serde_json::Value =
        serde_json::from_str(&editor.read_with(cx, |state, _| state.text().to_string())).unwrap();
    assert_eq!(edited["a"], "onZ");
    assert_eq!(edited["b"], "twZ");
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::test]
async fn saving_hidden_rules_refreshes_both_windows_without_toggling_show_hidden(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let root = path.parent().unwrap().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    for name in ["keep.txt", "hide.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    Settings::default().save_to(&path).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let hidden = root.join("hide.txt");
    let kept = root.join("keep.txt");
    let (window, this) = open_window(cx, Some(root.clone()), Settings::default());
    let (other_window, other) = another_settings_window(cx, Some(root));
    for (handle, view) in [(window, &this), (other_window, &other)] {
        settle(cx, Some(handle), |cx| {
            loaded(cx, view)
                && view.read_with(cx, |p, _| {
                    p.explorer.rows.iter().any(|row| row.entry.path == hidden)
                        && p.explorer.rows.iter().any(|row| row.entry.path == kept)
                })
        });
    }
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    // Keep both result lists open while the settings are saved.
    for (handle, view) in [(window, &this), (other_window, &other)] {
        cx.update_window(handle.into(), |_, window, cx| {
            view.update(cx, |p, cx| {
                p.search
                    .query
                    .update(cx, |input, cx| input.set_value("txt", window, cx));
                p.schedule_search(Duration::ZERO, window, cx);
                p.open_quick_open_with("txt", window, cx);
            });
        })
        .unwrap();
        super::test_support::wait(
            cx,
            Some(Duration::from_millis(200)),
            Some(handle),
            |cx| {
                view.read_with(cx, |p, _| {
                    p.search.matches == 2
                        && p.quick_open.as_ref().is_some_and(|q| q.results.len() == 2)
                })
            },
            |cx| {
                view.read_with(cx, |p, _| {
                    format!(
                        "before save: search={}; quick={:?}; index={}",
                        p.search.matches,
                        p.quick_open.as_ref().map(|q| &q.results),
                        p.index.is_some()
                    )
                })
            },
        );
    }
    let (id, editor) = this.read_with(cx, |p, _| {
        (p.documents[0].id, p.documents[0].editor.clone())
    });
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(
                    r#"{"show_hidden": false, "hidden_files": ["*.txt", "!keep.txt"]}"#,
                    window,
                    cx,
                )
            });
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(saved.await);
    for (handle, view) in [(window, &this), (other_window, &other)] {
        settle(cx, Some(handle), |cx| {
            view.read_with(cx, |p, _| {
                !p.explorer.show_hidden
                    && !p.explorer.rows.iter().any(|row| row.entry.path == hidden)
                    && p.explorer.rows.iter().any(|row| row.entry.path == kept)
                    && p.search.matches == 1
                    && p.quick_open
                        .as_ref()
                        .is_some_and(|q| q.results == [kept.clone()])
            })
        });
    }
    cx.read(|cx| {
        assert!(!cx.global::<Settings>().show_hidden);
        assert_eq!(
            cx.global::<Settings>().hidden_files.patterns(),
            &["*.txt", "!keep.txt"]
        );
    });
    assert_eq!(
        Settings::load_from(&path).hidden_files.patterns(),
        &["*.txt", "!keep.txt"]
    );
    // A mistyped array is saved as typed, but cannot silently replace the active rules.
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(r#"{"hidden_files": ["*.log", 1]}"#, window, cx)
            });
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(saved.await);
    this.read_with(cx, |p, _| {
        let notice = p.notice.as_ref().expect("a bad file opens the reason");
        assert_eq!(notice.title, "设置文件有误");
        assert!(
            notice.detail.contains("hidden_files 必须"),
            "{}",
            notice.detail
        );
    });
    cx.read(|cx| {
        assert_eq!(
            cx.global::<Settings>().hidden_files.patterns(),
            &["*.txt", "!keep.txt"]
        )
    });
    // Removing all rules reveals files again in the already-open result lists.
    let saved = cx
        .update_window(window.into(), |_, window, cx| {
            editor.update(cx, |state, cx| {
                state.set_value(r#"{"hidden_files": []}"#, window, cx)
            });
            this.update(cx, |p, cx| p.save_document(id, false, window, cx))
        })
        .unwrap();
    assert!(saved.await);
    for (handle, view) in [(window, &this), (other_window, &other)] {
        settle(cx, Some(handle), |cx| {
            view.read_with(cx, |p, _| {
                !p.explorer.show_hidden
                    && p.explorer.rows.iter().any(|r| r.entry.path == hidden)
                    && p.search.matches == 2
                    && p.quick_open.as_ref().is_some_and(|q| q.results.len() == 2)
            })
        });
    }
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

async fn completion_waits_for_user_edits(cx: &mut TestAppContext, mode: crate::save::AutoSave) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = r#"{"future_key": "原值"}"#;
    std::fs::write(&path, original).unwrap();
    let settings = Settings {
        auto_save: mode,
        ..Settings::default()
    };
    let (handle, this) = open_window(cx, None, settings);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1 && p.documents[0].dirty)
    });
    cx.executor()
        .advance_clock(crate::save::AUTO_SAVE_DELAY * 3);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.auto_save_on_focus_change(None, window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let editor = this.read_with(cx, |p, _| {
        assert!(p.documents[0].dirty);
        assert!(!p.documents[0].saving);
        p.documents[0].editor.clone()
    });
    // A real user edit releases the completion-only suppression in either mode.
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |input, cx| {
            let end = input.text().len();
            input.set_selected_range(end..end, cx);
            input.replace_text_in_range(None, " ", window, cx);
        });
    })
    .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(crate::save::AUTO_SAVE_DELAY * 3);
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.auto_save_on_focus_change(None, window, cx));
    })
    .unwrap();
    settle(cx, Some(handle), |cx| {
        this.read_with(cx, |p, _| !p.documents[0].dirty && !p.documents[0].saving)
    });
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["future_key"], "原值");
    assert_eq!(
        value["hidden_files"],
        serde_json::json!(Settings::default().hidden_files.patterns())
    );
    let _ = std::fs::remove_file(path);
}

#[gpui_kit::test]
async fn completing_old_settings_does_not_start_delay_auto_save(cx: &mut TestAppContext) {
    completion_waits_for_user_edits(cx, crate::save::AutoSave::AfterDelay).await;
}

#[gpui_kit::test]
async fn completing_old_settings_does_not_start_focus_change_auto_save(cx: &mut TestAppContext) {
    completion_waits_for_user_edits(cx, crate::save::AutoSave::OnFocusChange).await;
}

/// Opens the Settings tab and completes one frame so its controls can be found by id.
fn open_settings_tab(
    cx: &mut TestAppContext,
    window: WindowHandle<gpui_kit::base::Root>,
    this: &Entity<Workbench>,
) {
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn the_settings_tab_changes_settings_without_editing_json(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    open_settings_tab(cx, window, &this);
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Settings);
        assert!(p.settings_pane.is_some());
        assert!(
            p.tab_specs().iter().any(|spec| spec.pane == Pane::Settings),
            "the Settings tab has a tab-bar entry"
        );
    });
    // A second ⌘, returns to the same tab, keeping its input.
    let hidden = this.read_with(cx, |p, _| p.settings_pane.as_ref().unwrap().hidden.clone());
    open_settings_tab(cx, window, &this);
    this.read_with(cx, |p, _| {
        assert_eq!(p.active, Pane::Settings);
        assert_eq!(p.settings_pane.as_ref().unwrap().hidden, hidden);
    });

    cx.update_window(window.into(), |_, window, cx| {
        window.click("settings-show-hidden", cx);
    })
    .unwrap();
    cx.read(|cx| assert!(cx.global::<Settings>().show_hidden));
    cx.run_until_parked();
    let written: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["show_hidden"], true);

    cx.update_window(window.into(), |_, window, cx| {
        window.click("settings-font-larger", cx);
        window.render_frame(cx);
        window.click("settings-auto-save-off", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.read(|cx| {
        assert_eq!(cx.global::<Settings>().editor_font_size, 14.);
        assert_eq!(
            cx.global::<Settings>().auto_save,
            crate::save::AutoSave::Off
        );
    });

    // ⌘W closes it like any other tab.
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.close_editor(window, cx));
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.settings_pane.is_none());
        assert_eq!(p.active, Pane::Welcome);
        assert!(!p.tab_specs().iter().any(|spec| spec.pane == Pane::Settings));
    });

    // The JSON file stays reachable from the tab.
    open_settings_tab(cx, window, &this);
    cx.update_window(window.into(), |_, window, cx| {
        window.click("settings-json", cx);
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let _ = std::fs::remove_file(&path);
}

#[gpui_kit::test]
async fn hidden_rules_apply_only_when_valid(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    open_settings_tab(cx, window, &this);
    let set_rules = |cx: &mut TestAppContext, text: &str| {
        let text = text.to_string();
        cx.update_window(window.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                let pane = p.settings_pane.as_ref().unwrap();
                pane.hidden
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            });
        })
        .unwrap();
    };
    let apply = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("settings-hidden-apply", cx);
        })
        .unwrap();
    };

    set_rules(cx, ".DS_Store\n*.log\n!重要.log\n");
    apply(cx);
    cx.read(|cx| {
        assert_eq!(
            cx.global::<Settings>().hidden_files.patterns(),
            [".DS_Store", "*.log", "!重要.log"]
        )
    });
    this.read_with(cx, |p, _| assert_eq!(p.message, "隐藏规则已生效"));

    set_rules(cx, ".DS_Store\nsrc/tmp\n");
    apply(cx);
    cx.read(|cx| {
        assert_eq!(
            cx.global::<Settings>().hidden_files.patterns(),
            [".DS_Store", "*.log", "!重要.log"],
            "an invalid pattern must keep the rules in use"
        )
    });
    this.read_with(cx, |p, _| {
        assert!(p.message.contains("无效"), "{}", p.message)
    });
    let _ = std::fs::remove_file(&path);
}

#[gpui_kit::test]
async fn applying_stale_hidden_rules_keeps_a_newer_edit(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    open_settings_tab(cx, window, &this);
    // The rules changed elsewhere: the JSON file in its tab, or another window.
    cx.update(|cx| {
        let rules = crate::hidden_files::HiddenFiles::new(vec![".DS_Store".into()]).unwrap();
        crate::settings::update(cx, |settings| settings.hidden_files = rules).detach();
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.settings_pane
                .as_ref()
                .unwrap()
                .hidden
                .read(cx)
                .value()
                .as_ref(),
            ".DS_Store",
            "the untouched textarea follows the newer rules"
        );
    });
    cx.update_window(window.into(), |_, window, cx| {
        window.click("settings-hidden-apply", cx);
    })
    .unwrap();
    cx.read(|cx| {
        assert_eq!(
            cx.global::<Settings>().hidden_files.patterns(),
            [".DS_Store"],
            "应用 must not write back rules the user never saw"
        )
    });
    let _ = std::fs::remove_file(&path);
}

#[gpui_kit::test]
async fn applied_text_with_a_trailing_newline_still_follows_a_later_edit(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    open_settings_tab(cx, window, &this);
    let typed = ".DS_Store\n*.log\n\n";
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            let pane = p.settings_pane.as_ref().unwrap();
            pane.hidden
                .update(cx, |input, cx| input.set_value(typed, window, cx));
        });
    })
    .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-hidden-apply", cx);
    })
    .unwrap();
    // The textarea keeps the typed text, so what it shows and the applied rules differ.
    cx.update(|cx| {
        let rules = crate::hidden_files::HiddenFiles::new(vec![".DS_Store".into()]).unwrap();
        crate::settings::update(cx, |settings| settings.hidden_files = rules).detach();
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    this.read_with(cx, |p, cx| {
        assert_eq!(
            p.settings_pane
                .as_ref()
                .unwrap()
                .hidden
                .read(cx)
                .value()
                .as_ref(),
            ".DS_Store",
            "text with no unapplied edits must follow a newer rule change"
        );
    });
    let _ = std::fs::remove_file(&path);
}

#[gpui_kit::test]
async fn the_settings_tab_keeps_the_file_the_session_should_restore(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_settings_file(window, cx));
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.documents.len() == 1)
    });
    let saved = this.read_with(cx, |p, _| p.saved_active());
    open_settings_tab(cx, window, &this);
    assert_eq!(
        this.read_with(cx, |p, _| p.saved_active()),
        saved,
        "the Settings tab has no file: restoring the one before it is more useful"
    );
    let _ = std::fs::remove_file(&path);
}

#[gpui_kit::test]
async fn an_open_diff_follows_the_saved_layout_from_elsewhere(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let path = Settings::path().unwrap();
    let _ = std::fs::remove_file(&path);
    let (window, this) = open_window(cx, None, Settings::default());
    let old = "a\nb\nc\n";
    let patch: Arc<str> = crate::save::text_patch(old, "a\nB\nc\n").into();
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.show_diff_tab(
                DiffTab {
                    label: "layout.txt".into(),
                    tooltip: "layout.txt".into(),
                    path: PathBuf::from("/tmp/layout.txt"),
                    source: DiffSource::Local(patch, "old → new"),
                    checkpoint: None,
                },
                window,
                cx,
            )
        });
    })
    .unwrap();
    settle(cx, Some(window), |cx| {
        this.read_with(cx, |p, _| p.diff.doc.is_some() && !p.diff.loading)
    });
    this.read_with(cx, |p, _| assert!(p.diff.inline, "the default layout"));
    cx.update(|cx| {
        crate::settings::update(cx, |settings| settings.diff_inline = false).detach();
    });
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert!(!p.diff.inline, "the open Diff followed the setting");
    });
    let _ = std::fs::remove_file(&path);
}
