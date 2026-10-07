//! The agent panel in a headless window with the fake ACP agent: sending from the composer,
//! permissions and 始终允许 rules, reviewing and rejecting a change, reopening a stored session.

use super::super::test_support::{empty_store, new_window, open_window, wait};
use super::*;
// `super::*` brings in GPUI's `test` macro through `gpui_kit::*`; `#[gpui_kit::test]` expands
// to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, base::Root};
use workspace_editor_agent::{
    PermissionRequest, ToolCall, ToolCallPatch, ToolKind, ToolStatus, registry::UserAgentConfig,
};

fn fake_agent() -> UserAgentConfig {
    let exe = std::env::current_exe().unwrap();
    let program = exe
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("zj-fake-acp-agent");
    assert!(
        program.exists(),
        "{} is missing: run `cargo test --workspace` (it builds the agent crate's test binary)",
        program.display()
    );
    UserAgentConfig {
        id: "fake".into(),
        name: Some("Fake".into()),
        command: program.display().to_string(),
        args: Vec::new(),
        env: Default::default(),
    }
}

fn open(cx: &mut TestAppContext, root: Option<PathBuf>) -> (WindowHandle<Root>, Entity<Workbench>) {
    open_with(cx, root, empty_store())
}

fn open_with(
    cx: &mut TestAppContext,
    root: Option<PathBuf>,
    store: AgentStore,
) -> (WindowHandle<Root>, Entity<Workbench>) {
    let mut settings = crate::settings::Settings::default();
    settings.agent.panel_visible = true;
    settings.agent.custom = vec![fake_agent()];
    settings.agent.default_agent = "fake".into();
    open_window(cx, root, settings, store)
}

fn send(cx: &mut TestAppContext, handle: WindowHandle<Root>, this: &Entity<Workbench>, text: &str) {
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |this, cx| this.agent_focus_composer(window, cx));
        window.render_frame(cx);
        window.input(text, cx);
        window.render_frame(cx);
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
}

/// Waits (real time: the agent is a child process) until the current session is idle again.
/// The fake clock stays put.
fn settle(cx: &mut TestAppContext, this: &Entity<Workbench>) {
    let idle = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, _| {
            p.agent
                .current()
                .is_some_and(|s| !s.busy() && s.client.is_some())
        })
    };
    wait(cx, None, None, idle, |_| {
        "the agent never finished the turn".into()
    });
}

/// The user messages of the current session, and what is left in the composer.
fn sent(cx: &mut TestAppContext, this: &Entity<Workbench>) -> (Vec<String>, String) {
    this.read_with(cx, |p, cx| {
        let users = p
            .agent
            .current()
            .map(|s| {
                s.thread
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::User { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        (users, p.agent.composer.read(cx).value().to_string())
    })
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("zj-ui-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::canonicalize(root).unwrap()
}

#[gpui_kit::test]
async fn quitting_stops_agents_even_while_the_workbench_is_still_owned(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("quit-agent");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "stuck");
    until(cx, &this, "running agent process", |p| {
        p.agent
            .current()
            .and_then(|s| s.client.as_ref())
            .is_some_and(|c| c.pid().is_some() && c.is_busy())
    });
    let client = this.read_with(cx, |p, _| {
        p.agent.current().unwrap().client.as_ref().unwrap().clone()
    });
    let pid = client.pid().unwrap();
    // The native quit hook runs before clearing windows. Keep an extra owner like
    // a background operation, so quitting cannot rely solely on dropping the view.
    cx.update(|cx| cx.shutdown());
    assert!(
        client.pid().is_none(),
        "quitting left the agent running in the background"
    );
    // SAFETY: signal 0 only checks the known child pid; it does not send a signal.
    let alive = unsafe { libc::kill(pid as i32, 0) };
    assert_eq!(alive, -1, "agent survived application shutdown");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn pasted_images_preview_remove_and_send_as_image_only(cx: &mut TestAppContext) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    cx.executor().allow_parking();
    let root = temp_root("image-composer");
    let (handle, this) = open(cx, Some(root.clone()));
    let bytes = crate::agent_images::tests::png();
    cx.update_window(handle.into(), |_, window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("普通文字".into()));
        this.update(cx, |p, cx| p.agent_focus_composer(window, cx));
        window.render_frame(cx);
        window.press("cmd-v", cx);
        this.update(cx, |p, cx| {
            assert_eq!(p.agent.composer.read(cx).value().as_ref(), "普通文字");
            p.agent
                .composer
                .update(cx, |input, cx| input.set_value("", window, cx));
        });
    })
    .unwrap();
    let paste = |cx: &mut TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                bytes.clone(),
            )));
            this.update(cx, |p, cx| p.agent_focus_composer(window, cx));
            window.render_frame(cx);
            window.press("cmd-v", cx);
        })
        .unwrap();
        until(cx, &this, "image preview loaded", |p| {
            p.agent.images_loading == 0 && p.agent.attachments.len() == 1
        });
    };
    paste(cx);
    this.read_with(cx, |p, cx| {
        assert!(p.agent.composer.read(cx).value().is_empty());
        assert_eq!(p.agent.image_previews.len(), 1);
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent-chip-remove", 0usize), cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.agent.attachments.is_empty());
        assert!(p.agent.image_previews.is_empty());
    });
    paste(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("agent-send", cx);
    })
    .unwrap();
    settle(cx, &this);
    assert!(replies(cx, &this).contains(&format!("image/png:{}", STANDARD.encode(bytes))));
    this.read_with(cx, |p, _| {
        assert!(p.agent.attachments.is_empty());
        assert!(p.agent.image_previews.is_empty());
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn quota_hover_and_click_query_now_and_anchor_to_the_button(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt;
    cx.executor().allow_parking();
    let root = temp_root("quota-popup");
    let script = root.join("codex");
    let response = root.join("response.json");
    let queries = root.join("queries");
    std::fs::write(&script, format!("#!/bin/sh\nread -r init\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nread -r initialized\nread -r limits\nprintf 'query\\n' >> '{}'\ncat '{}'\n", queries.display(), response.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let limits = |used| serde_json::json!({"rateLimits":{"primary":{"usedPercent":used,"windowDurationMins":300}}});
    let write = |used| {
        std::fs::write(
            &response,
            serde_json::json!({"id":2,"result":limits(used)}).to_string(),
        )
        .unwrap()
    };
    write(20);
    let (handle, this) = open(cx, Some(root.clone()));
    cx.update_window(handle.into(), |_, window, cx| {
        cx.global_mut::<crate::settings::Settings>()
            .agent
            .env
            .insert(
                "codex".into(),
                std::collections::BTreeMap::from([(
                    "CODEX_PATH".into(),
                    script.display().to_string(),
                )]),
            );
        this.update(cx, |p, cx| {
            p.agent_new_session(None, window, cx);
            let key = p.agent.current.unwrap();
            p.agent.session_mut(key).unwrap().preset.id = "codex".into();
            p.agent.quota = Some(crate::quota::from_live(&limits(90), 1).unwrap());
        });
        window.render_frame(cx);
        window.hover("agent-quota", cx);
        window.render_frame(cx);
        let button = window.find("agent-quota").bounds();
        let card = window.find("agent-quota-card").bounds();
        assert!(card.bottom() <= button.top());
        assert!((card.right() - button.right()).abs() <= theme::ROW_INSET);
    })
    .unwrap();
    until(cx, &this, "hover refreshes old quota", |p| {
        !p.agent.quota_loading
            && p.agent
                .quota
                .as_ref()
                .is_some_and(|q| q.left_percent() == Some(80.0))
    });
    write(30);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("agent-quota", cx);
    })
    .unwrap();
    until(cx, &this, "click refreshes quota again", |p| {
        !p.agent.quota_loading
            && p.agent
                .quota
                .as_ref()
                .is_some_and(|q| q.left_percent() == Some(70.0))
    });
    this.read_with(cx, |p, _| {
        assert!(p.agent.quota_open);
        assert!(p.agent.quota_pinned);
        assert!(p.agent.quota.as_ref().unwrap().updated_ms > 1);
    });
    let query_count = || std::fs::read_to_string(&queries).unwrap().lines().count();
    assert_eq!(query_count(), 2);
    write(40);
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(599));
    cx.run_until_parked();
    assert_eq!(
        query_count(),
        2,
        "the periodic query ran before ten minutes"
    );
    cx.executor().advance_clock(Duration::from_secs(1));
    until(
        cx,
        &this,
        "ten minutes refreshes quota without hovering or clicking",
        |p| {
            !p.agent.quota_loading
                && p.agent
                    .quota
                    .as_ref()
                    .is_some_and(|q| q.left_percent() == Some(60.0))
        },
    );
    assert_eq!(query_count(), 3);
    std::fs::write(
        &response,
        serde_json::json!({"id":2,"error":{"message":"quota temporarily unavailable"}}).to_string(),
    )
    .unwrap();
    cx.run_until_parked();
    cx.executor().advance_clock(crate::quota::REFRESH_INTERVAL);
    until(cx, &this, "periodic errors are visible", |p| {
        !p.agent.quota_loading && p.agent.quota_error.is_some()
    });
    assert_eq!(query_count(), 4);
    assert_eq!(
        this.read_with(cx, |p, _| p.agent.quota.as_ref().unwrap().left_percent()),
        Some(60.0),
        "a failed refresh must preserve the previous quota"
    );
    write(50);
    cx.run_until_parked();
    cx.executor().advance_clock(crate::quota::REFRESH_INTERVAL);
    until(cx, &this, "the next interval retries after failure", |p| {
        !p.agent.quota_loading
            && p.agent.quota_error.is_none()
            && p.agent
                .quota
                .as_ref()
                .is_some_and(|q| q.left_percent() == Some(50.0))
    });
    assert_eq!(query_count(), 5);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.set_agent_panel(false, window, cx);
            assert!(p.agent.quota_next_refresh_ms.is_none());
        });
    })
    .unwrap();
    cx.executor().advance_clock(crate::quota::REFRESH_INTERVAL);
    cx.run_until_parked();
    assert_eq!(query_count(), 5, "a hidden panel kept querying quota");
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.set_agent_panel(true, window, cx);
            assert!(p.agent.quota_next_refresh_ms.is_some());
            p.agent_show(AgentView::History, window, cx);
            assert!(p.agent.quota_next_refresh_ms.is_none());
            p.agent_show(AgentView::Thread, window, cx);
            assert!(p.agent.quota_next_refresh_ms.is_some());
            p.agent_new_session(Some("fake".into()), window, cx);
            assert!(p.agent.quota_next_refresh_ms.is_none());
        });
    })
    .unwrap();
    cx.executor().advance_clock(crate::quota::REFRESH_INTERVAL);
    cx.run_until_parked();
    assert_eq!(
        query_count(),
        5,
        "switching agent kept querying Codex quota"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn the_agent_composer_uses_its_added_height_for_editing(cx: &mut TestAppContext) {
    let (handle, this) = open(cx, None);
    let mut minimum = None;
    for text in [
        "",
        "1\n2\n3\n4",
        "1\n2\n3\n4\n5\n6\n7\n8",
        &"长行自动换行 ".repeat(100),
        &"一行\n".repeat(20),
        "",
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.agent_focus_composer(window, cx);
                p.agent
                    .composer
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            });
            window.render_frame(cx);
            window.press("cmd-down", cx);
            window.render_frame(cx);
            this.read_with(cx, |p, cx| {
                let input = p.agent.composer.read(cx);
                let bounds = input.input_bounds();
                let padding = gpui_kit::component::Size::Medium.input_py() * 2.;
                assert!(bounds.size.height >= theme::AGENT_COMPOSER_MIN - padding);
                assert!(bounds.size.height <= input.line_height().unwrap() * 8.);
                if text.is_empty() {
                    assert_eq!(
                        *minimum.get_or_insert(bounds.size.height),
                        bounds.size.height
                    );
                } else if text.lines().count() >= 8 {
                    assert!(bounds.size.height > minimum.unwrap());
                }
                let send = window.find("agent-send").bounds();
                assert!(
                    bounds.bottom() <= send.top(),
                    "text={bounds:?} overlaps toolbar={send:?}"
                );
                let (cursor, _) = input.cursor_layout().unwrap();
                let scroll = input.scroll_offset().y;
                assert!(
                    cursor.top() + scroll >= bounds.top()
                        && cursor.bottom() + scroll <= bounds.bottom(),
                    "text={bounds:?}, caret={cursor:?}, scroll={scroll:?}"
                );
            });
        })
        .unwrap();
    }
    let bounds = this.read_with(cx, |p, cx| p.agent.composer.read(cx).text_bounds().unwrap());
    // Click in the lower half that used to be dead space, then type through native dispatch.
    let mut visual = gpui_kit::VisualTestContext::from_window(handle.into(), cx);
    visual.update(|window, cx| window.blur(cx));
    visual.simulate_click(
        bounds.origin + point(bounds.size.width / 2., bounds.size.height * 0.75),
        Modifiers::default(),
    );
    visual.update(|window, cx| window.input("新增区域也可输入", cx));
    this.read_with(&visual, |p, cx| {
        assert_eq!(
            p.agent.composer.read(cx).value().as_ref(),
            "新增区域也可输入"
        )
    });
}

