//! ACP (Agent Client Protocol) client for ZJ's agent panel. No GPUI.
//!
//! - [`registry`]: built-in presets (Claude Code, Codex, Claude Code · DeepSeek) and
//!   user-defined agents; launch resolution with friendly errors. [`provision`] installs npm
//!   adapters (and Node.js when needed) into ZJ's data directory on first use.
//! - [`AgentClient`]: one session. Its agent process starts on first use in its own process
//!   group with a sanitized environment and speaks ACP v1 over stdio; sessions from the same
//!   [`AgentPool`] with the same agent and variables share it. Each session streams typed
//!   [`AgentEvent`]s, is closed after an idle timeout (the process stops when none is left) and
//!   runs again on the next prompt (restored with `session/load` when the agent supports it).
//! - Client capabilities: `fs/read_text_file` (open buffers first, then disk) and
//!   `fs/write_text_file` (written to disk after a snapshot of the file as it was before the
//!   agent, for review; see [`review`]). `terminal/*` is not advertised in v1. When a session needs a login,
//!   [`AgentEvent::AuthRequired`] lists the agent's methods; `terminal` ones run in
//!   Terminal.app.
//!
//! Logging follows the workspace rule: `event=… key=value`, never message content, file
//! content, environment values or the agent's stderr.

mod client;
mod completion;
pub mod diff;
mod events;
pub mod fs;
mod host;
mod login;
mod process;
pub mod provision;
mod quota;
pub mod registry;
pub mod review;
pub mod thread;

pub use client::{AgentClient, ClientError, ClientOptions, PromptPart};
pub use completion::generate_text;
pub use diff::Hunk;
pub use events::*;
pub use fs::BufferProvider;
pub use host::AgentPool;
pub use quota::read_codex_quota;
pub use registry::{AgentPreset, Glyph, LaunchError, SearchPath, builtin_presets};
