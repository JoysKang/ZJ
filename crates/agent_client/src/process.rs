//! Agent child process: sanitized environment, its own process group, bounded stderr tail,
//! group-wide termination.

use crate::registry::ResolvedLaunch;
use std::{
    collections::VecDeque,
    env,
    ffi::OsStr,
    io::{self, Read},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Last bytes of stderr kept for crash reports. Shown to the user, never logged.
pub(crate) const STDERR_TAIL: usize = 16 * 1024;
const TERM_GRACE: Duration = Duration::from_millis(1500);

/// Variables that would make the agent misbehave when ZJ itself was started from a shell
/// inside a repository, or from inside another agent.
fn drop_inherited(key: &OsStr) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    key.starts_with("GIT_")
        || key.starts_with("ZJ_")
        || matches!(
            key,
            // Claude Code refuses to start "inside another Claude Code session".
            "CLAUDECODE" | "CLAUDE_CODE_ENTRYPOINT" | "CLAUDE_CODE_SSE_PORT"
        )
}

pub(crate) struct AgentProcess {
    child: Child,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    reaped: Option<ExitStatus>,
    /// Set once the group has been killed: a second `terminate` (Drop after an explicit one)
    /// must not signal a process group ID that may since have been reused.
    terminated: bool,
}

impl AgentProcess {
    pub fn spawn(launch: &ResolvedLaunch, cwd: &Path) -> io::Result<Self> {
        let mut command = Command::new(&launch.program);
        for (key, _) in env::vars_os() {
            if drop_inherited(&key) {
                command.env_remove(key);
            }
        }
        command
            .args(&launch.args)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
        if let Some(mut stderr) = child.stderr.take() {
            let tail = tail.clone();
            thread::Builder::new()
                .name("agent-stderr".into())
                .spawn(move || {
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = stderr.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        let mut tail = tail.lock().unwrap();
                        tail.extend(&buf[..n]);
                        let excess = tail.len().saturating_sub(STDERR_TAIL);
                        tail.drain(..excess);
                    }
                })?;
        }
        Ok(Self {
            child,
            stdin,
            stdout,
            stderr: tail,
            reaped: None,
            terminated: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr_tail(&self) -> String {
        let tail = self.stderr.lock().unwrap();
        let (a, b) = tail.as_slices();
        let mut bytes = a.to_vec();
        bytes.extend_from_slice(b);
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn try_status(&mut self) -> Option<ExitStatus> {
        if self.reaped.is_none() {
            self.reaped = self.child.try_wait().ok().flatten();
        }
        self.reaped
    }

    fn signal_group(&self, signal: i32) {
        let pgid = self.child.id() as i32;
        // SAFETY: process_group(0) made the child the leader of its own group, so -pgid only
        // reaches the agent and its descendants; kill() has no memory-safety preconditions.
        unsafe {
            libc::kill(-pgid, signal);
        }
    }

    /// SIGTERM to the whole group, SIGKILL after a grace period; reaps the leader.
    pub fn terminate(&mut self) -> Option<ExitStatus> {
        if self.terminated {
            return self.reaped;
        }
        self.terminated = true;
        // Closing stdin first lets well-behaved agents exit on EOF.
        self.stdin.take();
        if self.try_status().is_none() {
            self.signal_group(libc::SIGTERM);
            let deadline = Instant::now() + TERM_GRACE;
            while self.try_status().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
        }
        // Descendants may outlive the leader; the group id stays valid while any remain.
        self.signal_group(libc::SIGKILL);
        if self.reaped.is_none() {
            self.reaped = self.child.wait().ok();
        }
        self.reaped
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        // Even after the leader exited on its own, clear out any descendants it left.
        self.terminate();
    }
}

/// `code=…` / `signal=…` for logs and messages.
pub(crate) fn describe(status: Option<ExitStatus>) -> (Option<i32>, Option<i32>) {
    match status {
        Some(s) => (s.code(), s.signal()),
        None => (None, None),
    }
}
