//! End-to-end tests against `zj-fake-acp-agent` (src/bin/fake_acp_agent.rs).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use workspace_editor_agent::{
    AgentClient, AgentEvent, AgentPreset, BufferProvider, ClientError, ClientOptions, ExitReason,
    Glyph, PermissionKind, PromptPart, SearchPath, ToolContent, ToolKind, ToolStatus, TurnOutcome,
    registry::{EnvValue, Launch},
};

const FAKE: &str = env!("CARGO_BIN_EXE_zj-fake-acp-agent");

struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "zj-agent-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        Self(std::fs::canonicalize(dir).unwrap())
    }
    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn preset(env: &[(&str, &str)]) -> AgentPreset {
    AgentPreset {
        id: "fake".into(),
        display_name: "Fake".into(),
        glyph: Glyph::Generic,
        launch: vec![Launch::Binary {
            program: FAKE.into(),
            args: vec![],
        }],
        env: env
            .iter()
            .map(|(k, v)| (k.to_string(), EnvValue::Literal(v.to_string())))
            .collect(),
        install_hint: "test".into(),
        modes: Default::default(),
        session_meta: None,
        local_cli: None,
    }
}

fn options(ws: &Workspace, env: &[(&str, &str)]) -> ClientOptions {
    let mut options = ClientOptions::new(preset(env), &ws.0);
    options.handshake_timeout = Duration::from_secs(20);
    options.search_path = Some(SearchPath::new(vec![]));
    options
}

struct Events {
    rx: async_channel::Receiver<AgentEvent>,
    deadline: Duration,
}

impl Events {
    fn of(client: &AgentClient) -> Self {
        Self {
            rx: client.events(),
            deadline: Duration::from_secs(20),
        }
    }

    fn next(&self) -> AgentEvent {
        let deadline = self.deadline;
        async_io::block_on(async {
            let recv = std::pin::pin!(self.rx.recv());
            match futures::future::select(recv, async_io::Timer::after(deadline)).await {
                futures::future::Either::Left((Ok(event), _)) => event,
                futures::future::Either::Left((Err(_), _)) => panic!("event channel closed"),
                futures::future::Either::Right(_) => panic!("no event within {deadline:?}"),
            }
        })
    }

    /// Events up to and including the first one matching `stop`.
    fn until(&self, stop: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
        let mut seen = Vec::new();
        loop {
            let event = self.next();
            let done = stop(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    fn turn(&self) -> Vec<AgentEvent> {
        self.until(|e| matches!(e, AgentEvent::TurnEnded { .. }))
    }
}

fn text(t: &str) -> Vec<PromptPart> {
    vec![PromptPart::Text(t.into())]
}

fn message(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageChunk { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn outcome(events: &[AgentEvent]) -> TurnOutcome {
    match events.last() {
        Some(AgentEvent::TurnEnded { outcome, .. }) => outcome.clone(),
        other => panic!("turn did not end: {other:?}"),
    }
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists; no memory is touched.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn pid_of(events: &[AgentEvent]) -> u32 {
    let text = message(events);
    text.strip_prefix("pid:").unwrap().parse().unwrap()
}

#[test]
fn handshake_streaming_and_reuse() {
    let ws = Workspace::new("stream");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let turn = client.prompt(text("echo 你好 world")).unwrap();
    let seen = events.turn();
    assert!(matches!(seen[0], AgentEvent::Starting { .. }), "{seen:?}");
    match &seen[1] {
        AgentEvent::Ready(info) => {
            assert_eq!(info.name.as_deref(), Some("fake-agent"));
            assert!(!info.load_session);
            assert!(info.embedded_context);
        }
        other => panic!("{other:?}"),
    }
    // The commands the agent sends after `session/new` may arrive before its answer.
    match seen
        .iter()
        .find(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    {
        Some(AgentEvent::SessionStarted { resumed, modes, .. }) => {
            assert!(!resumed);
            let modes = modes.as_ref().unwrap();
            assert_eq!(modes.current, "default");
            assert_eq!(modes.available.len(), 2);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::ThoughtChunk { text } if text == "thinking"))
    );
    assert_eq!(message(&seen), "你好 world");
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::Plan(p) if p.len() == 2))
    );
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::AvailableCommands(c) if c[0].name == "review"))
    );
    assert!(seen.iter().any(|e| matches!(
        e,
        AgentEvent::Usage {
            used: 1200,
            size: 200000,
            ..
        }
    )));
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::TitleChanged { title: Some(t) } if t == "Echo title"))
    );
    assert!(
        matches!(seen.last(), Some(AgentEvent::TurnEnded { turn: t, outcome: TurnOutcome::EndTurn }) if *t == turn)
    );

    // The second prompt reuses the running process and session.
    let pid = client.pid().unwrap();
    client.prompt(text("pid")).unwrap();
    let seen = events.turn();
    assert_eq!(pid_of(&seen), pid);
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, AgentEvent::Starting { .. }))
    );

    client.set_mode("plan");
    let seen = events.until(|e| matches!(e, AgentEvent::ModeChanged { .. }));
    assert!(matches!(seen.last(), Some(AgentEvent::ModeChanged { mode_id }) if mode_id == "plan"));

    client.shutdown();
    assert!(!alive(pid), "agent process survived shutdown");
}