#[gpui_kit::test]
async fn a_running_turn_has_one_process_and_collapses_when_the_final_reply_arrives(
    cx: &mut TestAppContext,
) {
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            let session = p.agent.session_mut(key).unwrap();
            session.thread.push_user("task".into(), vec![], 1);
            session.thread.apply(
                &AgentEvent::ThoughtChunk {
                    text: "thinking".into(),
                },
                true,
            );
            session.thread.apply(
                &AgentEvent::MessageChunk {
                    text: "progress".into(),
                },
                true,
            );
            session.thread.apply(
                &AgentEvent::ToolCall(ToolCall {
                    id: "failed".into(),
                    title: "command".into(),
                    kind: ToolKind::Execute,
                    status: ToolStatus::Failed,
                    locations: vec![],
                    content: vec![],
                }),
                true,
            );
            session.thread.push_notice("allowed", false);
            session.thread.push_user("adjust course".into(), vec![], 1);
            session.thread.apply(
                &AgentEvent::MessageChunk {
                    text: "final".into(),
                },
                true,
            );
            p.agent_sync_replies(key, 0);
            p.agent_sync_list(true);
            assert_eq!(
                p.agent.thread_rows.iter().filter(|row| row.process).count(),
                1
            );
            assert_eq!(
                p.agent.thread_rows.len(),
                3,
                "no intermediate reply or tool escapes the process"
            );
            p.agent.expanded_processes.insert((key, 0));
            p.agent_sync_list(false);
            assert!(
                p.agent
                    .thread_rows
                    .windows(2)
                    .all(|rows| rows[0].range.start <= rows[1].range.start),
                "expanded details keep their order around steering input"
            );
            assert_eq!(
                p.agent
                    .thread_rows
                    .iter()
                    .filter(|row| !row.process && row.range.start == 5)
                    .count(),
                1,
                "steering input is not duplicated in expanded details"
            );
            // Actual event path: end streaming, reveal the final answer and collapse
            // even a process the user had opened while it ran.
            p.agent_events(
                key,
                vec![AgentEvent::TurnEnded {
                    turn: 1,
                    outcome: workspace_editor_agent::TurnOutcome::EndTurn,
                }],
                window,
                cx,
            );
            assert!(!p.agent.expanded_processes.contains(&(key, 0)));
            assert_eq!(p.agent.thread_rows.len(), 4);
            let visible_replies: Vec<_> = p
                .agent
                .thread_rows
                .iter()
                .filter(|row| !row.process)
                .filter_map(
                    |row| match &p.agent.current().unwrap().thread.items[row.range.start] {
                        Item::Agent { text, .. } => Some(text.as_str()),
                        _ => None,
                    },
                )
                .collect();
            assert_eq!(visible_replies, ["final"]);
            cx.notify();
        });
        window.render_frame(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
async fn pending_actions_stay_visible_and_a_stale_end_keeps_the_process_open(
    cx: &mut TestAppContext,
) {
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            p.agent
                .session_mut(key)
                .unwrap()
                .thread
                .push_user("task".into(), vec![], 2);
            p.agent_events(
                key,
                vec![
                    AgentEvent::ThoughtChunk {
                        text: "thinking".into(),
                    },
                    AgentEvent::PermissionRequested(PermissionRequest {
                        id: 1,
                        tool_call: ToolCallPatch {
                            id: "approval".into(),
                            ..Default::default()
                        },
                        raw_input: None,
                        options: vec![],
                    }),
                    AgentEvent::AuthRequired { methods: vec![] },
                    AgentEvent::Progress {
                        message: "waiting".into(),
                    },
                ],
                window,
                cx,
            );
            assert_eq!(p.agent.thread_rows.len(), 4);
            p.agent.expanded_processes.insert((key, 0));
            p.agent_sync_list(false);
            for index in [2, 3] {
                assert_eq!(
                    p.agent
                        .thread_rows
                        .iter()
                        .filter(|row| !row.process && row.range.start == index)
                        .count(),
                    1,
                    "pending actions stay visible once, outside the process"
                );
            }
            p.agent_events(
                key,
                vec![AgentEvent::TurnEnded {
                    turn: 1,
                    outcome: workspace_editor_agent::TurnOutcome::EndTurn,
                }],
                window,
                cx,
            );
            assert_eq!(p.agent.current().unwrap().thread.turn, Some(2));
            assert!(p.agent.expanded_processes.contains(&(key, 0)));
        });
    })
    .unwrap();
}

