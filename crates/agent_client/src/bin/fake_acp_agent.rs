//! Test-only ACP agent for `tests/client.rs` and the app's panel tests
//! (`crates/app/src/prototype/agent_ui_tests.rs`). Not shipped: the app does not build it.
//!
//! The first word of the prompt text selects a script:
//! `echo <text>` · `links` · `tool <path>` · `permission` · `read <path>` ·
//! `write <path> <text>` · `slow` · `crash` · `pid` · `env <NAME>` · `demo` (a scripted
//! turn for the panel's screenshots and e2e test: reads, a plan, edits from
//! `$FAKE_DEMO/edits/<path with / as __>`, then a command that needs approval).
//! `FAKE_LOAD_SESSION=1` advertises `loadSession` (history is replayed on load).
//! `FAKE_AUTH=<file>`: `session/new` needs a login until the file exists; `authenticate`
//! with `fake-login` creates it, and a `fake-terminal` method is offered to clients that
//! support terminal logins. With `FAKE_AUTH_AT=prompt` the session starts and
//! `session/prompt` asks for the login instead (like claude-agent-acp).

use agent_client_protocol::{
    self as sdk, Agent, Client, ConnectionTo, Responder, Stdio, schema::v1 as acp,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
struct State {
    cancelled: AtomicBool,
    sessions: AtomicU64,
    /// `_meta` of the last session/new or session/load, and every mode the client asked for.
    meta: std::sync::Mutex<String>,
    mode_requests: std::sync::Mutex<Vec<String>>,
    cwd: std::sync::Mutex<std::path::PathBuf>,
}

fn text(t: impl Into<String>) -> acp::ContentBlock {
    acp::ContentBlock::Text(acp::TextContent::new(t))
}

fn notify(
    cx: &ConnectionTo<Client>,
    session: &acp::SessionId,
    update: acp::SessionUpdate,
) -> sdk::Result<()> {
    cx.send_notification(acp::SessionNotification::new(session.clone(), update))
}

fn say(
    cx: &ConnectionTo<Client>,
    session: &acp::SessionId,
    t: impl Into<String>,
) -> sdk::Result<()> {
    notify(
        cx,
        session,
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(text(t))),
    )
}