#[test]
fn tool_calls_carry_kind_locations_and_diffs() {
    let ws = Workspace::new("tool");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let target = ws.path("src/a.rs");
    client
        .prompt(text(&format!("tool {}", target.display())))
        .unwrap();
    let seen = events.turn();
    let call = seen
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCall(c) => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.id, "t1");
    assert_eq!(call.kind, ToolKind::Edit);
    assert_eq!(call.status, ToolStatus::Pending);
    assert_eq!(call.locations[0].path, target);
    assert_eq!(call.locations[0].line, Some(3));
    assert_eq!(
        call.content,
        vec![ToolContent::Diff {
            path: target,
            new_file: false,
            added: 1,
            removed: 1,
        }]
    );
    let update = seen
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolCallUpdate(u) => Some(u.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(update.status, Some(ToolStatus::Completed));
    assert_eq!(update.title, None);
}

#[test]
fn permission_round_trip_and_cancel() {
    let ws = Workspace::new("perm");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    client.prompt(text("permission")).unwrap();
    let seen = events.until(|e| matches!(e, AgentEvent::PermissionRequested(_)));
    let AgentEvent::PermissionRequested(request) = seen.last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(request.tool_call.title.as_deref(), Some("cargo test"));
    assert_eq!(request.tool_call.kind, Some(ToolKind::Execute));
    assert_eq!(request.options[0].kind, PermissionKind::AllowOnce);
    // A turn is running: a second prompt is refused, not queued.
    assert_eq!(client.prompt(text("pid")), Err(ClientError::Busy));
    assert!(client.respond_permission(request.id, Some("allow".into())));
    assert!(!client.respond_permission(request.id, Some("allow".into())));
    let seen = events.turn();
    assert_eq!(message(&seen), "selected:allow");
    assert_eq!(outcome(&seen), TurnOutcome::EndTurn);

    // Cancelling answers the pending request with `cancelled`.
    client.prompt(text("permission")).unwrap();
    events.until(|e| matches!(e, AgentEvent::PermissionRequested(_)));
    client.cancel();
    let seen = events.turn();
    assert_eq!(message(&seen), "permission-cancelled");
    assert_eq!(outcome(&seen), TurnOutcome::Cancelled);
}

#[test]
fn cancel_stops_a_streaming_turn() {
    let ws = Workspace::new("cancel");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let started = Instant::now();
    client.prompt(text("slow")).unwrap();
    events.until(|e| matches!(e, AgentEvent::MessageChunk { .. }));
    client.cancel();
    let seen = events.turn();
    assert_eq!(outcome(&seen), TurnOutcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!client.is_busy());
}

struct Buffers(PathBuf);