#[gpui_kit::test]
async fn an_open_process_keeps_its_identity_and_reading_position_as_items_change(
    cx: &mut TestAppContext,
) {
    use workspace_editor_agent::{
        PermissionKind,
        thread::{MAX_ITEMS, PermissionState},
    };
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            let thread = &mut p.agent.session_mut(key).unwrap().thread;
            thread.push_user("task".into(), vec![], 1);
            thread.apply(
                &AgentEvent::PermissionRequested(PermissionRequest {
                    id: 1,
                    tool_call: ToolCallPatch {
                        id: "approval".into(),
                        ..Default::default()
                    },
                    raw_input: None,
                    options: vec![],
                }),
                true,
            );
            for _ in 2..MAX_ITEMS {
                thread.push_notice("details", false);
            }
            p.agent_sync_list(true);
            cx.notify();
        });
        window.render_frame(cx);
        window.click(("agent-process", 0usize), cx);
        window.render_frame(cx);
        let anchor = this
            .read(cx)
            .agent
            .thread_rows
            .iter()
            .position(|row| !row.process && row.range.start == 350)
            .unwrap();
        this.read(cx).agent.thread_list.scroll_to(ListOffset {
            item_ix: anchor,
            offset_in_item: px(12.),
        });
        window.render_frame(cx);
        // Completing the first approval moves it into the process. Subsequent
        // output then trims its first members at the 400-item memory limit.
        for step in 0..4 {
            this.update(cx, |p, cx| {
                let key = p.agent.current.unwrap();
                let thread = &mut p.agent.session_mut(key).unwrap().thread;
                if step == 0 {
                    assert!(thread.answer_permission(
                        1,
                        PermissionState::Answered(PermissionKind::AllowOnce, "allowed".into()),
                    ));
                } else {
                    thread.push_notice("newest", false);
                }
                p.agent_sync_list(false);
                cx.notify();
            });
            window.render_frame(cx);
            this.read_with(cx, |p, _| {
                let key = p.agent.current.unwrap();
                assert!(p.agent.expanded_processes.contains(&(key, 0)));
                assert_eq!(
                    p.agent.thread_rows.iter().filter(|row| row.process).count(),
                    1
                );
                assert!(p.agent.thread_rows.len() > MAX_ITEMS);
                let top = p.agent.thread_list.logical_scroll_top();
                let row = &p.agent.thread_rows[top.item_ix - usize::from(p.agent.older_row())];
                assert_eq!(
                    p.agent.current().unwrap().thread.dropped + row.range.start,
                    350
                );
                assert!((top.offset_in_item - px(12.)).abs() <= px(1.));
                assert!(!p.agent.thread_list.is_following_tail());
            });
        }
    })
    .unwrap();
}

#[gpui_kit::test]
async fn an_answered_permission_anchors_to_its_collapsed_process(cx: &mut TestAppContext) {
    use workspace_editor_agent::{PermissionKind, TurnOutcome, thread::PermissionState};
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            let thread = &mut p.agent.session_mut(key).unwrap().thread;
            for turn in 1..3 {
                thread.push_user("earlier task".into(), vec![], turn);
                thread.push_notice("detail", false);
                thread.apply(
                    &AgentEvent::MessageChunk {
                        text: "earlier reply\n\n".repeat(20),
                    },
                    true,
                );
                thread.apply(
                    &AgentEvent::TurnEnded {
                        turn,
                        outcome: TurnOutcome::EndTurn,
                    },
                    true,
                );
            }
            thread.push_user("current task".into(), vec![], 3);
            thread.push_notice("detail", false);
            for id in 1..=12 {
                thread.apply(
                    &AgentEvent::PermissionRequested(PermissionRequest {
                        id,
                        tool_call: ToolCallPatch {
                            id: id.to_string(),
                            ..Default::default()
                        },
                        raw_input: None,
                        options: vec![],
                    }),
                    true,
                );
            }
            p.agent_sync_replies(key, 0);
            p.agent_sync_list(true);
            cx.notify();
        });
        window.render_frame(cx);
        // Reading the first pending action with its execution process still folded.
        let anchor = this
            .read(cx)
            .agent
            .thread_rows
            .iter()
            .position(|row| !row.process && row.range.start == 8)
            .unwrap();
        this.read(cx).agent.thread_list.scroll_to(ListOffset {
            item_ix: anchor,
            offset_in_item: Pixels::ZERO,
        });
        window.render_frame(cx);
        assert!(!this.read(cx).agent.thread_list.is_following_tail());
        this.update(cx, |p, cx| {
            let key = p.agent.current.unwrap();
            assert!(p.agent.expanded_processes.is_empty());
            assert!(p.agent.session_mut(key).unwrap().thread.answer_permission(
                1,
                PermissionState::Answered(PermissionKind::AllowOnce, "allowed".into()),
            ));
            p.agent_sync_list(false);
            cx.notify();
        });
        window.render_frame(cx);
        this.read_with(cx, |p, _| {
            let top = p.agent.thread_list.logical_scroll_top();
            let row = &p.agent.thread_rows[top.item_ix];
            assert!(
                row.process,
                "a hidden anchor stays at its own process header"
            );
            assert_eq!(row.turn_start, 6);
            assert!(!p.agent.thread_list.is_following_tail());
        });
    })
    .unwrap();
}

