//! ACP agents in front of the Claude Code and Codex CLIs, so ZJ needs neither Node.js nor npm
//! adapters (docs/adr/0009). ZJ's own executable runs them in a helper mode:
//!
//! `workspace-editor --agent-bridge claude|codex <cli> [--login <args>…]`
//!
//! - [`claude`]: one `claude -p --input-format stream-json` per session.
//! - [`codex`]: one `codex app-server` shared by every session (threads).
//!
//! `agent_client` starts the helper like any ACP agent and speaks ACP v1 over its stdio. With
//! `--login` the helper runs the CLI's own sign-in instead (Terminal.app, see
//! `agent_client::login`).
//!
//! Logging follows the workspace rule (`event=… key=value` on stderr), never message or file
//! content. The CLIs' stderr is passed through for ZJ's crash card.

mod acp;
mod child;
mod claude;
mod codex;
mod unified;

use serde_json::Value;
use std::{
    io::BufRead,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread,
};

/// The argument that turns ZJ's executable into the bridge.
pub const FLAG: &str = "--agent-bridge";

/// What the bridge's single loop handles, in arrival order.
pub(crate) enum Event {
    /// A JSON-RPC message from ZJ.
    Client(Value),
    /// ZJ closed our stdin: stop.
    ClientGone,
    /// A line from the CLI started under `key`; `None` when its output ended.
    Cli(u64, Option<Value>),
}

/// Runs the bridge with the arguments after [`FLAG`]; the process exit code.
pub fn run(args: &[String]) -> i32 {
    let [kind, cli, rest @ ..] = args else {
        eprintln!("usage: --agent-bridge claude|codex <cli> [--login <args>…]");
        return 2;
    };
    let cli = PathBuf::from(cli);
    if let Some(("--login", login)) = rest.split_first().map(|(a, r)| (a.as_str(), r)) {
        return child::login(kind, &cli, login);
    }
    let (tx, rx) = mpsc::channel();
    read_stdin(tx.clone());
    match kind.as_str() {
        "claude" => claude::serve(cli, tx, rx),
        "codex" => codex::serve(cli, tx, rx),
        _ => {
            eprintln!("event=agent_bridge_unknown_agent");
            2
        }
    }
}

/// ZJ's messages, one JSON object per line.
fn read_stdin(tx: Sender<Event>) {
    let spawned = thread::Builder::new()
        .name("agent-bridge-stdin".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let Ok(message) = serde_json::from_str(line.trim()) else {
                    continue;
                };
                if tx.send(Event::Client(message)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Event::ClientGone);
        });
    if spawned.is_err() {
        eprintln!("event=agent_bridge_thread_failed");
        std::process::exit(1);
    }
}