async fn run_prompt(
    state: Arc<State>,
    request: acp::PromptRequest,
    responder: Responder<acp::PromptResponse>,
    cx: ConnectionTo<Client>,
) -> sdk::Result<()> {
    state.cancelled.store(false, Ordering::SeqCst);
    let session = request.session_id.clone();
    let prompt: String = request
        .prompt
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut words = prompt.splitn(3, ' ');
    let command = words.next().unwrap_or_default().to_string();
    let arg = words.next().unwrap_or_default().to_string();
    let rest = words.next().unwrap_or_default().to_string();
    let mut stop = acp::StopReason::EndTurn;
    match command.as_str() {
        "echo" => {
            let body = format!("{arg} {rest}").trim().to_string();
            notify(
                &cx,
                &session,
                acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(text("thinking"))),
            )?;
            let mid = body
                .char_indices()
                .nth(body.chars().count() / 2)
                .map_or(0, |(i, _)| i);
            say(&cx, &session, &body[..mid])?;
            say(&cx, &session, &body[mid..])?;
            notify(
                &cx,
                &session,
                acp::SessionUpdate::Plan(acp::Plan::new(vec![
                    acp::PlanEntry::new(
                        "read",
                        acp::PlanEntryPriority::High,
                        acp::PlanEntryStatus::Completed,
                    ),
                    acp::PlanEntry::new(
                        "write",
                        acp::PlanEntryPriority::Low,
                        acp::PlanEntryStatus::InProgress,
                    ),
                ])),
            )?;
            notify(
                &cx,
                &session,
                acp::SessionUpdate::AvailableCommandsUpdate(acp::AvailableCommandsUpdate::new(
                    vec![acp::AvailableCommand::new("review", "Review changes")],
                )),
            )?;
            notify(
                &cx,
                &session,
                acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(1200, 200000)),
            )?;
            notify(
                &cx,
                &session,
                acp::SessionUpdate::SessionInfoUpdate(
                    acp::SessionInfoUpdate::new().title("Echo title"),
                ),
            )?;
        }
        "links" => {
            let described: Vec<String> = request
                .prompt
                .iter()
                .filter_map(|b| match b {
                    acp::ContentBlock::ResourceLink(l) => Some(format!("link:{}", l.uri)),
                    acp::ContentBlock::Resource(r) => match &r.resource {
                        acp::EmbeddedResourceResource::TextResourceContents(t) => {
                            Some(format!("embedded:{}={}", t.uri, t.text))
                        }
                        _ => None,
                    },
                    _ => None,
                })
                .collect();
            say(&cx, &session, described.join("\n"))?;
        }
        "tool" => {
            let path = std::path::PathBuf::from(&arg);
            notify(
                &cx,
                &session,
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new("t1", "Edit file")
                        .kind(acp::ToolKind::Edit)
                        .status(acp::ToolCallStatus::Pending)
                        .locations(vec![acp::ToolCallLocation::new(path.clone()).line(3)])
                        .content(vec![acp::ToolCallContent::Diff(
                            acp::Diff::new(path, "new\n").old_text("old\n"),
                        )]),
                ),
            )?;
            notify(
                &cx,
                &session,
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    "t1",
                    acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
                )),
            )?;
        }
        "permission" => {
            let request = acp::RequestPermissionRequest::new(
                session.clone(),
                acp::ToolCallUpdate::new(
                    "t2",
                    acp::ToolCallUpdateFields::new()
                        .title("cargo test")
                        .kind(acp::ToolKind::Execute),
                ),
                vec![
                    acp::PermissionOption::new(
                        "allow",
                        "Allow once",
                        acp::PermissionOptionKind::AllowOnce,
                    ),
                    acp::PermissionOption::new(
                        "reject",
                        "Reject",
                        acp::PermissionOptionKind::RejectOnce,
                    ),
                ],
            );
            let response = cx.send_request(request).block_task().await?;
            match response.outcome {
                acp::RequestPermissionOutcome::Selected(s) => {
                    say(&cx, &session, format!("selected:{}", s.option_id))?
                }
                _ => {
                    say(&cx, &session, "permission-cancelled")?;
                    stop = acp::StopReason::Cancelled;
                }
            }
        }
        "read" => {
            let result = cx
                .send_request(acp::ReadTextFileRequest::new(session.clone(), &arg))
                .block_task()
                .await;
            match result {
                Ok(r) => say(&cx, &session, format!("content:{}", r.content))?,
                Err(e) => say(&cx, &session, format!("error:{}", e.message))?,
            }
        }
        "write" => {
            let result = cx
                .send_request(acp::WriteTextFileRequest::new(
                    session.clone(),
                    &arg,
                    rest.clone(),
                ))
                .block_task()
                .await;
            if let Err(e) = result {
                say(&cx, &session, format!("error:{}", e.message))?;
            } else {
                let back = cx
                    .send_request(acp::ReadTextFileRequest::new(session.clone(), &arg))
                    .block_task()
                    .await?;
                say(&cx, &session, format!("readback:{}", back.content))?;
            }
        }
        "slow" => {
            for i in 0..500 {
                if state.cancelled.load(Ordering::SeqCst) {
                    stop = acp::StopReason::Cancelled;
                    break;
                }
                say(&cx, &session, format!("{i} "))?;
                async_io::Timer::after(Duration::from_millis(20)).await;
            }
        }
        "crash" => {
            eprintln!("fake agent: crashing on purpose");
            std::process::exit(3);
        }
        "pid" => say(&cx, &session, format!("pid:{}", std::process::id()))?,
        "demo" => {
            let cwd = state.cwd.lock().unwrap().clone();
            stop = demo(&cx, &session, &cwd).await?;
        }
        "modes" => say(
            &cx,
            &session,
            format!(
                "meta:{} modes:{}",
                state.meta.lock().unwrap(),
                state.mode_requests.lock().unwrap().join(",")
            ),
        )?,
        "env" => say(
            &cx,
            &session,
            format!(
                "env:{}",
                std::env::var(&arg).unwrap_or_else(|_| "<unset>".into())
            ),
        )?,
        other => say(&cx, &session, format!("unknown:{other}"))?,
    }
    responder.respond(acp::PromptResponse::new(stop))
}

fn tool(
    cx: &ConnectionTo<Client>,
    session: &acp::SessionId,
    id: &str,
    title: &str,
    kind: acp::ToolKind,
    paths: &[std::path::PathBuf],
    content: Vec<acp::ToolCallContent>,
) -> sdk::Result<()> {
    notify(
        cx,
        session,
        acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(id.to_string(), title)
                .kind(kind)
                .status(acp::ToolCallStatus::Completed)
                .locations(
                    paths
                        .iter()
                        .map(|p| acp::ToolCallLocation::new(p.clone()))
                        .collect(),
                )
                .content(content),
        ),
    )
}