#[gpui_kit::test]
async fn process_expansion_can_resume_following_latest_output(cx: &mut TestAppContext) {
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            let session = p.agent.session_mut(key).unwrap();
            session.thread.push_user("task".into(), vec![], 1);
            session.thread.items.extend([
                Item::Thought {
                    text: "detail\n".repeat(80),
                    streaming: false,
                },
                Item::Agent {
                    text: "latest".into(),
                    streaming: false,
                },
            ]);
            p.agent_sync_replies(key, 0);
            p.agent_sync_list(true);
            cx.notify();
        });
        window.render_frame(cx);
        window.click(("agent-process", 0usize), cx);
        window.render_frame(cx);
        // The user returns to the bottom after inspecting execution details.
        this.read(cx).agent.thread_list.scroll_to_end();
        window.render_frame(cx);
        assert!(
            this.read(cx).agent.thread_list.is_following_tail(),
            "returning to the bottom must resume following"
        );
        for text in ["newest\n\n".repeat(80), "more\n\n".repeat(40)] {
            this.update(cx, |p, cx| {
                let key = p.agent.current.unwrap();
                p.agent
                    .session_mut(key)
                    .unwrap()
                    .thread
                    .apply(&AgentEvent::MessageChunk { text }, true);
                p.agent_sync_replies(key, 0);
                p.agent_sync_list(false);
                cx.notify();
            });
            window.render_frame(cx);
            let list = &this.read(cx).agent.thread_list;
            assert!(list.is_following_tail());
            let bottom = list
                .bounds_for_item(list.item_count() - 1)
                .unwrap()
                .bottom();
            assert!(
                (bottom - list.viewport_bounds().bottom()).abs() <= px(1.),
                "latest output must remain at the bottom: item={bottom:?}, viewport={:?}",
                list.viewport_bounds()
            );
        }
        // Reading earlier details must not be pulled back to a growing last reply.
        let list = &this.read(cx).agent.thread_list;
        list.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(12.),
        });
        window.render_frame(cx);
        let before = this.read(cx).agent.thread_list.logical_scroll_top();
        assert!(!this.read(cx).agent.thread_list.is_following_tail());
        this.update(cx, |p, cx| {
            let key = p.agent.current.unwrap();
            p.agent.session_mut(key).unwrap().thread.apply(
                &AgentEvent::MessageChunk {
                    text: "more output\n\n".repeat(80),
                },
                true,
            );
            p.agent_sync_replies(key, 0);
            p.agent_sync_list(false);
            cx.notify();
        });
        window.render_frame(cx);
        let list = &this.read(cx).agent.thread_list;
        let after = list.logical_scroll_top();
        assert!(!list.is_following_tail());
        assert_eq!(before.item_ix, after.item_ix);
        assert!(
            (before.offset_in_item - after.offset_in_item).abs() <= px(1.),
            "reading position changed: before={before:?}, after={after:?}"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn expanded_process_details_stay_individual_virtual_rows(cx: &mut TestAppContext) {
    let (handle, this) = open(cx, None);
    let mut group = (0, 0);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session();
            let key = p.agent.current.unwrap();
            let session = p.agent.session_mut(key).unwrap();
            session.thread.items.push_back(Item::User {
                text: "task".into(),
                attachments: vec![],
            });
            for _ in 0..200 {
                session.thread.items.push_back(Item::Thought {
                    text: "details".into(),
                    streaming: false,
                });
            }
            session.thread.items.push_back(Item::Agent {
                text: "final".into(),
                streaming: false,
            });
            group = (key, 0);
            p.agent_sync_replies(key, 0);
            p.agent_sync_list(false);
            cx.notify();
        });
        window.render_frame(cx);
        window.click(("agent-process", group.1), cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.agent.expanded_processes.contains(&group));
        assert_eq!(p.agent.thread_rows.len(), 203);
        assert!(
            p.agent.thread_rows[2..]
                .iter()
                .all(|row| !row.process && row.range.len() == 1)
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent-process", group.1), cx);
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert_eq!(p.agent.thread_rows.len(), 3);
        assert_eq!(p.agent.thread_rows.last().unwrap().range, 201..202);
    });
}

#[gpui_kit::test]
async fn steering_works_from_enter_and_send_with_context_and_a_separate_stop_button(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = temp_root("steering");
    let path = root.join("a.txt");
    std::fs::write(&path, "context").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    this.update(cx, |p, _| {
        p.agent_ensure_session();
        p.agent
            .session_mut(p.agent.current.unwrap())
            .unwrap()
            .changes_collapsed = false;
    });
    send(cx, handle, &this, "steerable");
    until(cx, &this, "the original turn starts", |p| {
        p.agent
            .current()
            .is_some_and(|s| s.client.as_ref().is_some_and(|c| c.supports_steering()))
    });
    until(cx, &this, "the running process opens by default", |p| {
        let key = p.agent.current.unwrap();
        p.agent.expanded_processes.contains(&(key, 0))
            && p.agent.thread_rows.iter().any(|row| row.process)
    });
    this.read_with(cx, |p, _| {
        assert!(
            p.agent.current().unwrap().changes_collapsed,
            "new turns keep changed files folded independently of the running process"
        )
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent-process", 0usize), cx);
    })
    .unwrap();
    this.update(cx, |p, _| {
        p.agent.attachments.push(Attachment::File(path.clone()));
        p.agent
            .session_mut(p.agent.current.unwrap())
            .unwrap()
            .changes_collapsed = false;
    });
    send(cx, handle, &this, "先修复测试");
    wait(
        cx,
        None,
        Some(handle),
        |cx| replies(cx, &this).contains("steered:先修复测试"),
        |_| "the runtime instruction never arrived".into(),
    );
    this.read_with(cx, |p, _| {
        let session = p.agent.current().unwrap();
        assert_eq!(session.turns, 1);
        assert!(session.busy());
        assert!(!session.changes_collapsed, "steering preserves manually opened changed files");
        assert!(!p.agent.expanded_processes.contains(&(session.key, 0)),
            "steering preserves a manually folded process");
        assert!(session.thread.items.iter().any(|item| matches!(item, Item::User { text, attachments } if text == "先修复测试" && attachments == &["a.txt".to_string()])));
    });
    assert!(sent(cx, &this).1.is_empty());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_focus_composer(window, cx));
        window.input("再检查边界", cx);
        window.render_frame(cx);
        window.click("agent-send", cx);
        window.render_frame(cx);
    })
    .unwrap();
    wait(
        cx,
        None,
        Some(handle),
        |cx| replies(cx, &this).contains("steered:再检查边界"),
        |_| "clicking send did not steer".into(),
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent-process", 0usize), cx);
        window.click("agent-stop", cx);
    })
    .unwrap();
    settle(cx, &this);
    this.read_with(cx, |p, _| {
        assert!(
            p.agent.expanded_processes.is_empty(),
            "ending the turn folds its process"
        );
    });
    assert_eq!(sent(cx, &this).0, ["steerable", "先修复测试", "再检查边界"]);
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn steering_keeps_a_permission_request_pending(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("steering-awaiting");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "permission");
    until(cx, &this, "permission is pending", |p| {
        pending_permission(p).is_some()
    });
    let (key, id) = this.read_with(cx, |p, _| pending_permission(p).unwrap());
    send(cx, handle, &this, "先不要扩大修改范围");
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.agent.current().unwrap().thread.status,
            agent_thread::Status::Awaiting
        );
        assert_eq!(pending_permission(p), Some((key, id)));
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_answer(key, id, PermissionChoice::Reject, window, cx)
        });
    })
    .unwrap();
    settle(cx, &this);
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn steering_unavailable_keeps_the_typed_message_and_context(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("steering-unavailable");
    let mut settings = crate::settings::Settings::default();
    let mut preset = fake_agent();
    preset.env.insert("FAKE_NO_STEERING".into(), "1".into());
    settings.agent.custom = vec![preset];
    settings.agent.default_agent = "fake".into();
    settings.agent.panel_visible = true;
    let (handle, this) = open_window(cx, Some(root.clone()), settings, empty_store());
    send(cx, handle, &this, "steerable");
    wait(
        cx,
        None,
        Some(handle),
        |cx| replies(cx, &this).contains("started"),
        |_| "the original turn never starts".into(),
    );
    let attachment = Attachment::File(root.join("a.txt"));
    this.update(cx, |p, _| p.agent.attachments.push(attachment.clone()));
    send(cx, handle, &this, "保留这条补充说明");
    assert_eq!(
        sent(cx, &this),
        (vec!["steerable".into()], "保留这条补充说明".into())
    );
    this.read_with(cx, |p, _| {
        assert_eq!(p.agent.attachments, [attachment]);
        assert!(p.message.contains("不能接收"));
    });
    this.update(cx, |p, cx| p.agent_cancel(cx));
    settle(cx, &this);
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn mention_without_a_folder_says_so(cx: &mut TestAppContext) {
    let (handle, this) = open(cx, None);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |this, cx| this.agent_focus_composer(window, cx));
        window.input("@ma", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        let mention = p.agent.mention.as_ref().expect("the @ picker is open");
        assert_eq!(mention.query, "ma");
        assert!(mention.results.is_empty());
        assert_eq!(
            p.agent_mention_hint(),
            "没有打开文件夹，也没有打开的文件可以引用"
        );
    });
}

