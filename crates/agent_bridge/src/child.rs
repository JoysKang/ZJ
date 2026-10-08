//! The CLI as a child: JSON lines in and out. It stays in the bridge's process group, so
//! ZJ stopping the group stops it too.

use crate::Event;
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{self, ChildStdin, Command, Stdio},
    sync::mpsc::Sender,
    thread,
    time::{Duration, Instant},
};

/// How long a CLI gets to exit after its stdin closes.
const EXIT_GRACE: Duration = Duration::from_millis(1500);

pub(crate) struct Child {
    process: process::Child,
    stdin: Option<ChildStdin>,
}

impl Child {
    /// Starts `program`; its stdout lines arrive as `Event::Cli(key, …)`.
    pub fn spawn(
        program: &Path,
        args: &[String],
        cwd: &Path,
        key: u64,
        events: Sender<Event>,
    ) -> io::Result<Self> {
        let mut process = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Ours is ZJ's: it keeps the tail for the crash card.
            .stderr(Stdio::inherit())
            .spawn()?;
        let (Some(stdout), stdin) = (process.stdout.take(), process.stdin.take()) else {
            let _ = process.kill();
            let _ = process.wait();
            return Err(io::Error::other("CLI pipes missing"));
        };
        thread::Builder::new()
            .name("agent-bridge-cli".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                        continue;
                    };
                    if events.send(Event::Cli(key, Some(message))).is_err() {
                        return;
                    }
                }
                let _ = events.send(Event::Cli(key, None));
            })?;
        Ok(Self { process, stdin })
    }

    /// False when the CLI is gone.
    pub fn send(&mut self, message: &Value) -> bool {
        let Some(stdin) = &mut self.stdin else {
            return false;
        };
        let mut line = message.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_ok()
    }

    /// Closes its stdin and waits briefly for it to exit, then kills it.
    fn stop(&mut self) {
        self.stdin.take();
        let started = Instant::now();
        while started.elapsed() < EXIT_GRACE {
            match self.process.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => thread::sleep(Duration::from_millis(20)),
            }
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `--login`: the CLI's own interactive sign-in, replacing this process (Terminal.app runs it).
pub(crate) fn login(kind: &str, cli: &Path, args: &[String]) -> i32 {
    let mut command = Command::new(cli);
    match kind {
        "claude" => command.args(["auth", "login"]),
        "codex" => command.arg("login"),
        _ => return 2,
    };
    let error = command.args(args).exec();
    eprintln!("无法启动 {}：{error}", cli.display());
    1
}