async fn demo(
    cx: &ConnectionTo<Client>,
    session: &acp::SessionId,
    cwd: &std::path::Path,
) -> sdk::Result<acp::StopReason> {
    let dir = std::path::PathBuf::from(std::env::var("FAKE_DEMO").unwrap_or_default());
    let mut edits: Vec<(std::path::PathBuf, String)> = std::fs::read_dir(dir.join("edits"))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().replace("__", "/");
                    Some((cwd.join(name), std::fs::read_to_string(e.path()).ok()?))
                })
                .collect()
        })
        .unwrap_or_default();
    edits.sort();
    let pause = || async_io::Timer::after(Duration::from_millis(30));
    notify(
        cx,
        session,
        acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(text(
            "先看重连路径里 last_update_id 的处理。",
        ))),
    )?;
    let reads: Vec<std::path::PathBuf> =
        ["src/feed/binance.rs", "src/book/l2.rs", "src/feed/mod.rs"]
            .iter()
            .map(|p| cwd.join(p))
            .collect();
    tool(
        cx,
        session,
        "d1",
        "Read 3 files",
        acp::ToolKind::Read,
        &reads,
        vec![],
    )?;
    tool(
        cx,
        session,
        "d2",
        "last_update_id · 7 处，2 个文件",
        acp::ToolKind::Search,
        &[],
        vec![],
    )?;
    pause().await;
    for chunk in [
        "原因在 `on_reconnect`：它只重置了 WebSocket 的退避状态，",
        "`last_update_id` 仍是断线前的值，所以重连后的第一批增量被当成连续数据直接应用。",
    ] {
        say(cx, session, chunk)?;
        pause().await;
    }
    let plan = |done: usize| {
        let steps = [
            "定位重连路径里的序列号处理",
            "重连后清空缓冲，拉取快照后按序回放",
            "添加回归测试 tests/reconnect.rs",
            "运行 cargo test 并确认通过",
        ];
        acp::SessionUpdate::Plan(acp::Plan::new(
            steps
                .iter()
                .enumerate()
                .map(|(i, step)| {
                    acp::PlanEntry::new(
                        *step,
                        acp::PlanEntryPriority::Medium,
                        if i < done {
                            acp::PlanEntryStatus::Completed
                        } else if i == done {
                            acp::PlanEntryStatus::InProgress
                        } else {
                            acp::PlanEntryStatus::Pending
                        },
                    )
                })
                .collect(),
        ))
    };
    notify(cx, session, plan(1))?;
    for (n, (path, after)) in edits.iter().enumerate() {
        let before = std::fs::read_to_string(path).ok();
        let mut diff = acp::Diff::new(path.clone(), after.clone());
        if let Some(before) = &before {
            diff = diff.old_text(before.clone());
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        tool(
            cx,
            session,
            &format!("e{n}"),
            &format!("Edit {name}"),
            acp::ToolKind::Edit,
            std::slice::from_ref(path),
            vec![acp::ToolCallContent::Diff(diff)],
        )?;
        cx.send_request(acp::WriteTextFileRequest::new(
            session.clone(),
            path.clone(),
            after.clone(),
        ))
        .block_task()
        .await?;
    }
    notify(cx, session, plan(2))?;
    notify(
        cx,
        session,
        acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(76_000, 200_000)),
    )?;
    notify(
        cx,
        session,
        acp::SessionUpdate::SessionInfoUpdate(
            acp::SessionInfoUpdate::new().title("修复重连后序列号缺口"),
        ),
    )?;
    let request = acp::RequestPermissionRequest::new(
        session.clone(),
        acp::ToolCallUpdate::new(
            "d9",
            acp::ToolCallUpdateFields::new()
                .title("cargo test --test reconnect -- --nocapture")
                .kind(acp::ToolKind::Execute),
        ),
        vec![
            acp::PermissionOption::new("allow", "Allow once", acp::PermissionOptionKind::AllowOnce),
            acp::PermissionOption::new("reject", "Reject", acp::PermissionOptionKind::RejectOnce),
        ],
    );
    let response = cx.send_request(request).block_task().await?;
    Ok(match response.outcome {
        acp::RequestPermissionOutcome::Selected(s) if s.option_id.to_string() == "allow" => {
            notify(cx, session, plan(4))?;
            say(
                cx,
                session,
                "\n\n测试通过：`test reconnect_resyncs_snapshot ... ok`。",
            )?;
            acp::StopReason::EndTurn
        }
        acp::RequestPermissionOutcome::Selected(_) => {
            say(cx, session, "\n\n好的，不运行测试。")?;
            acp::StopReason::EndTurn
        }
        _ => acp::StopReason::Cancelled,
    })
}