#[gpui_kit::test]
async fn slash_commands_complete_and_model_settings_switch(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("slash");
    let (handle, this) = open(cx, Some(root.clone()));
    let typed = |cx: &mut TestAppContext, text: &str| {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |this, cx| {
                this.agent_focus_composer(window, cx);
                this.agent
                    .composer
                    .update(cx, |c, cx| c.set_value("", window, cx));
            });
            window.input(text, cx);
            window.render_frame(cx);
        })
        .unwrap();
    };
    // Nothing to list yet: typing `/` starts the agent, and its commands follow without a
    // message being sent.
    typed(cx, " \n/re");
    until(cx, &this, "the agent listed its commands", |p| {
        p.agent
            .current()
            .is_some_and(|s| !s.thread.commands.is_empty())
    });
    let names = |cx: &mut TestAppContext| {
        this.read_with(cx, |p, cx| {
            p.agent_slash_matches(cx)
                .into_iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(names(cx), ["review"]);
    assert!(sent(cx, &this).0.is_empty());
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(sent(cx, &this), (vec![], "/review ".to_string()));
    this.read_with(cx, |p, _| assert_eq!(p.agent.slash, None));
    cx.update_window(handle.into(), |_, window, cx| {
        window.input("当前任务", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(sent(cx, &this), (vec![], "/review 当前任务".to_string()));

    // Model and effort are offered; the mode option is left to the mode menu.
    this.read_with(cx, |p, _| {
        let configs = &p.agent.current().unwrap().thread.configs;
        let ids: Vec<&str> = configs.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["model", "effort"]);
        assert_eq!(agent_model::config_label(configs), "SONNET · HIGH");
    });
    this.update(cx, |p, cx| {
        p.agent_set_config("model".into(), "opus".into(), cx)
    });
    until(cx, &this, "the model switched", |p| {
        p.agent.current().is_some_and(|s| {
            s.thread
                .configs
                .first()
                .is_some_and(|c| c.current == "opus")
        })
    });

    // A new session lists them before its own agent starts (an empty one would be reused).
    typed(cx, "");
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_new_session(None, window, cx));
    })
    .unwrap();
    typed(cx, "/re");
    assert_eq!(names(cx), ["review"]);
    this.read_with(cx, |p, _| {
        assert!(p.agent.current().is_some_and(|s| s.client.is_none()));
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn finished_replies_get_their_code_highlighted(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("highlight");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "markdown");
    settle(cx, &this);
    // Kit asks while painting; the answer arrives from the background and is kept.
    wait(
        cx,
        None,
        Some(handle),
        |cx| this.read_with(cx, |p, _| p.agent.code.highlighted() == 1),
        |_| "the rust block was never highlighted".into(),
    );
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn windows_share_one_agent_process(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let (ra, rb) = (temp_root("share-a"), temp_root("share-b"));
    let (ha, a) = open(cx, Some(ra.clone()));
    let (hb, b) = cx.update(|cx| {
        let documents = cx.global::<crate::workbench::OpenDocuments>().0.clone();
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.)));
        let (window, this) = new_window(cx, Some(rb.clone()), documents, bounds);
        (window.downcast::<Root>().unwrap(), this)
    });
    send(cx, ha, &a, "pid");
    settle(cx, &a);
    send(cx, hb, &b, "pid");
    settle(cx, &b);
    let first = replies(cx, &a);
    assert!(first.starts_with("pid:"), "{first}");
    assert_eq!(replies(cx, &b), first);
    let _ = std::fs::remove_dir_all(ra);
    let _ = std::fs::remove_dir_all(rb);
}

#[gpui_kit::test]
async fn enter_sends_every_turn(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("send");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "echo one");
    settle(cx, &this);
    send(cx, handle, &this, "echo two");
    settle(cx, &this);
    assert_eq!(
        sent(cx, &this),
        (
            vec!["echo one".to_string(), "echo two".to_string()],
            String::new()
        )
    );
    // Each finished turn shows how long it took under its last row, not under a user message.
    this.read_with(cx, |p, _| {
        let s = p.agent.current().unwrap();
        assert_eq!(s.turn_times.len(), 2);
        for index in s.turn_times.keys() {
            let item = &s.thread.items[index - s.thread.dropped];
            assert!(!matches!(item, Item::User { .. }));
        }
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn without_a_folder_the_session_runs_in_the_default_workspace(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("default");
    let workspace = data.join("workspace");
    let (handle, this) = open_with(
        cx,
        None,
        AgentStore {
            history: Some(Arc::new(History::new(data.join("history.sqlite")))),
            default_workspace: Some(workspace.clone()),
        },
    );
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    assert!(!cx.did_prompt_for_paths());
    assert!(workspace.is_dir(), "the default workspace is created");
    assert_eq!(
        sent(cx, &this),
        (vec!["echo hi".to_string()], String::new())
    );
    this.read_with(cx, |p, _| {
        assert_eq!(p.root, None);
        assert_eq!(p.agent.current().unwrap().root, Some(workspace.clone()));
    });

    // The history list of a window without a folder is the default workspace's.
    let mut rows = Vec::new();
    for _ in 0..100 {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |this, cx| this.agent_reload_history(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        rows = this.read_with(cx, |p, _| p.agent.history.rows.clone());
        if !rows.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].workspace_root, workspace);
    this.read_with(cx, |p, cx| {
        assert_eq!(p.agent.history.counts.0, 1);
        assert_eq!(p.agent_scope_label(false, cx), "默认工作区");
        assert_eq!(workspace_label(&workspace, cx), "默认工作区");
    });
    let _ = std::fs::remove_dir_all(data);
}

/// Waits (real time) until `done` holds for the window's workbench.
fn until(
    cx: &mut TestAppContext,
    this: &Entity<Workbench>,
    what: &str,
    done: impl Fn(&Workbench) -> bool,
) {
    let what = what.to_string();
    wait(
        cx,
        None,
        None,
        |cx| this.read_with(cx, |p, _| done(p)),
        move |_| format!("never: {what}"),
    );
}

/// The agent replies of the current session, joined.
fn replies(cx: &mut TestAppContext, this: &Entity<Workbench>) -> String {
    this.read_with(cx, |p, _| {
        p.agent
            .current()
            .map(|s| {
                s.thread
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Agent { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .unwrap_or_default()
    })
}

fn pending_permission(p: &Workbench) -> Option<(u64, workspace_editor_agent::PermissionId)> {
    let session = p.agent.current()?;
    let card = session.thread.pending_permissions().next()?;
    Some((session.key, card.request.id))
}

#[gpui_kit::test]
async fn command_approval_shows_the_exact_command_and_working_directory(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("permission-command");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "permission raw");
    until(cx, &this, "command approval", |p| {
        pending_permission(p).is_some()
    });
    let expected = "printf '%s\\n' 'a  b'\n# preserve  whitespace";
    let (_, id) = this.read_with(cx, |p, _| {
        let card = p
            .agent
            .current()
            .unwrap()
            .thread
            .pending_permissions()
            .next()
            .unwrap();
        assert_eq!(
            workspace_editor_agent::thread::permission_command(&card.request).as_deref(),
            Some(expected)
        );
        assert_eq!(
            workspace_editor_agent::thread::permission_cwd(&card.request),
            Some("/tmp/review folder")
        );
        pending_permission(p).unwrap()
    });
    wait(
        cx,
        None,
        Some(handle),
        |cx| {
            cx.update_window(handle.into(), |_, window, _| {
                window.try_find(("agent-permission-copy", id)).is_some()
            })
            .unwrap()
        },
        |_| "command approval did not become visible".into(),
    );
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent-permission-copy", id), cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()),
            Some(expected.into())
        );
        window.click(("agent-reject", id), cx);
    })
    .unwrap();
    settle(cx, &this);
    assert!(replies(cx, &this).contains("selected:reject"));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn a_permission_waits_for_the_user_and_allow_once_continues(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("permission-once");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "permission");
    until(cx, &this, "a permission request", |p| {
        pending_permission(p).is_some()
    });
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.agent.current().unwrap().row_status(),
            agent_model::RowStatus::Awaiting
        );
    });
    let (key, id) = this.read_with(cx, |p, _| pending_permission(p).unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_answer(key, id, PermissionChoice::Once, window, cx)
        });
    })
    .unwrap();
    settle(cx, &this);
    assert!(replies(cx, &this).contains("selected:allow"));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn always_allow_picks_the_agents_own_option_and_zj_keeps_no_rule(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("permission-always");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "permission");
    until(cx, &this, "a permission request", |p| {
        pending_permission(p).is_some()
    });
    let (key, id) = this.read_with(cx, |p, _| pending_permission(p).unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_answer(key, id, PermissionChoice::Always, window, cx)
        });
    })
    .unwrap();
    settle(cx, &this);
    // The agent got its own "always allow" option and remembers it itself.
    assert!(replies(cx, &this).contains("selected:allow_always"));
    // ZJ answers nothing on its own: the same request asks again.
    send(cx, handle, &this, "permission");
    until(cx, &this, "the second request waits for the user", |p| {
        pending_permission(p).is_some()
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn rejecting_a_hunk_in_the_review_restores_the_file_and_the_tab(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("review");
    let path = root.join("a.txt");
    std::fs::write(&path, "one").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    let folder = Some(root.clone());
    let open_path = path.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.open_file(open_path, folder, window, cx));
    })
    .unwrap();
    until(cx, &this, "the file opens", |p| p.documents.len() == 1);
    send(cx, handle, &this, &format!("write {} ONE", path.display()));
    settle(cx, &this);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "ONE");
    let editor = this.read_with(cx, |p, _| p.documents[0].editor.clone());
    let buffer = |cx: &mut TestAppContext| editor.read_with(cx, |s, _| s.text().to_string());
    wait(
        cx,
        None,
        None,
        |cx| buffer(cx) == "ONE",
        |_| "the tab never followed the agent's write".into(),
    );
    let key = this.read_with(cx, |p, _| {
        let session = p.agent.current().unwrap();
        assert!(session.thread.changed_files.contains_key(&path));
        session.key
    });
    let review_path = path.clone();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_review(key, review_path, window, cx)
        });
    })
    .unwrap();
    until(cx, &this, "the review loads", |p| p.diff.doc.is_some());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_review_hunk(0, false, window, cx));
    })
    .unwrap();
    wait(
        cx,
        None,
        None,
        |cx| std::fs::read_to_string(&path).unwrap() == "one" && buffer(cx) == "one",
        |_| "rejecting did not restore the file and the tab".into(),
    );
    until(cx, &this, "the file leaves the changed list", |p| {
        p.agent
            .current()
            .is_some_and(|s| !s.thread.changed_files.contains_key(&path))
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn accepting_a_file_opens_source_instead_of_an_empty_review(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("review-accept-file");
    let path = root.join("a.md");
    std::fs::write(&path, "one\ntwo\nthree").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    send(
        cx,
        handle,
        &this,
        &format!("write {} one\nTWO\nthree", path.display()),
    );
    settle(cx, &this);
    let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_review(key, path.clone(), window, cx)
        });
    })
    .unwrap();
    until(cx, &this, "the review loads", |p| p.diff.doc.is_some());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("agent-review-accept-file", cx);
    })
    .unwrap();
    until(cx, &this, "accepting opens the source file", |p| {
        p.active_document().is_some_and(|(file, _)| file == path)
    });
    this.read_with(cx, |p, cx| {
        assert!(p.diff.tab.is_none(), "the resolved review tab must close");
        let editor = p.active_editor().unwrap();
        assert_eq!(editor.read(cx).text().to_string(), "one\nTWO\nthree");
        assert_eq!(editor.read(cx).cursor(), 4, "keep the reviewed line");
        assert!(p.active_preview().is_none());
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn accepting_the_last_hunk_keeps_the_selected_line_in_the_existing_editor(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = temp_root("review-accept-hunk");
    let path = root.join("a.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\nfive").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    send(
        cx,
        handle,
        &this,
        &format!("write {} ONE\ntwo\nthree\nfour\nFIVE", path.display()),
    );
    settle(cx, &this);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.open_file(path.clone(), Some(root.clone()), window, cx)
        });
    })
    .unwrap();
    until(cx, &this, "the source opens", |p| p.documents.len() == 1);
    let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_review(key, path.clone(), window, cx);
            p.diff.inline = false;
        });
    })
    .unwrap();
    until(cx, &this, "two changes load", |p| {
        p.diff_change_starts().len() == 2
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_review_hunk(0, true, window, cx));
    })
    .unwrap();
    until(cx, &this, "one change remains", |p| {
        p.diff_change_starts().len() == 1
    });
    assert_eq!(this.read_with(cx, |p, _| p.active), Pane::Diff);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.diff.selection = Some(diff_ops::DiffSelection {
                list: diff_view::DiffList::Modified,
                anchor: 2,
                head: 2,
            });
            p.agent_review_hunk(0, true, window, cx);
        });
    })
    .unwrap();
    until(cx, &this, "the last acceptance returns to the file", |p| {
        p.active_document().is_some_and(|(file, _)| file == path)
    });
    this.read_with(cx, |p, cx| {
        assert!(p.diff.tab.is_none());
        assert_eq!(p.documents.len(), 1);
        assert_eq!(p.active_editor().unwrap().read(cx).cursor(), 8);
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn accepting_a_scrolled_review_keeps_the_visible_code(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("review-accept-scrolled");
    let path = root.join("a.txt");
    let before = (0..180).map(|i| format!("line {i}\n")).collect::<String>();
    let after = before.replace("line 0\n", "changed 0\n");
    std::fs::write(&path, &before).unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    send(
        cx,
        handle,
        &this,
        &format!("write {} {after}", path.display()),
    );
    settle(cx, &this);
    let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_review(key, path.clone(), window, cx);
            p.diff.inline = false;
        });
    })
    .unwrap();
    until(cx, &this, "the review loads", |p| p.diff.doc.is_some());
    let mut reviewed_y = Pixels::ZERO;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        this.update(cx, |p, cx| {
            p.diff.selection = Some(diff_ops::DiffSelection {
                list: diff_view::DiffList::Modified,
                anchor: 0,
                head: 0,
            });
            p.diff
                .scroll
                .scroll_to_item_strict(120, ScrollStrategy::Top);
            cx.notify();
        });
        window.render_frame(cx);
        window.render_frame(cx);
        reviewed_y = this.read_with(cx, |p, cx| {
            let height =
                theme::diff_metrics(gpui_kit::component::Theme::global(cx).mono_font_size).row;
            p.diff.scroll.0.borrow().base_handle.offset().y + height * 120.
        });
        window.click("agent-review-accept-file", cx);
    })
    .unwrap();
    until(cx, &this, "the source opens", |p| {
        p.active_document().is_some_and(|(file, _)| file == path)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.simulate_next_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    this.read_with(cx, |p, cx| {
        let editor = p.active_editor().unwrap();
        let state = editor.read(cx);
        assert_eq!(state.cursor_position().line, 120);
        let height = state.line_height().unwrap();
        let source_y = state.scroll_offset().y + height * 120.;
        assert!(
            (source_y - reviewed_y).abs() < height / 2.,
            "accepted line moved on screen: before={reviewed_y:?}, after={source_y:?}"
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn resolving_a_centered_review_preserves_wrapped_source_position(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    for (inline, accept, whole_file) in [
        (true, true, true),
        (false, true, false),
        (true, false, false),
        (false, false, true),
    ] {
        let root = temp_root("review-centered");
        let path = root.join("a.md");
        let before = (0..180)
            .map(|i| {
                if i == 30 {
                    format!("{}\n", "long text ".repeat(100))
                } else {
                    format!("line {i}\n")
                }
            })
            .collect::<String>()
            .trim_end()
            .to_string();
        let after = before.replace("line 80\n", "changed 80\n");
        std::fs::write(&path, &before).unwrap();
        let (handle, this) = open(cx, Some(root.clone()));
        send(
            cx,
            handle,
            &this,
            &format!("write {} {after}", path.display()),
        );
        settle(cx, &this);
        let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
        if !whole_file {
            cx.update_window(handle.into(), |_, window, cx| {
                this.update(cx, |p, cx| {
                    p.open_file(path.clone(), Some(root.clone()), window, cx)
                });
            })
            .unwrap();
            until(cx, &this, "existing source opens", |p| {
                p.documents.len() == 1
            });
        }
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.agent_open_review(key, path.clone(), window, cx);
                p.diff.inline = inline;
            });
        })
        .unwrap();
        until(cx, &this, "one centered change loads", |p| {
            p.diff.doc.is_some()
        });
        this.read_with(cx, |p, _| assert_eq!(p.diff_change_starts().len(), 1));
        let mut reviewed_y = Pixels::ZERO;
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            reviewed_y = this.read_with(cx, |p, cx| {
                let scroll = p.diff.scroll.0.borrow();
                let height =
                    theme::diff_metrics(gpui_kit::component::Theme::global(cx).mono_font_size).row;
                scroll.base_handle.offset().y + height * p.diff_change_starts()[0] as f32
            });
            this.update(cx, |p, cx| {
                if whole_file {
                    p.agent_review_file(accept, window, cx);
                } else {
                    p.agent_review_hunk(0, accept, window, cx);
                }
            });
        })
        .unwrap();
        until(cx, &this, "completed review returns to source", |p| {
            p.active_document().is_some_and(|(file, _)| file == path)
        });
        wait(
            cx,
            None,
            None,
            |cx| {
                this.read_with(cx, |p, cx| {
                    p.active_editor().is_some_and(|editor| {
                        editor.read(cx).text() == if accept { &after } else { &before }.as_str()
                    })
                })
            },
            |_| "returned source did not finish reloading".into(),
        );
        cx.update_window(handle.into(), |_, window, cx| {
            // Native frame callbacks run before layout of the returned source editor.
            window.simulate_next_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        this.read_with(cx, |p, cx| {
            assert!(p.documents[0].soft_wrap);
            let state = p.active_editor().unwrap().read(cx);
            assert_eq!(state.cursor_position().line, 80);
            assert_eq!(state.text().to_string(), if accept { after.clone() } else { before.clone() });
            let (caret, height) = state.cursor_layout().unwrap();
            let source_y = caret.origin.y - state.input_bounds().origin.y
                - (height - caret.size.height) / 2. + state.scroll_offset().y;
            assert!((source_y - reviewed_y).abs() < height / 2., "inline={inline}, accept={accept}, whole_file={whole_file}: before={reviewed_y:?}, after={source_y:?}");
        });
        let _ = std::fs::remove_dir_all(root);
    }
}

#[gpui_kit::test]
async fn resolving_one_of_multiple_changes_moves_to_the_next_change(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    for accept in [true, false] {
        let root = temp_root("review-next-change");
        let path = root.join("a.txt");
        let before = (0..200)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .trim_end()
            .to_string();
        let after = before
            .replace("line 20\n", "changed 20\n")
            .replace("line 120\n", "changed 120\n");
        std::fs::write(&path, &before).unwrap();
        let (handle, this) = open(cx, Some(root.clone()));
        send(
            cx,
            handle,
            &this,
            &format!("write {} {after}", path.display()),
        );
        settle(cx, &this);
        let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| {
                p.agent_open_review(key, path.clone(), window, cx);
                p.diff.inline = false;
            });
        })
        .unwrap();
        until(cx, &this, "two changes load", |p| p.diff.doc.is_some());
        this.read_with(cx, |p, _| assert_eq!(p.diff_change_starts().len(), 2));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            this.update(cx, |p, cx| p.agent_review_hunk(0, accept, window, cx));
        })
        .unwrap();
        until(cx, &this, "one change remains", |p| {
            p.diff_change_starts().len() == 1
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        this.read_with(cx, |p, cx| {
            assert_eq!(p.active, Pane::Diff);
            let scroll = p.diff.scroll.0.borrow();
            let height =
                theme::diff_metrics(gpui_kit::component::Theme::global(cx).mono_font_size).row;
            let y = scroll.base_handle.offset().y + height * p.diff_change_starts()[0] as f32;
            let middle = (scroll.base_handle.bounds().size.height - height) / 2.;
            assert!(
                (y - middle).abs() < height,
                "accept={accept}: next={y:?}, middle={middle:?}"
            );
        });
        let _ = std::fs::remove_dir_all(root);
    }
}