impl BufferProvider for Buffers {
    fn buffer_text(&self, path: &Path) -> futures::future::BoxFuture<'static, Option<String>> {
        let text = (path == self.0).then(|| "unsaved buffer\n".to_string());
        Box::pin(async move { text })
    }
}

/// An editor that never answers; reports each request on `asked`.
struct Silent {
    asked: async_channel::Sender<()>,
}

impl BufferProvider for Silent {
    fn buffer_text(&self, _: &Path) -> futures::future::BoxFuture<'static, Option<String>> {
        let _ = self.asked.try_send(());
        Box::pin(futures::future::pending())
    }
}

#[test]
fn direct_writes_take_the_unsaved_buffer_as_the_before() {
    let ws = Workspace::new("buffer-snapshot");
    let file = ws.path("src/a.rs");
    let mut opts = options(&ws, &[]);
    opts.buffers = Some(Arc::new(Buffers(file.clone())));
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client
        .prompt(text(&format!("write {} agent", file.display())))
        .unwrap();
    events.turn();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "agent");
    assert_eq!(
        client.snapshot(&file),
        Some(Some("unsaved buffer\n".into()))
    );
}

#[test]
fn stopping_does_not_wait_for_a_silent_editor() {
    let ws = Workspace::new("silent");
    let (asked, was_asked) = async_channel::bounded(4);
    let mut opts = options(&ws, &[]);
    opts.buffers = Some(Arc::new(Silent { asked }));
    let client = AgentClient::start(opts).unwrap();
    client
        .prompt(text(&format!("read {}", ws.path("src/a.rs").display())))
        .unwrap();
    was_asked.recv_blocking().unwrap();
    let started = Instant::now();
    drop(client);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn reads_prefer_open_buffers_and_stay_in_the_workspace() {
    let ws = Workspace::new("read");
    std::fs::write(ws.path("src/b.rs"), "on disk\n").unwrap();
    let mut opts = options(&ws, &[]);
    opts.buffers = Some(Arc::new(Buffers(ws.path("src/a.rs"))));
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    let ask = |prompt: String| {
        client.prompt(text(&prompt)).unwrap();
        message(&events.turn())
    };
    assert_eq!(
        ask(format!("read {}", ws.path("src/a.rs").display())),
        "content:unsaved buffer\n"
    );
    assert_eq!(
        ask(format!("read {}", ws.path("src/b.rs").display())),
        "content:on disk\n"
    );
    let outside = ask("read /etc/hostname".into());
    assert!(
        outside.starts_with("error:") && outside.contains("不在工作区内"),
        "{outside}"
    );
    let missing = ask(format!("read {}", ws.path("src/none.rs").display()));
    assert!(missing.starts_with("error:"), "{missing}");
}

#[test]
fn direct_writes_hit_the_disk() {
    let ws = Workspace::new("direct");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let path = ws.path("new/dir/note.txt");
    client
        .prompt(text(&format!("write {} 你好 disk", path.display())))
        .unwrap();
    let seen = events.turn();
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::FileWritten { path: p } if *p == path))
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "你好 disk");
    assert_eq!(message(&seen), "readback:你好 disk");
}

#[test]
fn crash_is_reported_and_the_next_prompt_restarts() {
    let ws = Workspace::new("crash");
    let client = AgentClient::start(options(&ws, &[("FAKE_LOAD_SESSION", "1")])).unwrap();
    let events = Events::of(&client);
    client.prompt(text("pid")).unwrap();
    let first = events.turn();
    let first_pid = pid_of(&first);
    let first_session = first
        .iter()
        .find_map(|e| match e {
            AgentEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .unwrap();

    client.prompt(text("crash")).unwrap();
    let seen = events.until(|e| matches!(e, AgentEvent::Exited { .. }));
    assert!(
        matches!(outcome(&seen[..seen.len() - 1]), TurnOutcome::Failed(_)),
        "{seen:?}"
    );
    match seen.last().unwrap() {
        AgentEvent::Exited {
            reason: ExitReason::Crashed {
                code, stderr_tail, ..
            },
        } => {
            assert_eq!(*code, Some(3));
            assert!(stderr_tail.contains("crashing on purpose"), "{stderr_tail}");
        }
        other => panic!("{other:?}"),
    }
    assert!(!alive(first_pid));
    assert_eq!(client.pid(), None);

    // Restart with session/load: same session id, replayed history is not re-emitted.
    client.prompt(text("pid")).unwrap();
    let seen = events.turn();
    assert!(matches!(seen[0], AgentEvent::Starting { .. }));
    assert!(seen.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { session_id, resumed: true, .. } if *session_id == first_session
    )));
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, AgentEvent::UserMessageChunk { .. }))
    );
    // A replayed edit is history: it takes no "before the agent" snapshot.
    assert!(client.snapshot_paths().is_empty());
    let second_pid = pid_of(&seen);
    assert_ne!(first_pid, second_pid);
}

