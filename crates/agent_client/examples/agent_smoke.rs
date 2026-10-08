//! Real-agent smoke test (docs/adr/0009): drives `AgentClient` against the Claude Code or
//! Codex CLI on this machine through the bridge and prints what each scenario saw, plus the
//! agent process tree's footprint. It runs real turns (a few short prompts) on the account.
//!
//! ```sh
//! cargo build --release -p workspace-editor-agent-bridge --bin zj-agent-bridge
//! ZJ_AGENT_BRIDGE=target/release/zj-agent-bridge \
//!   cargo run --release -p workspace-editor-agent --example agent_smoke -- codex /tmp/smoke-ws
//! ```
//!
//! Arguments: `<preset id> <empty or scratch workspace> [scenario,…]` with scenarios
//! `edit,image,steer,cancel,config,title,question,commands,pool,resume,text` (all by default;
//! `question` is Claude Code's, `commands` Codex's built-in `/` commands except `/logout`).
#![allow(clippy::print_stdout)] // A report for the terminal.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use workspace_editor_agent::{
    AgentClient, AgentEvent, AgentPool, AgentPreset, ClientOptions, PermissionKind, PromptPart,
    generate_text,
};

// 8×8 red PNG.
const RED_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8CAFWEXHbQSACj/P8Fu7N9hAAAAAElFTkSuQmCC";

struct Run {
    client: AgentClient,
    rx: async_channel::Receiver<AgentEvent>,
    session: Option<String>,
    configs: Vec<(String, String, usize)>,
    title: Option<String>,
}

fn next(rx: &async_channel::Receiver<AgentEvent>, wait: Duration) -> Option<AgentEvent> {
    async_io::block_on(async {
        let recv = std::pin::pin!(rx.recv());
        match futures::future::select(recv, async_io::Timer::after(wait)).await {
            futures::future::Either::Left((Ok(e), _)) => Some(e),
            _ => None,
        }
    })
}

/// What a turn showed: the reply, notes, and how it ended.
struct Turn {
    reply: String,
    notes: Vec<String>,
    end: String,
}

/// Runs a turn to its end, approving permissions; `steer` / `cancel` act once text streams.
fn turn(run: &mut Run, parts: Vec<PromptPart>, steer: Option<&str>, cancel: bool) -> Turn {
    let started = Instant::now();
    let mut t = Turn {
        reply: String::new(),
        notes: Vec::new(),
        end: String::new(),
    };
    if let Err(e) = run.client.prompt(parts) {
        t.end = format!("prompt refused: {e:?}");
        return t;
    }
    let mut acted = false;
    loop {
        let Some(e) = next(&run.rx, Duration::from_secs(240)) else {
            t.end = "TIMEOUT".into();
            return t;
        };
        match &e {
            AgentEvent::MessageChunk { text } => {
                t.reply.push_str(text);
                if !acted && t.reply.len() > 20 {
                    if let Some(s) = steer {
                        acted = true;
                        let r = run.client.steer(vec![PromptPart::Text(s.into())]);
                        t.notes.push(format!("steer -> {r:?}"));
                    }
                    if cancel {
                        acted = true;
                        run.client.cancel();
                        t.notes.push("cancel sent".into());
                    }
                }
            }
            AgentEvent::PermissionRequested(p) => {
                let allow = p
                    .options
                    .iter()
                    .find(|o| o.kind == PermissionKind::AllowOnce);
                t.notes.push(format!("permission: {:?}", p.tool_call.title));
                run.client
                    .respond_permission(p.id, allow.or(p.options.first()).map(|o| o.id.clone()));
            }
            AgentEvent::ToolCall(c) => t
                .notes
                .push(format!("tool {:?} {:?} {:?}", c.kind, c.title, c.content)),
            AgentEvent::FileWritten { path } => t.notes.push(format!("written {}", path.display())),
            AgentEvent::SessionStarted {
                session_id,
                resumed,
                modes,
            } => {
                run.session = Some(session_id.clone());
                let modes = modes.as_ref().map(|m| {
                    let ids: Vec<&String> = m.available.iter().map(|a| &a.0).collect();
                    format!("{} of {ids:?}", m.current)
                });
                t.notes
                    .push(format!("session resumed={resumed} modes={modes:?}"));
            }
            AgentEvent::Ready(info) => t.notes.push(format!(
                "ready {:?} image={} load={}",
                info.name, info.image, info.load_session
            )),
            AgentEvent::ConfigOptions(c) => {
                run.configs = c
                    .iter()
                    .map(|o| (o.id.clone(), o.current.clone(), o.values.len()))
                    .collect();
                t.notes.push(format!("configs {:?}", run.configs));
            }
            AgentEvent::AvailableCommands(c) => t.notes.push(format!("commands {}", c.len())),
            AgentEvent::ModeChanged { mode_id } => t.notes.push(format!("mode {mode_id}")),
            AgentEvent::TitleChanged { title } => {
                t.notes.push(format!("title {title:?}"));
                run.title = title.clone();
            }
            AgentEvent::Error { message } => t.notes.push(format!("ERROR {message}")),
            AgentEvent::AuthRequired { methods } => t.notes.push(format!("AUTH {methods:?}")),
            AgentEvent::Exited { reason } => t.notes.push(format!("exited {reason:?}")),
            AgentEvent::TurnEnded { outcome, .. } => {
                t.end = format!("{outcome:?} in {:.1}s", started.elapsed().as_secs_f64());
                return t;
            }
            _ => {}
        }
    }
}