#[gpui_kit::test]
async fn accepting_all_does_not_cancel_a_newer_file_open(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("review-accept-background");
    let path = root.join("a.txt");
    let other = root.join("b.txt");
    std::fs::write(&path, "one").unwrap();
    std::fs::write(&other, "other").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, &format!("write {} ONE", path.display()));
    settle(cx, &this);
    let key = this.read_with(cx, |p, _| p.agent.current().unwrap().key);
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_review(key, path.clone(), window, cx)
        });
    })
    .unwrap();
    until(cx, &this, "the review loads", |p| p.diff.doc.is_some());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_resolve_all(true, window, cx);
            p.open_file(other.clone(), Some(root.clone()), window, cx);
        });
    })
    .unwrap();
    until(
        cx,
        &this,
        "the newer file opens and the old review closes",
        |p| p.diff.tab.is_none() && p.active_document().is_some_and(|(file, _)| file == other),
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "ONE");
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn a_stored_session_reopens_with_its_messages(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("restore");
    let root = data.join("work");
    std::fs::create_dir_all(&root).unwrap();
    let history = Arc::new(History::new(data.join("history.sqlite")));
    let store = AgentStore {
        history: Some(history.clone()),
        default_workspace: None,
    };
    let (handle, this) = open_with(cx, Some(root.clone()), store);
    send(cx, handle, &this, "echo 从历史恢复");
    settle(cx, &this);
    until(cx, &this, "the session is stored", |p| {
        p.agent.current().is_some_and(|s| s.db.is_some())
    });
    let id = this.read_with(cx, |p, _| p.agent.current().unwrap().db.unwrap());
    history.flush().unwrap();
    // Close the live session, then open it again from the history list.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent.sessions.clear();
            p.agent.current = None;
            p.agent_open_stored(id, window, cx);
        });
    })
    .unwrap();
    until(cx, &this, "the stored session opens", |p| {
        p.agent.current().is_some_and(|s| s.db == Some(id))
    });
    wait(
        cx,
        None,
        None,
        |cx| sent(cx, &this).0 == ["echo 从历史恢复"] && replies(cx, &this).contains("从历史恢复"),
        |cx| {
            format!(
                "restored messages: {:?} / {}",
                sent(cx, &this).0,
                replies(cx, &this)
            )
        },
    );
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn a_long_stored_session_keeps_turns_consistent_across_history_pages(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let data = temp_root("history-pages");
    let history = Arc::new(History::new(data.join("history.sqlite")));
    let id = history
        .create_session(workspace_editor_agent_history::NewSession {
            workspace_root: data.clone(),
            workspace_name: None,
            agent_id: "fake".into(),
            acp_session_id: None,
            title: None,
            first_prompt: None,
            repo: None,
            branch: None,
            created_at: None,
        })
        .unwrap();
    for n in 0..3 * HISTORY_PAGE {
        let role = [HistoryRole::User, HistoryRole::Thought, HistoryRole::Agent][n % 3];
        history.append_message(id, role, n.to_string());
    }
    history.flush().unwrap();
    let (handle, this) = open_with(
        cx,
        Some(data.clone()),
        AgentStore {
            history: Some(history),
            default_workspace: None,
        },
    );
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_open_stored(id, window, cx));
    })
    .unwrap();
    until(cx, &this, "the last history page opens", |p| {
        p.agent.current().is_some_and(|s| s.db == Some(id))
    });
    this.read_with(cx, |p, _| {
        assert_eq!(p.agent.current().unwrap().thread.dropped, 2 * HISTORY_PAGE);
        assert!(p.agent.older_row());
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_load_older(window, cx));
    })
    .unwrap();
    until(
        cx,
        &this,
        "the previous history page joins the thread",
        |p| {
            p.agent
                .current()
                .is_some_and(|s| s.thread.items.len() == 2 * HISTORY_PAGE)
        },
    );
    this.read_with(cx, |p, _| {
        let thread = &p.agent.current().unwrap().thread;
        assert_eq!(thread.dropped, HISTORY_PAGE);
        assert!(p.agent.older_row());
        assert_eq!(
            p.agent.thread_rows.iter().filter(|row| row.process).count(),
            2 * HISTORY_PAGE / 3,
            "a turn cut by pagination rejoins as one process"
        );
        assert!(
            p.agent
                .thread_rows
                .iter()
                .all(|row| row.range.end <= thread.items.len())
        );
    });
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn loading_older_messages_never_drops_a_live_session(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("load-older");
    let (handle, this) = open_with(
        cx,
        Some(data.clone()),
        AgentStore {
            history: Some(Arc::new(History::new(data.join("history.sqlite")))),
            default_workspace: None,
        },
    );
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    until(cx, &this, "the session is in the history", |p| {
        p.agent.current().is_some_and(|s| s.db.is_some())
    });
    let key = this.read_with(cx, |p, _| p.agent.current.unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.agent_load_older(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    // Still the same live session, with its agent (and so its review snapshots).
    this.read_with(cx, |p, _| {
        let session = p.agent.session(key).expect("the live session stays");
        assert!(session.client.is_some());
        assert!(p.message.contains("会话进行中"), "{}", p.message);
    });
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn history_writes_reach_the_database_in_the_order_they_were_queued(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("history-order");
    let store = Arc::new(History::new(data.join("history.sqlite")));
    let (_, this) = open_with(cx, Some(data.clone()), empty_store());
    let id = store
        .create_session(workspace_editor_agent_history::NewSession {
            workspace_root: data.clone(),
            workspace_name: None,
            agent_id: "fake".into(),
            acp_session_id: None,
            title: None,
            first_prompt: None,
            repo: None,
            branch: None,
            created_at: None,
        })
        .unwrap();
    // Each batch of a turn queues its own write; the scheduler may run tasks in any order.
    this.update(cx, |_, cx| {
        for n in 0..30 {
            let text = n.to_string();
            super::history::background_history(cx, store.clone(), move |history| {
                history.append_message(id, workspace_editor_agent_history::Role::Agent, text)
            });
        }
    });
    let mut texts = Vec::new();
    for _ in 0..200 {
        cx.run_until_parked();
        store.flush().unwrap();
        texts = store
            .messages(id)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect::<Vec<_>>();
        if texts.len() == 30 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let expected: Vec<String> = (0..30).map(|n| n.to_string()).collect();
    assert_eq!(texts, expected);
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn a_live_session_whose_record_was_deleted_lets_go_of_it(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let data = temp_root("deleted-record");
    let store = Arc::new(History::new(data.join("history.sqlite")));
    let (handle, this) = open_with(
        cx,
        Some(data.clone()),
        AgentStore {
            history: Some(store.clone()),
            default_workspace: None,
        },
    );
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    until(cx, &this, "the session is in the history", |p| {
        p.agent.current().is_some_and(|s| s.db.is_some())
    });
    let id = this.read_with(cx, |p, _| p.agent.current().unwrap().db.unwrap());
    // Archive the still-open session, then 删除已归档.
    for op in [
        super::history::HistoryOp::Archive(id, true),
        super::history::HistoryOp::DeleteArchived(Default::default()),
    ] {
        cx.update_window(handle.into(), |_, window, cx| {
            this.update(cx, |p, cx| p.agent_history_op(op, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        store.flush().unwrap();
    }
    until(
        cx,
        &this,
        "the live session forgets the deleted record",
        |p| p.agent.current().is_some_and(|s| s.db.is_none()),
    );
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn answering_a_request_the_agent_no_longer_waits_for_changes_nothing(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = temp_root("stale-permission");
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, "permission");
    until(cx, &this, "a permission request", |p| {
        pending_permission(p).is_some()
    });
    let (key, id) = this.read_with(cx, |p, _| pending_permission(p).unwrap());
    // 停止 answers the request `cancelled`; the card stays until the turn ends.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_cancel(cx);
            p.agent_answer(key, id, PermissionChoice::Once, window, cx);
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(p.message.contains("已失效"), "{}", p.message)
    });
    settle(cx, &this);
    assert!(!replies(cx, &this).contains("selected:allow"));
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn sessions_left_idle_are_put_away_unless_something_still_needs_them(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let data = temp_root("reclaim");
    let (handle, this) = open_with(
        cx,
        Some(data.clone()),
        AgentStore {
            history: Some(Arc::new(History::new(data.join("history.sqlite")))),
            default_workspace: None,
        },
    );
    send(cx, handle, &this, "echo hi");
    settle(cx, &this);
    until(cx, &this, "the session is in the history", |p| {
        p.agent.current().is_some_and(|s| s.db.is_some())
    });
    let first = this.read_with(cx, |p, _| p.agent.current.unwrap());
    // Another session is shown; the first one has been left alone for an hour.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_new_session(None, window, cx);
            let session = p.agent.session_mut(first).unwrap();
            session.last_active = std::time::Instant::now() - Duration::from_secs(3600);
            // An unread reply keeps it.
            session.thread.unread = true;
            p.agent_reclaim_idle(cx);
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| assert!(p.agent.session(first).is_some()));
    cx.update_window(handle.into(), |_, _, cx| {
        this.update(cx, |p, cx| {
            p.agent.session_mut(first).unwrap().thread.unread = false;
            p.agent_reclaim_idle(cx);
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| {
        assert!(
            p.agent.session(first).is_none(),
            "an idle, saved session is put away"
        );
        assert!(p.agent.current.is_some_and(|k| k != first));
    });
    let _ = std::fs::remove_dir_all(data);
}

#[gpui_kit::test]
async fn files_written_one_after_another_all_get_their_line_counts(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("recount");
    let (a, b) = (root.join("a.txt"), root.join("b.txt"));
    std::fs::write(&a, "one\n").unwrap();
    std::fs::write(&b, "two\n").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    send(cx, handle, &this, &format!("write {} ONE", a.display()));
    settle(cx, &this);
    send(cx, handle, &this, &format!("write {} TWO", b.display()));
    settle(cx, &this);
    until(cx, &this, "both files are counted", |p| {
        p.agent.current().is_some_and(|s| {
            [&a, &b].iter().all(|path| {
                s.thread
                    .changed_files
                    .get(*path)
                    .is_some_and(|c| c.added + c.removed > 0)
            })
        })
    });
    let _ = std::fs::remove_dir_all(root);
}

#[gpui_kit::test]
async fn enter_in_the_file_picker_attaches_without_sending(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("mention-enter");
    std::fs::write(root.join("alpha.rs"), "").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    until(cx, &this, "index", |p| p.index.is_some());
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |this, cx| this.agent_focus_composer(window, cx));
        window.input("look @alp", cx);
        window.render_frame(cx);
    })
    .unwrap();
    until(cx, &this, "the picker lists the file", |p| {
        p.agent
            .mention
            .as_ref()
            .is_some_and(|m| !m.results.is_empty())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(sent(cx, &this), (vec![], "look ".to_string()));
    this.read_with(cx, |p, _| {
        assert_eq!(
            p.agent.attachments,
            [Attachment::File(root.join("alpha.rs"))]
        );
    });
    let _ = std::fs::remove_dir_all(root);
}

/// A file link in a reply opens in ZJ, at its line; the system is not asked to open it.
#[gpui_kit::test]
async fn a_file_link_in_a_reply_opens_in_zj(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = temp_root("link");
    std::fs::create_dir_all(root.join("configs")).unwrap();
    let file = root.join("configs/flower.py");
    std::fs::write(&file, "a = 1\nb = 2\nc = 3\n").unwrap();
    let (handle, this) = open(cx, Some(root.clone()));
    // Relative to the folder, with a line, as agents write them.
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_link("configs/flower.py:3", window, cx)
        });
    })
    .unwrap();
    wait(
        cx,
        None,
        Some(handle),
        |cx| {
            this.read_with(cx, |p, cx| {
                p.active_document()
                    .is_some_and(|(path, editor)| path == file && editor.read(cx).cursor() == 12)
            })
        },
        |_| "the linked file never opened at its line".into(),
    );
    this.update(cx, |p, cx| {
        p.message.clear();
        cx.notify();
    });
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_open_link("configs/missing.py", window, cx)
        });
    })
    .unwrap();
    this.read_with(cx, |p, _| assert!(p.message.contains("找不到链接的文件")));
    let _ = std::fs::remove_dir_all(root);
}