#[test]
fn idle_agent_is_stopped_and_restarted_transparently() {
    let ws = Workspace::new("idle");
    let mut opts = options(&ws, &[]);
    opts.idle_timeout = Duration::from_millis(300);
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("pid")).unwrap();
    let pid = pid_of(&events.turn());
    let seen = events.until(|e| matches!(e, AgentEvent::Exited { .. }));
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::Exited {
            reason: ExitReason::Idle
        })
    ));
    assert!(!alive(pid), "idle agent still running");
    assert_eq!(client.pid(), None);
    // No loadSession: a fresh session is started.
    client.prompt(text("echo again")).unwrap();
    let seen = events.turn();
    assert!(matches!(seen[0], AgentEvent::Starting { .. }));
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { resumed: false, .. }))
    );
    assert_eq!(message(&seen), "again");
}

#[test]
fn a_long_turn_is_not_cut_by_the_idle_timer() {
    let ws = Workspace::new("busy");
    let mut opts = options(&ws, &[]);
    opts.idle_timeout = Duration::from_millis(100);
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("permission")).unwrap();
    let seen = events.until(|e| matches!(e, AgentEvent::PermissionRequested(_)));
    let AgentEvent::PermissionRequested(request) = seen.last().unwrap().clone() else {
        unreachable!()
    };
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        client.pid().is_some(),
        "stopped while waiting for permission"
    );
    client.respond_permission(request.id, Some("reject".into()));
    assert_eq!(message(&events.turn()), "selected:reject");
}

#[test]
fn prompt_context_links_and_embeds() {
    let ws = Workspace::new("links");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let file = ws.path("src/a.rs");
    client
        .prompt(vec![
            PromptPart::Text("links".into()),
            PromptPart::File(file.clone()),
            PromptPart::Selection {
                path: file.clone(),
                start_line: 1,
                end_line: 1,
                text: Some("fn a() {}".into()),
            },
        ])
        .unwrap();
    let reply = message(&events.turn());
    let uri = format!("file://{}", file.display());
    assert_eq!(
        reply,
        format!("link:{uri}\nembedded:{uri}#L1:1=fn a() {{}}")
    );
}

#[test]
fn launch_errors_are_friendly() {
    let ws = Workspace::new("launch");
    let mut opts = ClientOptions::new(AgentPreset::find_builtin("claude-code").unwrap(), &ws.0);
    opts.search_path = Some(SearchPath::new(vec![]));
    // No install directory: nothing may be downloaded.
    opts.install_root = None;
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("hi")).unwrap();
    let seen = events.turn();
    match &seen[0] {
        AgentEvent::Error { message } => assert!(message.contains("Node.js"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(outcome(&seen), TurnOutcome::Failed(m) if m.contains("Node.js")));
    assert!(!client.is_busy());

    let mut opts = options(&ws, &[]);
    opts.preset.launch = vec![Launch::Binary {
        program: "/nonexistent/agent".into(),
        args: vec![],
    }];
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("hi")).unwrap();
    assert!(
        matches!(outcome(&events.turn()), TurnOutcome::Failed(m) if m.contains("/nonexistent/agent"))
    );
}

