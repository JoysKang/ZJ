//! The agent panel in a headless window with the fake ACP agent: sending from the composer,
//! permissions and 始终允许 rules, reviewing and rejecting a change, reopening a stored session.

use super::super::test_support::{empty_store, open_window, wait};
use super::*;
// `super::*` brings in GPUI's `test` macro through `gpui_kit::*`; `#[gpui_kit::test]` expands
// to the built-in one.
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, base::Root};
use workspace_editor_agent::registry::UserAgentConfig;

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
