//! Typed events for the UI, converted from ACP `session/update` and client requests. The UI
//! never sees ACP schema types, so protocol upgrades stay inside this crate.

use agent_client_protocol::schema::v1 as acp;
use std::path::PathBuf;

/// A prompt turn started with [`AgentClient::prompt`](crate::AgentClient::prompt).
pub type TurnId = u64;
/// A pending permission request, answered with
/// [`AgentClient::respond_permission`](crate::AgentClient::respond_permission).
pub type PermissionId = u64;

#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    /// The agent process was started (lazily, on the first prompt or `connect`).
    Starting {
        pid: u32,
    },
    Ready(AgentInfo),
    SessionStarted {
        session_id: String,
        /// `true` when an earlier session was restored with `session/load` after a restart.
        resumed: bool,
        modes: Option<Modes>,
    },
    /// The agent echoed a user message (outside of history replay).
    UserMessageChunk {
        text: String,
    },
    MessageChunk {
        text: String,
    },
    ThoughtChunk {
        text: String,
    },
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCallPatch),
    /// Full plan; replaces the previous one.
    Plan(Vec<PlanEntry>),
    AvailableCommands(Vec<AgentCommand>),
    ModeChanged {
        mode_id: String,
    },
    Usage {
        used: u64,
        size: u64,
        cost: Option<(f64, String)>,
    },
    TitleChanged {
        title: Option<String>,
    },
    PermissionRequested(PermissionRequest),
    /// The agent wrote a file on disk (the editor should reload it).
    FileWritten {
        path: PathBuf,
    },
    TurnEnded {
        turn: TurnId,
        outcome: TurnOutcome,
    },
    /// The agent process is gone. The next prompt restarts it.
    Exited {
        reason: ExitReason,
    },
    /// The agent needs a login before it can start a session. Answer with
    /// [`AgentClient::login`](crate::AgentClient::login) (and, for a terminal method, with
    /// [`AgentClient::retry_login`](crate::AgentClient::retry_login) once the user is done).
    /// A pending prompt waits and continues after the login.
    AuthRequired {
        methods: Vec<AuthChoice>,
    },
    /// A first-use install or login step; each message replaces the previous one.
    Progress {
        message: String,
    },
    /// Launch or protocol failure the user should see.
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentInfo {
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    pub load_session: bool,
    pub embedded_context: bool,
    pub image: bool,
    /// Authentication methods the agent advertises (login happens in the agent).
    pub auth_methods: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Runs in Terminal.app instead of through `authenticate`.
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Modes {
    pub current: String,
    pub available: Vec<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Text(String),
    /// An edit, summarized when the event is converted (off the UI thread): the texts can be
    /// whole files, and the panel only shows the path and the line counts.
    Diff {
        path: PathBuf,
        /// The agent created the file (no old text).
        new_file: bool,
        added: usize,
        removed: usize,
    },
    Terminal {
        id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub title: String,
    pub kind: ToolKind,
    pub status: ToolStatus,
    pub locations: Vec<Location>,
    pub content: Vec<ToolContent>,
}

/// Fields that changed; `None` means unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolCallPatch {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<ToolKind>,
    pub status: Option<ToolStatus>,
    pub locations: Option<Vec<Location>>,
    pub content: Option<Vec<ToolContent>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: PermissionKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionRequest {
    pub id: PermissionId,
    pub tool_call: ToolCallPatch,
    pub options: Vec<PermissionOption>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// Stopped after the idle timeout; restarts transparently.
    Idle,
    Shutdown,
    /// Handshake or session setup failed (already reported as an error); ZJ stopped it.
    SetupFailed {
        stderr_tail: String,
    },
    Crashed {
        code: Option<i32>,
        signal: Option<i32>,
        /// Last part of the agent's stderr, for the error card. Not logged.
        stderr_tail: String,
    },
}

/// Text for a content block; non-text blocks get a short placeholder.
pub(crate) fn content_text(block: &acp::ContentBlock) -> String {
    match block {
        acp::ContentBlock::Text(t) => t.text.clone(),
        acp::ContentBlock::Image(_) => "[图片]".into(),
        acp::ContentBlock::Audio(_) => "[音频]".into(),
        acp::ContentBlock::ResourceLink(link) => link.uri.clone(),
        acp::ContentBlock::Resource(res) => match &res.resource {
            acp::EmbeddedResourceResource::TextResourceContents(t) => t.text.clone(),
            _ => "[附件]".into(),
        },
        _ => String::new(),
    }
}

fn kind(kind: &acp::ToolKind) -> ToolKind {
    match kind {
        acp::ToolKind::Read => ToolKind::Read,
        acp::ToolKind::Edit => ToolKind::Edit,
        acp::ToolKind::Delete => ToolKind::Delete,
        acp::ToolKind::Move => ToolKind::Move,
        acp::ToolKind::Search => ToolKind::Search,
        acp::ToolKind::Execute => ToolKind::Execute,
        acp::ToolKind::Think => ToolKind::Think,
        acp::ToolKind::Fetch => ToolKind::Fetch,
        acp::ToolKind::SwitchMode => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

fn status(status: &acp::ToolCallStatus) -> ToolStatus {
    match status {
        acp::ToolCallStatus::Pending => ToolStatus::Pending,
        acp::ToolCallStatus::InProgress => ToolStatus::InProgress,
        acp::ToolCallStatus::Completed => ToolStatus::Completed,
        acp::ToolCallStatus::Failed => ToolStatus::Failed,
        _ => ToolStatus::Pending,
    }
}

fn locations(locations: &[acp::ToolCallLocation]) -> Vec<Location> {
    locations
        .iter()
        .map(|l| Location {
            path: l.path.clone(),
            line: l.line,
        })
        .collect()
}

fn contents(content: &[acp::ToolCallContent]) -> Vec<ToolContent> {
    content
        .iter()
        .map(|c| match c {
            acp::ToolCallContent::Content(c) => ToolContent::Text(content_text(&c.content)),
            acp::ToolCallContent::Diff(d) => {
                let (added, removed) =
                    crate::review::line_counts(d.old_text.as_deref().unwrap_or(""), &d.new_text);
                ToolContent::Diff {
                    path: d.path.clone(),
                    new_file: d.old_text.is_none(),
                    added,
                    removed,
                }
            }
            acp::ToolCallContent::Terminal(t) => ToolContent::Terminal {
                id: t.terminal_id.to_string(),
            },
            _ => ToolContent::Text(String::new()),
        })
        .collect()
}

pub(crate) fn tool_call(call: &acp::ToolCall) -> ToolCall {
    ToolCall {
        id: call.tool_call_id.to_string(),
        title: call.title.clone(),
        kind: kind(&call.kind),
        status: status(&call.status),
        locations: locations(&call.locations),
        content: contents(&call.content),
    }
}

pub(crate) fn tool_patch(update: &acp::ToolCallUpdate) -> ToolCallPatch {
    let f = &update.fields;
    ToolCallPatch {
        id: update.tool_call_id.to_string(),
        title: f.title.clone(),
        kind: f.kind.as_ref().map(kind),
        status: f.status.as_ref().map(status),
        locations: f.locations.as_deref().map(locations),
        content: f.content.as_deref().map(contents),
    }
}

/// `None` for updates the UI has no use for (or unknown future variants).
pub(crate) fn from_update(update: &acp::SessionUpdate) -> Option<AgentEvent> {
    Some(match update {
        acp::SessionUpdate::UserMessageChunk(c) => AgentEvent::UserMessageChunk {
            text: content_text(&c.content),
        },
        acp::SessionUpdate::AgentMessageChunk(c) => AgentEvent::MessageChunk {
            text: content_text(&c.content),
        },
        acp::SessionUpdate::AgentThoughtChunk(c) => AgentEvent::ThoughtChunk {
            text: content_text(&c.content),
        },
        acp::SessionUpdate::ToolCall(call) => AgentEvent::ToolCall(tool_call(call)),
        acp::SessionUpdate::ToolCallUpdate(u) => AgentEvent::ToolCallUpdate(tool_patch(u)),
        acp::SessionUpdate::Plan(plan) => AgentEvent::Plan(
            plan.entries
                .iter()
                .map(|e| PlanEntry {
                    content: e.content.clone(),
                    status: match e.status {
                        acp::PlanEntryStatus::InProgress => PlanStatus::InProgress,
                        acp::PlanEntryStatus::Completed => PlanStatus::Completed,
                        _ => PlanStatus::Pending,
                    },
                })
                .collect(),
        ),
        acp::SessionUpdate::AvailableCommandsUpdate(u) => AgentEvent::AvailableCommands(
            u.available_commands
                .iter()
                .map(|c| AgentCommand {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    input_hint: c.input.as_ref().and_then(|i| match i {
                        acp::AvailableCommandInput::Unstructured(u) => Some(u.hint.clone()),
                        _ => None,
                    }),
                })
                .collect(),
        ),
        acp::SessionUpdate::CurrentModeUpdate(u) => AgentEvent::ModeChanged {
            mode_id: u.current_mode_id.to_string(),
        },
        acp::SessionUpdate::UsageUpdate(u) => AgentEvent::Usage {
            used: u.used,
            size: u.size,
            cost: u.cost.as_ref().map(|c| (c.amount, c.currency.clone())),
        },
        acp::SessionUpdate::SessionInfoUpdate(u) => match &u.title {
            agent_client_protocol::schema::MaybeUndefined::Value(title) => {
                AgentEvent::TitleChanged {
                    title: Some(title.clone()),
                }
            }
            agent_client_protocol::schema::MaybeUndefined::Null => {
                AgentEvent::TitleChanged { title: None }
            }
            _ => return None,
        },
        _ => return None,
    })
}

pub(crate) fn permission_options(options: &[acp::PermissionOption]) -> Vec<PermissionOption> {
    options
        .iter()
        .map(|o| PermissionOption {
            id: o.option_id.to_string(),
            name: o.name.clone(),
            kind: match o.kind {
                acp::PermissionOptionKind::AllowOnce => PermissionKind::AllowOnce,
                acp::PermissionOptionKind::AllowAlways => PermissionKind::AllowAlways,
                acp::PermissionOptionKind::RejectAlways => PermissionKind::RejectAlways,
                _ => PermissionKind::RejectOnce,
            },
        })
        .collect()
}

pub(crate) fn turn_outcome(reason: &acp::StopReason) -> TurnOutcome {
    match reason {
        acp::StopReason::EndTurn => TurnOutcome::EndTurn,
        acp::StopReason::MaxTokens => TurnOutcome::MaxTokens,
        acp::StopReason::MaxTurnRequests => TurnOutcome::MaxTurnRequests,
        acp::StopReason::Refusal => TurnOutcome::Refusal,
        acp::StopReason::Cancelled => TurnOutcome::Cancelled,
        _ => TurnOutcome::EndTurn,
    }
}