fn start(preset: &AgentPreset, ws: &Path, pool: &Arc<AgentPool>, resume: Option<String>) -> Run {
    let mut o = ClientOptions::new(preset.clone(), ws);
    o.pool = Some(pool.clone());
    o.resume_session = resume;
    let client = AgentClient::start(o).expect("start");
    let rx = client.events();
    Run {
        client,
        rx,
        session: None,
        configs: Vec::new(),
        title: None,
    }
}

/// Footprint of the agent's process group (bridge and CLI), from macOS `footprint`.
fn footprint(pid: u32) -> String {
    let script = format!(
        "for p in $(pgrep -g $(ps -o pgid= -p {pid})); do printf '%s ' \"$(basename \"$(ps -o comm= -p $p)\")\"; footprint $p 2>/dev/null | grep -m1 -o 'Footprint: [0-9.]* [KMG]B'; done"
    );
    let out = std::process::Command::new("sh")
        .args(["-c", &script])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).replace('\n', "; "))
        .unwrap_or_default();
    out.trim_end_matches("; ").to_string()
}

fn show(name: &str, t: &Turn) {
    println!("\n### {name}: {}", t.end);
    for n in &t.notes {
        println!("  - {}", n.chars().take(300).collect::<String>());
    }
    let reply: String = t.reply.chars().take(240).collect();
    println!("  reply: {}", reply.replace('\n', " / "));
}

