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
