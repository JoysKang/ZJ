//! Separate test binary: it changes the process environment, which must not race other tests.

use std::time::Duration;
use workspace_editor_agent::{
    AgentClient, AgentEvent, AgentPreset, ClientOptions, Glyph, PromptPart, SearchPath,
    registry::{EnvValue, Launch},
};

#[test]
fn inherited_git_and_claude_variables_are_removed() {
    // SAFETY: this binary has a single test and sets the variables before any thread starts.
    unsafe {
        std::env::set_var("GIT_DIR", "/elsewhere/.git");
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("ZJ_SETTINGS", "/tmp/x.json");
        std::env::set_var("KEEP_ME", "kept");
    }
    let root = std::env::temp_dir().join(format!("zj-agent-env-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let preset = AgentPreset {
        id: "fake".into(),
        display_name: "Fake".into(),
        glyph: Glyph::Generic,
        launch: vec![Launch::Binary {
            program: env!("CARGO_BIN_EXE_zj-fake-acp-agent").into(),
            args: vec![],
        }],
        env: vec![("PRESET_VALUE".into(), EnvValue::Literal("p".into()))],
        install_hint: String::new(),
    };
    let mut options = ClientOptions::new(preset, &root);
    options.search_path = Some(SearchPath::new(vec![]));
    let client = AgentClient::start(options).unwrap();
    let events = client.events();
    let ask = |name: &str| -> String {
        client
            .prompt(vec![PromptPart::Text(format!("env {name}"))])
            .unwrap();
        let mut reply = String::new();
        loop {
            let event = async_io::block_on(async {
                let recv = std::pin::pin!(events.recv());
                match futures::future::select(recv, async_io::Timer::after(Duration::from_secs(20)))
                    .await
                {
                    futures::future::Either::Left((event, _)) => event.unwrap(),
                    futures::future::Either::Right(_) => panic!("timeout"),
                }
            });
            match event {
                AgentEvent::MessageChunk { text } => reply.push_str(&text),
                AgentEvent::TurnEnded { .. } => return reply,
                _ => {}
            }
        }
    };
    assert_eq!(ask("GIT_DIR"), "env:<unset>");
    assert_eq!(ask("CLAUDECODE"), "env:<unset>");
    assert_eq!(ask("ZJ_SETTINGS"), "env:<unset>");
    assert_eq!(ask("KEEP_ME"), "env:kept");
    assert_eq!(ask("PRESET_VALUE"), "env:p");
    // `PATH` is rebuilt from the search path, led by the agent's own directory.
    let bin_dir = std::path::Path::new(env!("CARGO_BIN_EXE_zj-fake-acp-agent"))
        .parent()
        .unwrap();
    assert_eq!(ask("PATH"), format!("env:{}", bin_dir.display()));
    drop(client);
    let _ = std::fs::remove_dir_all(&root);
}
