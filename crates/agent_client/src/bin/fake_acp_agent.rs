//! Test-only ACP agent for `tests/client.rs`. Not shipped: the app does not build it.
//!
//! The first word of the prompt text selects a script:
//! `echo <text>` · `links` · `tool <path>` · `permission` · `read <path>` ·
//! `write <path> <text>` · `slow` · `crash` · `pid` · `env <NAME>`.
//! `FAKE_LOAD_SESSION=1` advertises `loadSession` (history is replayed on load).

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

fn main() -> sdk::Result<()> {
    let load = std::env::var_os("FAKE_LOAD_SESSION").is_some();
    let state = Arc::new(State::default());
    let on_new = state.clone();
    let on_cancel = state.clone();
    let on_prompt = state.clone();
    async_io::block_on(
        Agent
            .builder()
            .name("fake-agent")
            .on_receive_request(
                async move |request: acp::InitializeRequest,
                            responder: Responder<acp::InitializeResponse>,
                            _cx| {
                    responder.respond(
                        acp::InitializeResponse::new(request.protocol_version)
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
                    let n = on_new.sessions.fetch_add(1, Ordering::SeqCst);
                    responder.respond(
                        acp::NewSessionResponse::new(format!("s-{}-{n}", std::process::id()))
                            .modes(acp::SessionModeState::new(
                                "default",
                                vec![
                                    acp::SessionMode::new("default", "Default"),
                                    acp::SessionMode::new("plan", "Plan"),
                                ],
                            )),
                    )
                },
                sdk::on_receive_request!(),
            )
            .on_receive_request(
                async move |request: acp::LoadSessionRequest,
                            responder: Responder<acp::LoadSessionResponse>,
                            cx: ConnectionTo<Client>| {
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
                    let state = on_prompt.clone();
                    let task_cx = cx.clone();
                    cx.spawn(run_prompt(state, request, responder, task_cx))
                },
                sdk::on_receive_request!(),
            )
            .connect_to(Stdio::new()),
    )
}
