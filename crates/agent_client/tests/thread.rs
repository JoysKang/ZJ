//! The panel's state model fed by the real client and the fake agent, end to end.

use std::{path::PathBuf, time::Duration};
use workspace_editor_agent::{
    AgentClient, AgentEvent, AgentPreset, ClientOptions, Glyph, PermissionKind, PromptPart,
    SearchPath, WriteMode,
    registry::Launch,
    thread::{ChangeOrigin, Item, PermissionState, Record, Status, Thread},
};

fn client(tag: &str, mode: WriteMode) -> (AgentClient, PathBuf) {
    let root = std::env::temp_dir().join(format!("zj-thread-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("a.rs"), "one\n").unwrap();
    let preset = AgentPreset {
        id: "fake".into(),
        display_name: "Fake".into(),
        glyph: Glyph::Generic,
        launch: vec![Launch::Binary {
            program: env!("CARGO_BIN_EXE_zj-fake-acp-agent").into(),
            args: vec![],
        }],
        env: vec![],
        install_hint: String::new(),
        modes: Default::default(),
        session_meta: None,
    };
    let mut options = ClientOptions::new(preset, &root);
    options.search_path = Some(SearchPath::new(vec![]));
    options.write_mode = mode;
    (AgentClient::start(options).unwrap(), root)
}

/// Sends a prompt and applies events to the thread until the turn ends; permission requests
/// are answered with `answer`.
fn run(client: &AgentClient, thread: &mut Thread, prompt: &str, answer: Option<&str>) {
    let turn = client
        .prompt(vec![PromptPart::Text(prompt.into())])
        .unwrap();
    thread.push_user(prompt.into(), vec![], turn);
    let events = client.events();
    loop {
        let event = async_io::block_on(async {
            let recv = std::pin::pin!(events.recv());
            match futures::future::select(recv, async_io::Timer::after(Duration::from_secs(20)))
                .await
            {
                futures::future::Either::Left((e, _)) => e.unwrap(),
                futures::future::Either::Right(_) => panic!("timeout"),
            }
        });
        thread.apply(&event, true);
        if let AgentEvent::PermissionRequested(request) = &event
            && let Some(answer) = answer
        {
            client.respond_permission(request.id, Some(answer.into()));
            thread.answer_permission(
                request.id,
                PermissionState::Answered(PermissionKind::AllowOnce, answer.into()),
            );
        }
        if matches!(event, AgentEvent::TurnEnded { .. }) {
            return;
        }
    }
}

#[test]
fn a_conversation_builds_the_panel_model() {
    let (client, root) = client("conv", WriteMode::Direct);
    let mut thread = Thread::new();
    run(&client, &mut thread, "echo 你好 world", None);
    assert_eq!(thread.status, Status::Idle);
    assert_eq!(thread.title.as_deref(), Some("Echo title"));
    assert_eq!(thread.usage, Some((1200, 200000)));
    assert_eq!(thread.commands[0].name, "review");
    assert_eq!(thread.modes.as_ref().unwrap().current, "default");
    assert!(
        thread
            .items
            .iter()
            .any(|i| matches!(i, Item::Agent { text, .. } if text == "你好 world"))
    );
    assert!(
        thread
            .items
            .iter()
            .any(|i| matches!(i, Item::Plan(p) if p.len() == 2))
    );
    assert_eq!(
        thread.take_records(),
        vec![Record::Agent("你好 world".into())]
    );

    run(&client, &mut thread, "permission", Some("allow"));
    assert!(thread.items.iter().any(|i| matches!(
        i,
        Item::Permission(card) if matches!(card.state, PermissionState::Answered(..))
    )));
    assert!(
        matches!(thread.items.back(), Some(Item::Agent { text, .. }) if text == "selected:allow")
    );

    let file = root.join("a.rs");
    run(
        &client,
        &mut thread,
        &format!("write {} two", file.display()),
        None,
    );
    assert_eq!(thread.changed_files[&file].origin, ChangeOrigin::Written);
    assert_eq!(client.snapshot(&file), Some(Some("one\n".into())));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn accept_first_proposals_show_as_pending_files() {
    let (client, root) = client("shadow", WriteMode::AcceptFirst);
    let mut thread = Thread::new();
    let file = root.join("a.rs");
    run(
        &client,
        &mut thread,
        &format!("write {} two", file.display()),
        None,
    );
    assert_eq!(thread.changed_files[&file].origin, ChangeOrigin::Proposed);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n");
    assert_eq!(client.shadow().hunks(&file).len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_crash_shows_a_notice_and_an_error_status() {
    let (client, root) = client("crash", WriteMode::Direct);
    let mut thread = Thread::new();
    run(&client, &mut thread, "crash", None);
    assert_eq!(thread.status, Status::Error);
    assert!(
        thread
            .items
            .iter()
            .any(|i| matches!(i, Item::Notice { error: true, .. }))
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The scripted turn the panel screenshots use: reads, a plan, two Direct writes (one new
/// file), then a command that waits for approval. Afterwards the review helpers accept one
/// hunk into the snapshot, reject another on disk, and revert the new file.
#[test]
fn a_full_turn_reviews_hunk_by_hunk() {
    use workspace_editor_agent::{
        review::{resolve_file, resolve_hunk, review_texts},
        thread::{command_prefix, permission_command, rule_matches},
    };
    let root = std::env::temp_dir().join(format!("zj-thread-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/feed")).unwrap();
    std::fs::create_dir_all(root.join("demo/edits")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    let before = "a\nb\nc\nd\ne\nf\n";
    let file = root.join("src/feed/binance.rs");
    std::fs::write(&file, before).unwrap();
    std::fs::write(
        root.join("demo/edits/src__feed__binance.rs"),
        "a\nB\nc\nd\ne\nF\n",
    )
    .unwrap();
    std::fs::write(root.join("demo/edits/tests__new.rs"), "x\n").unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    let preset = AgentPreset {
        id: "fake".into(),
        display_name: "Fake".into(),
        glyph: Glyph::Generic,
        launch: vec![Launch::Binary {
            program: env!("CARGO_BIN_EXE_zj-fake-acp-agent").into(),
            args: vec![],
        }],
        env: vec![(
            "FAKE_DEMO".into(),
            workspace_editor_agent::registry::EnvValue::Literal(
                root.join("demo").display().to_string(),
            ),
        )],
        install_hint: String::new(),
        modes: Default::default(),
        session_meta: None,
    };
    let mut options = ClientOptions::new(preset, &root);
    options.search_path = Some(SearchPath::new(vec![]));
    let client = AgentClient::start(options).unwrap();
    let mut thread = Thread::new();

    // The command needs approval; no rule covers it yet, so the panel would ask.
    let turn = client
        .prompt(vec![PromptPart::Text("demo".into())])
        .unwrap();
    thread.push_user("demo".into(), vec![], turn);
    let events = client.events();
    let next = || {
        async_io::block_on(async {
            let recv = std::pin::pin!(events.recv());
            match futures::future::select(recv, async_io::Timer::after(Duration::from_secs(20)))
                .await
            {
                futures::future::Either::Left((e, _)) => e.unwrap(),
                futures::future::Either::Right(_) => panic!("timeout"),
            }
        })
    };
    let request = loop {
        let event = next();
        thread.apply(&event, false);
        if let AgentEvent::PermissionRequested(request) = event {
            break request;
        }
    };
    assert_eq!(thread.status, Status::Awaiting);
    let command = permission_command(&request).unwrap();
    assert_eq!(command, "cargo test --test reconnect -- --nocapture");
    assert!(!rule_matches(&request, &[]));
    assert!(rule_matches(&request, &[command_prefix(&command)]));
    assert_eq!(thread.changed_files.len(), 2);
    assert!(
        thread
            .items
            .iter()
            .any(|i| matches!(i, Item::Plan(p) if p.len() == 4))
    );
    client.respond_permission(request.id, Some("allow".into()));
    thread.answer_permission(
        request.id,
        PermissionState::Answered(PermissionKind::AllowOnce, "allow".into()),
    );
    loop {
        let event = next();
        thread.apply(&event, false);
        if matches!(event, AgentEvent::TurnEnded { .. }) {
            break;
        }
    }
    assert_eq!(thread.status, Status::Idle);
    assert!(
        thread.unread,
        "the turn ended while the panel was not looking"
    );
    assert_eq!(thread.title.as_deref(), Some("修复重连后序列号缺口"));

    // Review: two hunks against the snapshot taken before the agent wrote.
    let (base, after, origin) = review_texts(&client, &file).unwrap();
    assert_eq!(origin, ChangeOrigin::Written);
    assert_eq!(base.as_deref(), Some(before));
    assert_eq!(after, "a\nB\nc\nd\ne\nF\n");
    resolve_hunk(&client, &file, 0, true).unwrap();
    let (base, _, _) = review_texts(&client, &file).unwrap();
    assert_eq!(
        base.as_deref(),
        Some("a\nB\nc\nd\ne\nf\n"),
        "accepted into the snapshot"
    );
    resolve_hunk(&client, &file, 0, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "a\nB\nc\nd\ne\nf\n"
    );
    assert!(
        review_texts(&client, &file).is_none(),
        "nothing left to review"
    );
    let created = root.join("tests/new.rs");
    assert!(created.exists());
    resolve_file(&client, &created, false).unwrap();
    assert!(!created.exists(), "rejecting a new file removes it");
    let _ = std::fs::remove_dir_all(root);
}