#[test]
fn prompts_wait_for_a_login_then_continue() {
    let ws = Workspace::new("login");
    let auth = ws.path("auth");
    let auth_env = auth.to_string_lossy().into_owned();
    let client = AgentClient::start(options(&ws, &[("FAKE_AUTH", &auth_env)])).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    let required = |seen: &[AgentEvent]| match seen.last() {
        Some(AgentEvent::AuthRequired { methods }) => methods.clone(),
        other => panic!("{other:?}"),
    };
    let methods = required(&events.until(|e| matches!(e, AgentEvent::AuthRequired { .. })));
    assert_eq!(methods[0].id, "fake-login");
    assert!(!methods[0].terminal);
    if cfg!(target_os = "macos") {
        assert!(
            methods
                .iter()
                .any(|m| m.id == "fake-terminal" && m.terminal)
        );
    }
    assert!(client.is_busy());
    // Not signed in yet: asked again.
    client.retry_login(BTreeMap::new());
    required(&events.until(|e| matches!(e, AgentEvent::AuthRequired { .. })));
    client.login("fake-login");
    let seen = events.turn();
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    );
    assert_eq!(outcome(&seen), TurnOutcome::EndTurn);
    assert_eq!(message(&seen), "hi");
    assert!(auth.exists());
    client.shutdown();

    std::fs::remove_file(&auth).unwrap();
    let client = AgentClient::start(options(&ws, &[("FAKE_AUTH", &auth_env)])).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    events.until(|e| matches!(e, AgentEvent::AuthRequired { .. }));
    client.cancel();
    assert_eq!(outcome(&events.turn()), TurnOutcome::Cancelled);
    assert!(!client.is_busy());
}

#[test]
fn retrying_a_login_restarts_the_agent_with_new_variables() {
    let ws = Workspace::new("login-restart");
    let var = |name: &str| {
        BTreeMap::from([(
            "FAKE_AUTH".to_string(),
            ws.path(name).to_string_lossy().into_owned(),
        )])
    };
    // Not declared by the preset: a settings-only variable still reaches the agent.
    let mut options = options(&ws, &[]);
    options.env_overrides = var("old");
    let client = AgentClient::start(options).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    events.until(|e| matches!(e, AgentEvent::AuthRequired { .. }));
    // The user put a working key in the settings: only the new process can see it.
    std::fs::write(ws.path("new"), "").unwrap();
    client.retry_login(var("new"));
    let seen = events.turn();
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::Starting { .. }))
    );
    assert!(!seen.iter().any(|e| matches!(e, AgentEvent::Exited { .. })));
    assert_eq!(outcome(&seen), TurnOutcome::EndTurn);
    assert_eq!(message(&seen), "hi");
    assert!(!ws.path("old").exists());
    client.shutdown();
}

#[test]
fn prompts_refused_for_a_login_are_sent_again() {
    let ws = Workspace::new("login-prompt");
    let auth = ws.path("auth");
    let auth_env = auth.to_string_lossy().into_owned();
    let client = AgentClient::start(options(
        &ws,
        &[("FAKE_AUTH", &auth_env), ("FAKE_AUTH_AT", "prompt")],
    ))
    .unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    let seen = events.until(|e| matches!(e, AgentEvent::AuthRequired { .. }));
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnded { .. }))
    );
    assert!(client.is_busy());
    client.retry_login(BTreeMap::new());
    events.until(|e| matches!(e, AgentEvent::AuthRequired { .. }));
    client.login("fake-login");
    let seen = events.turn();
    assert_eq!(outcome(&seen), TurnOutcome::EndTurn);
    assert_eq!(message(&seen), "hi");
    // Same session: no second `session/new`.
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    );
    client.shutdown();
}