fn check(label: &str, ok: bool) {
    println!("  => {label}: {}", if ok { "ok" } else { "FAILED" });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [preset, ws, rest @ ..] = args.as_slice() else {
        eprintln!("usage: agent_smoke <preset id> <workspace> [scenario,…]");
        std::process::exit(2);
    };
    let only: Vec<&str> = rest
        .first()
        .map(|s| s.split(',').collect())
        .unwrap_or_default();
    let want = |s: &str| only.is_empty() || only.contains(&s);
    let ws = PathBuf::from(ws);
    std::fs::create_dir_all(&ws).unwrap();
    let preset = AgentPreset::find_builtin(preset).expect("a built-in preset id");
    let pool = Arc::new(AgentPool::new());
    let mut run = start(&preset, &ws, &pool, None);
    println!("# {}", preset.display_name);

    let t = turn(
        &mut run,
        vec![PromptPart::Text(
            "Reply with exactly the word PINEAPPLE and nothing else.".into(),
        )],
        None,
        false,
    );
    show("basic", &t);
    check("reply", t.reply.contains("PINEAPPLE"));
    if let Some(pid) = run.client.pid() {
        println!("\n### footprint after the first turn: {}", footprint(pid));
    }
    if want("edit") {
        let t = turn(
            &mut run,
            vec![PromptPart::Text(
                "Create a file named hello.txt in the workspace containing the single line: hi. Use your file editing tool, don't run shell commands.".into(),
            )],
            None,
            false,
        );
        show("edit", &t);
        check("file written", ws.join("hello.txt").exists());
        check("review snapshot", !run.client.snapshot_paths().is_empty());
        std::fs::write(ws.join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let t = turn(
            &mut run,
            vec![PromptPart::Text(
                "In notes.txt, replace the line beta with BETA. Use your file editing tool (read the file first if needed), don't run shell commands.".into(),
            )],
            None,
            false,
        );
        show("edit an existing file", &t);
        let notes = ws.join("notes.txt").canonicalize().unwrap();
        check(
            "file changed",
            std::fs::read_to_string(&notes).is_ok_and(|t| t.contains("BETA")),
        );
        check(
            "snapshot is the text before",
            run.client.snapshot(&notes) == Some(Some("alpha\nbeta\ngamma\n".into())),
        );
    }
    if want("image") {
        let parts = vec![
            PromptPart::Image {
                data: RED_PNG.into(),
                mime_type: "image/png".into(),
            },
            PromptPart::Text("What single color fills this image? One word.".into()),
        ];
        let t = turn(&mut run, parts, None, false);
        show("image", &t);
        check("image understood", t.reply.to_lowercase().contains("red"));
    }
    if want("steer") {
        let t = turn(
            &mut run,
            vec![PromptPart::Text(
                "Write the numbers 1 to 40, each on its own line followed by a short sentence about it.".into(),
            )],
            Some("Stop the list now and just reply STEERED."),
            false,
        );
        show("steering", &t);
        check(
            "steered",
            t.reply.contains("STEERED") && t.end.starts_with("EndTurn"),
        );
    }
    if want("cancel") {
        let t = turn(
            &mut run,
            vec![PromptPart::Text(
                "Write a 600-word essay about rivers.".into(),
            )],
            None,
            true,
        );
        show("cancel", &t);
        check("cancelled", t.end.starts_with("Cancelled"));
    }
    if want("config") {
        let model = run.configs.iter().find(|c| c.0 == "model").cloned();
        println!("\n### settings: {:?}", run.configs);
        check(
            "model setting offered",
            model.as_ref().is_some_and(|m| m.2 > 1),
        );
        let effort = run
            .configs
            .iter()
            .find(|c| c.0 == "effort" || c.0 == "reasoning_effort")
            .cloned();
        if let Some((id, _, _)) = effort {
            check("effort accepted", run.client.set_config_option(id, "low"));
        }
        check(
            "mode switch",
            run.client.set_mode(if preset.id == "codex" {
                "read-only"
            } else {
                "plan"
            }),
        );
        let t = turn(
            &mut run,
            vec![PromptPart::Text("Reply with exactly OK.".into())],
            None,
            false,
        );
        show("after settings", &t);
        check("turn after settings", t.end.starts_with("EndTurn"));
        println!("  settings now: {:?}", run.configs);
    }
    // Generated after the first turn (in the background); before any `/rename`.
    if want("title") {
        println!("\n### title: {:?}", run.title);
        check("session titled", run.title.is_some());
    }
    if want("question") && preset.id != "codex" {
        // The harness picks a question's first answer, as ⏎ does.
        let t = turn(
            &mut run,
            vec![PromptPart::Text(
                "Use your AskUserQuestion tool to ask me which color I prefer, with the options Teal and Amber (in that order). Then reply with just the color I chose.".into(),
            )],
            None,
            false,
        );
        show("question", &t);
        check("asked", t.notes.iter().any(|n| n.starts_with("permission")));
        check("answer used", t.reply.contains("Teal"));
    }
    if want("commands") && preset.id == "codex" {
        std::fs::write(ws.join("notes.txt"), "alpha\nBETA\ngamma\nTODO: remove\n").unwrap();
        for (command, expect) in [
            ("/status", "模型"),
            ("/skills", "技能"),
            ("/mcp", "MCP"),
            ("/plan", "规划模式"),
            (
                "Plan how to rename notes.txt to log.txt. Before planning, use your request_user_input tool to ask me whether to keep a backup copy (options: Yes, No). Don't change anything yet.",
                "notes",
            ),
            ("/plan", "已关闭规划模式"),
            ("/rename 冒烟测试会话", "已重命名"),
            (
                "/goal Reply with the single word GOALDONE, then stop.",
                "GOALDONE",
            ),
            ("/goal clear", "目标已清除"),
            ("/review", ""),
            ("/compact", "压缩"),
        ] {
            let t = turn(
                &mut run,
                vec![PromptPart::Text(command.into())],
                None,
                false,
            );
            show(command, &t);
            check(
                command,
                t.end.starts_with("EndTurn") && t.reply.contains(expect),
            );
        }
        check(
            "title from /rename",
            run.title.as_deref() == Some("冒烟测试会话"),
        );
    }
    if want("pool") {
        let mut second = start(&preset, &ws, &pool, None);
        let t = turn(
            &mut second,
            vec![PromptPart::Text("Reply with exactly MANGO.".into())],
            None,
            false,
        );
        show("second session on the same process", &t);
        check("second session", t.reply.contains("MANGO"));
        check("same process", second.client.pid() == run.client.pid());
        let t = turn(
            &mut run,
            vec![PromptPart::Text("Reply with exactly KIWI.".into())],
            None,
            false,
        );
        check("first session still answers", t.reply.contains("KIWI"));
        if let Some(pid) = run.client.pid() {
            println!("  footprint with two sessions: {}", footprint(pid));
        }
        second.client.shutdown();
    }
    if want("resume") {
        let session = run.session.clone();
        run.client.shutdown();
        std::thread::sleep(Duration::from_secs(2));
        let mut again = start(&preset, &ws, &pool, session);
        let t = turn(
            &mut again,
            vec![PromptPart::Text(
                "Which fruit word did I ask you to reply with at the very start of this conversation? One word.".into(),
            )],
            None,
            false,
        );
        show("resume after restart", &t);
        check("remembers", t.reply.to_uppercase().contains("PINEAPPLE"));
        again.client.shutdown();
    } else {
        run.client.shutdown();
    }
    if want("text") {
        let mut o = ClientOptions::new(preset.clone(), &ws);
        o.pool = Some(pool.clone());
        let answer = async_io::block_on(generate_text(
            o,
            "Write a one-line commit message for adding hello.txt. Reply with the message only."
                .into(),
        ));
        println!("\n### commit message: {answer:?}");
        check("commit message", answer.is_ok_and(|a| !a.is_empty()));
    }
    pool.shutdown();
}