fn main() -> sdk::Result<()> {
    let load = std::env::var_os("FAKE_LOAD_SESSION").is_some();
    let state = Arc::new(State::default());
    let on_new = state.clone();
    let on_cancel = state.clone();
    let on_prompt = state.clone();
    let on_load = state.clone();
    let on_mode = state.clone();
    // FAKE_BYPASS_DEFAULT=1: like a user whose Claude settings default to bypassPermissions.
    let bypass_default = std::env::var_os("FAKE_BYPASS_DEFAULT").is_some();
    let auth_file = std::env::var_os("FAKE_AUTH").map(std::path::PathBuf::from);
    let at_prompt = std::env::var("FAKE_AUTH_AT").is_ok_and(|v| v == "prompt");
    let new_auth = auth_file.clone().filter(|_| !at_prompt);
    let prompt_auth = auth_file.clone().filter(|_| at_prompt);
    let login_file = auth_file.clone();
    async_io::block_on(
        Agent
            .builder()
            .name("fake-agent")
            .on_receive_request(
                async move |request: acp::InitializeRequest,
                            responder: Responder<acp::InitializeResponse>,
                            _cx| {
                    let mut methods = Vec::new();
                    if auth_file.is_some() {
                        methods.push(acp::AuthMethod::Agent(acp::AuthMethodAgent::new(
                            "fake-login",
                            "Fake login",
                        )));
                        if request.client_capabilities.auth.terminal {
                            methods.push(acp::AuthMethod::Terminal(
                                acp::AuthMethodTerminal::new("fake-terminal", "Fake terminal")
                                    .args(vec!["login".into()]),
                            ));
                        }
                    }
                    responder.respond(
                        acp::InitializeResponse::new(request.protocol_version)
                            .auth_methods(methods)
                            .agent_capabilities(
                                acp::AgentCapabilities::new()
                                    .load_session(load)
                                    .prompt_capabilities(
                                        acp::PromptCapabilities::new().embedded_context(true),
                                    ),
                            )
                            .agent_info(acp::Implementation::new("fake-agent", "1.0.0")),
                    )
                },
                sdk::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: acp::NewSessionRequest,
                            responder: Responder<acp::NewSessionResponse>,
                            _cx| {
                    assert!(request.mcp_servers.is_empty());
                    if new_auth.as_ref().is_some_and(|f| !f.exists()) {
                        return responder.respond_with_error(sdk::Error::auth_required());
                    }
                    *on_new.meta.lock().unwrap() = serde_json::to_string(&request.meta).unwrap();
                    let n = on_new.sessions.fetch_add(1, Ordering::SeqCst);
                    *on_new.cwd.lock().unwrap() = request.cwd.clone();
                    let mut modes = vec![
                        acp::SessionMode::new("default", "Default"),
                        acp::SessionMode::new("plan", "Plan"),
                    ];
                    if bypass_default {
                        modes.push(acp::SessionMode::new("bypassPermissions", "Bypass"));
                    }
                    let current = if bypass_default {
                        "bypassPermissions"
                    } else {
                        "default"
                    };
                    responder.respond(
                        acp::NewSessionResponse::new(format!("s-{}-{n}", std::process::id()))
                            .modes(acp::SessionModeState::new(current, modes)),
                    )
                },
                sdk::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: acp::LoadSessionRequest,
                            responder: Responder<acp::LoadSessionResponse>,
                            cx: ConnectionTo<Client>| {
                    *on_load.meta.lock().unwrap() = serde_json::to_string(&request.meta).unwrap();
                    // Replay: the client must not show these again.
                    notify(
                        &cx,
                        &request.session_id,
                        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(text(
                            "old question",
                        ))),
                    )?;
                    say(&cx, &request.session_id, "old answer")?;
                    responder.respond(acp::LoadSessionResponse::new())
                },
                sdk::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: acp::SetSessionModeRequest,
                            responder: Responder<acp::SetSessionModeResponse>,
                            cx: ConnectionTo<Client>| {
                    on_mode
                        .mode_requests
                        .lock()
                        .unwrap()
                        .push(request.mode_id.to_string());
                    notify(
                        &cx,
                        &request.session_id,
                        acp::SessionUpdate::CurrentModeUpdate(acp::CurrentModeUpdate::new(
                            request.mode_id.clone(),
                        )),
                    )?;
                    responder.respond(acp::SetSessionModeResponse::new())
                },
                sdk::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: acp::AuthenticateRequest,
                            responder: Responder<acp::AuthenticateResponse>,
                            _cx| {
                    if request.method_id.to_string() != "fake-login" {
                        return responder.respond_with_error(sdk::Error::invalid_params());
                    }
                    if let Some(file) = &login_file {
                        std::fs::write(file, "ok").unwrap();
                    }
                    responder.respond(acp::AuthenticateResponse::new())
                },
                sdk::on_receive_request!(),
            )
            .on_receive_notification(
                async move |_n: acp::CancelNotification, _cx| {
                    on_cancel.cancelled.store(true, Ordering::SeqCst);
                    Ok(())
                },
                sdk::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: acp::PromptRequest,
                            responder: Responder<acp::PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    if prompt_auth.as_ref().is_some_and(|f| !f.exists()) {
                        return responder.respond_with_error(sdk::Error::auth_required());
                    }
                    let state = on_prompt.clone();
                    let task_cx = cx.clone();
                    cx.spawn(run_prompt(state, request, responder, task_cx))
                },
                sdk::on_receive_request!(),
            )
            .connect_to(Stdio::new()),
    )
}