fn write_script(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn npm_adapters_are_installed_on_first_use() {
    let ws = Workspace::new("install");
    let bin = ws.path("node-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = ws.path("npm.log");
    // `node <entry>` starts the fake agent; `npm install --prefix DIR` lays out a package.
    write_script(
        &bin.join("node"),
        &format!("[ \"$1\" = --version ] && echo v24.0.0 && exit 0\nexec '{FAKE}'"),
    );
    write_script(
        &bin.join("npm"),
        &format!(
            "echo \"$*\" >> '{}'\nwhile [ $# -gt 0 ]; do [ \"$1\" = --prefix ] && p=\"$2\"; shift; done\n\
             d=\"$p/node_modules/@zj/fake\"; mkdir -p \"$d/dist\"\n\
             echo '{{\"bin\":{{\"fake\":\"dist/index.js\"}}}}' > \"$d/package.json\"\n: > \"$d/dist/index.js\"",
            log.display()
        ),
    );
    let start = || {
        let mut opts = options(&ws, &[]);
        opts.preset.launch = vec![Launch::Package {
            package: "@zj/fake@1.0.0".into(),
            bin: "fake".into(),
            args: vec![],
        }];
        opts.search_path = Some(SearchPath::new(vec![bin.clone()]));
        opts.install_root = Some(ws.path("data"));
        AgentClient::start(opts).unwrap()
    };
    let client = start();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    let seen = events.turn();
    assert!(
        matches!(&seen[0], AgentEvent::Progress { message } if message.contains("首次使用")),
        "{seen:?}"
    );
    assert_eq!(outcome(&seen), TurnOutcome::EndTurn);
    assert_eq!(message(&seen), "hi");
    let installs = std::fs::read_to_string(&log).unwrap();
    assert_eq!(installs.lines().count(), 1);
    assert!(installs.contains("@zj/fake@1.0.0"), "{installs}");
    client.shutdown();

    // Installed: the next client starts it directly.
    let client = start();
    let events = Events::of(&client);
    client.prompt(text("echo again")).unwrap();
    let seen = events.turn();
    assert!(matches!(&seen[0], AgentEvent::Starting { .. }), "{seen:?}");
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
}

#[test]
fn overrides_fill_preset_env() {
    let ws = Workspace::new("env");
    let mut opts = options(&ws, &[]);
    opts.preset.env = vec![(
        "ZJ_PROBE_TOKEN".into(),
        EnvValue::FromEnv("ZJ_UNSET_FOR_TEST".into()),
    )];
    opts.env_overrides = BTreeMap::from([("ZJ_PROBE_TOKEN".into(), "from-settings".into())]);
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("env ZJ_PROBE_TOKEN")).unwrap();
    assert_eq!(message(&events.turn()), "env:from-settings");
}

/// Real adapter: handshake and `session/new` (no prompt, so no login is needed for the
/// handshake). Downloads the package through npx on first run:
/// `cargo test -p workspace-editor-agent -- --ignored real_claude --nocapture`.
#[test]
#[ignore]
fn real_claude_agent_acp_handshake() {
    let ws = Workspace::new("real");
    let mut opts = ClientOptions::new(AgentPreset::find_builtin("claude-code").unwrap(), &ws.0);
    opts.handshake_timeout = Duration::from_secs(300);
    let client = AgentClient::start(opts).unwrap();
    let mut events = Events::of(&client);
    events.deadline = Duration::from_secs(300);
    client.connect();
    let mut seen = events.until(|e| {
        matches!(
            e,
            AgentEvent::Ready(_) | AgentEvent::Error { .. } | AgentEvent::Exited { .. }
        )
    });
    // Adapter alone (Node + ACP), before session/new starts Claude Code itself.
    if let Some(pid) = client.pid() {
        std::thread::sleep(Duration::from_millis(500));
        eprintln!(
            "event=real_claude_rss stage=ready group_rss_kib={}",
            group_rss_kib(pid)
        );
    }
    if matches!(seen.last(), Some(AgentEvent::Ready(_))) {
        seen.extend(events.until(|e| {
            matches!(
                e,
                AgentEvent::SessionStarted { .. }
                    | AgentEvent::Error { .. }
                    | AgentEvent::Exited { .. }
            )
        }));
    }
    eprintln!("event=real_claude_handshake events={seen:?}");
    assert!(
        seen.iter().any(|e| matches!(e, AgentEvent::Ready(_))),
        "{seen:?}"
    );
    if let Some(pid) = client.pid() {
        std::thread::sleep(Duration::from_secs(3));
        eprintln!(
            "event=real_claude_rss stage=session group_rss_kib={}",
            group_rss_kib(pid)
        );
    }
}

/// Sum of VmRSS over the agent's process group (Linux only; 0 elsewhere).
fn group_rss_kib(pgid: u32) -> u64 {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // pid (comm) state ppid pgrp ...
        let Some(after) = stat.rsplit_once(") ").map(|(_, a)| a) else {
            continue;
        };
        if after.split_whitespace().nth(2) != Some(&pgid.to_string()) {
            continue;
        }
        let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        total += status
            .lines()
            .find(|l| l.starts_with("VmRSS:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
            .unwrap_or(0);
    }
    total
}

#[test]
fn sessions_start_in_ask_mode_and_never_request_bypass() {
    let ws = Workspace::new("modes");
    let mut opts = options(&ws, &[("FAKE_BYPASS_DEFAULT", "1")]);
    let claude = AgentPreset::find_builtin("claude-code").unwrap();
    opts.preset.modes = claude.modes.clone();
    opts.preset.session_meta = claude.session_meta.clone();
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("modes")).unwrap();
    let seen = events.turn();
    // The forbidden mode is never offered to the UI.
    let modes = seen
        .iter()
        .find_map(|e| match e {
            AgentEvent::SessionStarted { modes, .. } => modes.clone(),
            _ => None,
        })
        .unwrap();
    assert!(
        modes
            .available
            .iter()
            .all(|(id, _)| id != "bypassPermissions")
    );
    // The agent started in bypass (user settings); ZJ switched it to default first.
    assert!(
        seen.iter()
            .any(|e| matches!(e, AgentEvent::ModeChanged { mode_id } if mode_id == "default"))
    );
    let reply = message(&seen);
    assert!(
        reply.contains(r#""allowDangerouslySkipPermissions":false"#),
        "{reply}"
    );
    assert!(reply.ends_with("modes:default"), "{reply}");
    // The UI cannot ask for it either.
    assert!(!client.set_mode("bypassPermissions"));
    assert!(client.set_mode("plan"));
    events.until(|e| matches!(e, AgentEvent::ModeChanged { mode_id } if mode_id == "plan"));
    client.prompt(text("modes")).unwrap();
    let reply = message(&events.turn());
    assert!(reply.ends_with("modes:default,plan"), "{reply}");
    assert!(!reply.contains("modes:bypass") && !reply.contains(",bypass"));
    // The agent switching itself to bypass is warned about and switched straight back.
    client.prompt(text("bypass")).unwrap();
    let seen = events.turn();
    assert!(
        seen.iter().any(
            |e| matches!(e, AgentEvent::Error { message } if message.contains("已切回 default"))
        ),
        "{seen:?}"
    );
    assert!(!seen.iter().any(
        |e| matches!(e, AgentEvent::ModeChanged { mode_id } if mode_id == "bypassPermissions")
    ));
    let restored =
        |e: &AgentEvent| matches!(e, AgentEvent::ModeChanged { mode_id } if mode_id == "default");
    if !seen.iter().any(restored) {
        events.until(restored);
    }
    client.prompt(text("modes")).unwrap();
    let reply = message(&events.turn());
    assert!(reply.ends_with("modes:default,plan,default"), "{reply}");
}

#[test]
fn model_settings_are_offered_but_modes_are_not() {
    let ws = Workspace::new("configs");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    client.prompt(text("configs")).unwrap();
    let seen = events.turn();
    let configs = seen
        .iter()
        .find_map(|e| match e {
            AgentEvent::ConfigOptions(configs) => Some(configs.clone()),
            _ => None,
        })
        .unwrap();
    let ids: Vec<&str> = configs.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["model", "effort"]);
    assert_eq!(configs[0].current, "sonnet");
    assert_eq!(configs[0].values[1], ("opus".into(), "OPUS".into()));
    // Only what the UI is shown can be set: not the mode option, not an unknown value.
    assert!(!client.set_config_option("mode", "bypassPermissions"));
    assert!(!client.set_config_option("model", "haiku"));
    assert!(client.set_config_option("model", "opus"));
    events.until(|e| matches!(e, AgentEvent::ConfigOptions(c) if c[0].current == "opus"));
    client.prompt(text("configs")).unwrap();
    assert_eq!(message(&events.turn()), "configs:model=opus");
    client.shutdown();
}

#[test]
fn direct_writes_remember_the_file_before_the_agent() {
    let ws = Workspace::new("snapshot");
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    let existing = ws.path("src/a.rs");
    let created = ws.path("src/new.rs");
    for (path, body) in [
        (&existing, "first"),
        (&existing, "second"),
        (&created, "new"),
    ] {
        client
            .prompt(text(&format!("write {} {body}", path.display())))
            .unwrap();
        events.turn();
    }
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "second");
    // The original, not the intermediate version.
    assert_eq!(client.snapshot(&existing), Some(Some("fn a() {}\n".into())));
    assert_eq!(client.snapshot(&created), Some(None));
    assert_eq!(client.snapshot_paths().len(), 2);
    client.clear_snapshots();
    assert!(client.snapshot_paths().is_empty());
}

