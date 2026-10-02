//! ACP (Agent Client Protocol) client for ZJ's agent panel. No GPUI.
//!
//! - [`registry`]: built-in presets (Claude Code, Codex, Gemini CLI, Claude Code · DeepSeek)
//!   and user-defined agents; launch resolution with friendly errors (Node.js missing, …).
//! - [`AgentClient`]: one supervisor thread per agent. The process starts on first use in its
//!   own process group with a sanitized environment, speaks ACP v1 over stdio, streams typed
//!   [`AgentEvent`]s, stops after an idle timeout and restarts on the next prompt (restoring
//!   the session with `session/load` when the agent supports it).
//! - Client capabilities: `fs/read_text_file` (open buffers first, then disk) and
//!   `fs/write_text_file` ([`WriteMode::Direct`] or [`WriteMode::AcceptFirst`] through the
//!   [`ShadowStore`]). `terminal/*` is not advertised in v1.
//!
//! Logging follows the workspace rule: `event=… key=value`, never message content, file
//! content, environment values or the agent's stderr.

mod client;
mod events;
pub mod fs;
mod process;
pub mod registry;
pub mod shadow;

pub use client::{AgentClient, ClientError, ClientOptions, PromptPart, WriteMode};
pub use events::*;
pub use fs::BufferProvider;
pub use registry::{AgentPreset, Glyph, LaunchError, SearchPath, builtin_presets};
pub use shadow::{Hunk, PendingEdit, ShadowStore};