#[test]
fn a_session_from_history_is_resumed_with_load() {
    let ws = Workspace::new("resume");
    let mut opts = options(&ws, &[("FAKE_LOAD_SESSION", "1")]);
    opts.resume_session = Some("s-old-7".into());
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    let seen = events.turn();
    assert!(seen.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { session_id, resumed: true, .. } if session_id == "s-old-7"
    )));
    // The replayed history is not shown again.
    assert_eq!(message(&seen), "hi");
    // Without loadSession support the id is ignored and a new session starts.
    let mut opts = options(&ws, &[]);
    opts.resume_session = Some("s-old-7".into());
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    assert!(
        events
            .turn()
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { resumed: false, .. }))
    );
}

#[test]
fn dropping_the_client_does_not_wait_out_a_hung_handshake() {
    let ws = Workspace::new("hang");
    let mut opts = options(&ws, &[("FAKE_HANG_INIT", "1")]);
    opts.handshake_timeout = Duration::from_secs(120);
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("echo hi")).unwrap();
    events.until(|e| matches!(e, AgentEvent::Starting { .. }));
    // The UI thread drops the client when a window closes; it joins the supervisor.
    let started = std::time::Instant::now();
    drop(client);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_file_without_a_snapshot_is_reported() {
    let ws = Workspace::new("nosnap");
    let file = ws.path("data.bin");
    std::fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
    let client = AgentClient::start(options(&ws, &[])).unwrap();
    let events = Events::of(&client);
    client
        .prompt(text(&format!("write {} text", file.display())))
        .unwrap();
    let seen = events.turn();
    assert!(
        seen.iter().any(
            |e| matches!(e, AgentEvent::Error { message } if message.contains("不能对比或还原"))
        ),
        "{seen:?}"
    );
    assert_eq!(client.snapshot(&file), None);
}

#[test]
fn a_cancel_the_agent_ignores_still_ends_the_turn() {
    let ws = Workspace::new("stuck");
    let mut opts = options(&ws, &[]);
    opts.cancel_grace = Duration::from_millis(300);
    let client = AgentClient::start(opts).unwrap();
    let events = Events::of(&client);
    client.prompt(text("stuck")).unwrap();
    events.until(|e| matches!(e, AgentEvent::SessionStarted { .. }));
    std::thread::sleep(Duration::from_millis(100));
    client.cancel();
    let seen = events.until(|e| matches!(e, AgentEvent::TurnEnded { .. }));
    assert!(
        matches!(outcome(&seen), TurnOutcome::Failed(m) if m.contains("没有响应取消")),
        "{seen:?}"
    );
    // The next prompt is not refused as "a turn is running".
    client.prompt(text("echo again")).unwrap();
}
