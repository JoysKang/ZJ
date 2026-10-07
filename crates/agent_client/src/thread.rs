//! The conversation as the panel shows it, built from [`AgentEvent`]s. No GPUI: the app keeps
//! one [`Thread`] per session and renders its items.
//!
//! Bounded: at most [`MAX_ITEMS`] items stay in memory; older ones are dropped from the front
//! (`dropped` counts them) and come back from the history database on demand.

use crate::events::{
    AgentCommand, AgentEvent, AuthChoice, ConfigOption, ExitReason, Modes, PermissionId,
    PermissionKind, PermissionRequest, PlanEntry, ToolCall, ToolCallPatch, ToolContent, ToolKind,
    ToolStatus, TurnId, TurnOutcome,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
};

pub const MAX_ITEMS: usize = 400;
/// Tool output kept per card (the agent already has the full text).
const MAX_TOOL_TEXT: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Running,
    /// A permission request is waiting for the user.
    Awaiting,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCard {
    pub call: ToolCall,
    /// Lines added / removed by the call's diffs.
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PermissionState {
    Pending,
    /// Answered with this option (its kind decides the card's color).
    Answered(PermissionKind, String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PermissionCard {
    pub request: PermissionRequest,
    pub state: PermissionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginState {
    Pending,
    Done,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoginCard {
    pub methods: Vec<AuthChoice>,
    pub state: LoginState,
    /// The agent still asked for a login after a retry.
    pub retried: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    User {
        text: String,
        /// File and selection chips shown above the message.
        attachments: Vec<String>,
    },
    /// Markdown; `streaming` while chunks still arrive.
    Agent {
        text: String,
        streaming: bool,
    },
    Thought {
        text: String,
        streaming: bool,
    },
    Tool(ToolCard),
    Plan(Vec<PlanEntry>),
    Permission(PermissionCard),
    Login(LoginCard),
    Notice {
        text: String,
        error: bool,
    },
}

/// A file the agent changed, reviewed against the snapshot from before the agent.
#[derive(Clone, Debug, PartialEq)]
pub struct FileChange {
    pub added: usize,
    pub removed: usize,
    pub new_file: bool,
}

/// Something the caller persists to the history database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Agent(String),
    Tool(String),
}

#[derive(Default)]
pub struct Thread {
    pub items: VecDeque<Item>,
    /// Absolute turn starts, including the start of a turn truncated at the front.
    /// A steering prompt stays in the current turn.
    pub turn_starts: BTreeSet<usize>,
    /// Older items not held in memory.
    pub dropped: usize,
    pub status: Status,
    pub title: Option<String>,
    pub session_id: Option<String>,
    pub modes: Option<Modes>,
    pub commands: Vec<AgentCommand>,
    /// Model, effort and the like (see [`ConfigOption`]).
    pub configs: Vec<ConfigOption>,
    /// (used, size) tokens of the context window.
    pub usage: Option<(u64, u64)>,
    pub changed_files: BTreeMap<PathBuf, FileChange>,
    pub turn: Option<TurnId>,
    /// A turn ended while the user was looking elsewhere.
    pub unread: bool,
    pub last_error: Option<String>,
    /// Bumped on every change; renderers cache per version.
    pub version: u64,
    /// Items appended since the last [`Thread::take_records`] that should be persisted.
    records: Vec<Record>,
    /// The last item is an install progress notice, replaced by the next one.
    progress_shown: bool,
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_TOOL_TEXT {
        let mut cut = MAX_TOOL_TEXT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

fn clip_content(content: Vec<ToolContent>) -> Vec<ToolContent> {
    content
        .into_iter()
        .map(|c| match c {
            ToolContent::Text(t) => ToolContent::Text(clip(t)),
            other => other,
        })
        .collect()
}

impl ToolCard {
    fn new(call: ToolCall) -> Self {
        let mut card = Self {
            call,
            added: 0,
            removed: 0,
        };
        card.call.content = clip_content(std::mem::take(&mut card.call.content));
        card.recount();
        card
    }

    fn recount(&mut self) {
        let (mut added, mut removed) = (0, 0);
        for content in &self.call.content {
            if let ToolContent::Diff {
                added: a,
                removed: r,
                ..
            } = content
            {
                added += a;
                removed += r;
            }
        }
        self.added = added;
        self.removed = removed;
    }

    fn patch(&mut self, patch: &ToolCallPatch) {
        if let Some(title) = &patch.title {
            self.call.title = title.clone();
        }
        if let Some(kind) = patch.kind {
            self.call.kind = kind;
        }
        if let Some(status) = patch.status {
            self.call.status = status;
        }
        if let Some(locations) = &patch.locations {
            self.call.locations = locations.clone();
        }
        if let Some(content) = &patch.content {
            self.call.content = clip_content(content.clone());
            self.recount();
        }
    }
}

/// The command a permission request is about: its title without decoration (Claude puts
/// commands in backticks).
pub fn permission_command(request: &PermissionRequest) -> Option<String> {
    let title = request.tool_call.title.as_deref()?.trim();
    let title = title.trim_matches('`').trim();
    (!title.is_empty()).then(|| title.to_string())
}

impl Thread {
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, item: Item) {
        self.progress_shown = false;
        self.items.push_back(item);
        while self.items.len() > MAX_ITEMS {
            self.items.pop_front();
            self.dropped += 1;
            let first = self.turn_starts.range(..=self.dropped).next_back().copied();
            self.turn_starts
                .retain(|start| *start >= self.dropped || Some(*start) == first);
        }
    }

    fn pending_login(&mut self) -> Option<&mut LoginCard> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Login(card) if card.state == LoginState::Pending => Some(card),
            _ => None,
        })
    }

    fn close_login(&mut self, state: LoginState) -> bool {
        match self.pending_login() {
            Some(card) => {
                card.state = state;
                true
            }
            None => false,
        }
    }

    fn end_streaming(&mut self) {
        for item in self.items.iter_mut().rev().take(4) {
            match item {
                Item::Agent { streaming, text } if *streaming => {
                    *streaming = false;
                    self.records.push(Record::Agent(text.clone()));
                }
                Item::Thought { streaming, .. } => *streaming = false,
                _ => {}
            }
        }
    }

    /// The user sent a prompt; the panel adds it before the agent answers.
    pub fn push_user(&mut self, text: String, attachments: Vec<String>, turn: TurnId) {
        self.end_streaming();
        self.push(Item::User { text, attachments });
        if self.turn != Some(turn) {
            self.turn_starts.insert(self.dropped + self.items.len() - 1);
            self.turn = Some(turn);
            self.status = Status::Running;
        }
        self.last_error = None;
        self.version += 1;
    }

    pub fn push_notice(&mut self, text: impl Into<String>, error: bool) {
        self.push(Item::Notice {
            text: text.into(),
            error,
        });
        self.version += 1;
    }

    /// Restores items from the history database (oldest first), replacing what is shown.
    /// Stored messages have no turn ids, so historical user prompts define turn boundaries.
    pub fn load(&mut self, items: Vec<Item>, older: usize) {
        self.items.clear();
        self.turn_starts.clear();
        self.dropped = older;
        for item in items {
            if matches!(item, Item::User { .. }) {
                self.turn_starts.insert(self.dropped + self.items.len());
            }
            self.push(item);
        }
        self.version += 1;
    }

    /// Older items fetched from history go in front.
    pub fn prepend(&mut self, items: Vec<Item>) {
        let n = items.len();
        let from = self.dropped.saturating_sub(n);
        self.turn_starts.extend(
            items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| matches!(item, Item::User { .. }).then_some(from + i)),
        );
        for item in items.into_iter().rev() {
            self.items.push_front(item);
        }
        self.dropped = from;
        while self.items.len() > MAX_ITEMS {
            self.items.pop_back();
        }
        self.turn_starts
            .retain(|i| *i < self.dropped + self.items.len());
        self.version += 1;
    }

    pub fn take_records(&mut self) -> Vec<Record> {
        std::mem::take(&mut self.records)
    }

    pub fn pending_permissions(&self) -> impl Iterator<Item = &PermissionCard> {
        self.items.iter().filter_map(|item| match item {
            Item::Permission(card) if card.state == PermissionState::Pending => Some(card),
            _ => None,
        })
    }

    /// Marks a permission request as answered; returns `false` if it was not pending.
    pub fn answer_permission(&mut self, id: PermissionId, state: PermissionState) -> bool {
        let mut found = false;
        for item in self.items.iter_mut().rev() {
            if let Item::Permission(card) = item
                && card.request.id == id
                && card.state == PermissionState::Pending
            {
                card.state = state.clone();
                found = true;
                break;
            }
        }
        if found {
            if self.pending_permissions().next().is_none() && self.status == Status::Awaiting {
                self.status = Status::Running;
            }
            self.version += 1;
        }
        found
    }

    fn cancel_permissions(&mut self) {
        for item in self.items.iter_mut() {
            if let Item::Permission(card) = item
                && card.state == PermissionState::Pending
            {
                card.state = PermissionState::Cancelled;
            }
        }
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCard> {
        self.items.iter_mut().rev().find_map(|item| match item {
            Item::Tool(card) if card.call.id == id => Some(card),
            _ => None,
        })
    }

    fn note_tool_files(&mut self, card: &ToolCard) {
        if card.call.kind != ToolKind::Edit {
            return;
        }
        // One call can edit several files, each in several hunks: every file gets its own.
        let mut files: BTreeMap<&PathBuf, FileChange> = BTreeMap::new();
        for content in &card.call.content {
            if let ToolContent::Diff {
                path,
                new_file,
                added,
                removed,
            } = content
            {
                let file = files.entry(path).or_insert(FileChange {
                    added: 0,
                    removed: 0,
                    new_file: *new_file,
                });
                file.added += added;
                file.removed += removed;
            }
        }
        for (path, change) in files {
            let entry = self
                .changed_files
                .entry(path.clone())
                .or_insert(FileChange {
                    added: 0,
                    removed: 0,
                    new_file: change.new_file,
                });
            // Until the app recomputes against the snapshot, show the call's own counts.
            if entry.added == 0 && entry.removed == 0 {
                entry.added = change.added;
                entry.removed = change.removed;
            }
        }
    }

    /// Replaces a changed file's counts (computed by the app against the snapshot); `None` removes the file from the summary (accepted, rejected or reverted).
    pub fn set_file_change(&mut self, path: PathBuf, change: Option<FileChange>) {
        match change {
            Some(change) => {
                self.changed_files.insert(path, change);
            }
            None => {
                self.changed_files.remove(&path);
            }
        }
        self.version += 1;
    }

    /// Applies one event. `visible` tells whether the user is looking at this thread (a turn
    /// that ends while it is hidden leaves it unread).
    pub fn apply(&mut self, event: &AgentEvent, visible: bool) {
        self.version += 1;
        // A login asked for by `session/prompt` ends without `SessionStarted`: the resent
        // prompt producing output is the sign it worked.
        if matches!(
            event,
            AgentEvent::MessageChunk { .. }
                | AgentEvent::ThoughtChunk { .. }
                | AgentEvent::ToolCall(_)
                | AgentEvent::Plan(_)
                | AgentEvent::PermissionRequested(_)
        ) && self.close_login(LoginState::Done)
        {
            self.status = Status::Running;
        }
        match event {
            AgentEvent::Starting { .. } | AgentEvent::Ready(_) => {}
            AgentEvent::SessionStarted {
                session_id,
                resumed,
                modes,
            } => {
                let changed = self
                    .session_id
                    .as_ref()
                    .is_some_and(|old| old != session_id);
                self.session_id = Some(session_id.clone());
                self.modes = modes.clone();
                if self.close_login(LoginState::Done) {
                    self.status = if self.turn.is_some() {
                        Status::Running
                    } else {
                        Status::Idle
                    };
                }
                if changed && !resumed {
                    self.push(Item::Notice {
                        text:
                            "Agent 重启后无法恢复原会话，已开始新会话（之前的上下文不在 Agent 里）"
                                .into(),
                        error: false,
                    });
                }
            }
            AgentEvent::UserMessageChunk { .. } => {}
            AgentEvent::MessageChunk { text } => match self.items.back_mut() {
                Some(Item::Agent {
                    text: last,
                    streaming: true,
                }) => last.push_str(text),
                _ => {
                    self.end_streaming();
                    self.push(Item::Agent {
                        text: text.clone(),
                        streaming: true,
                    });
                }
            },
            AgentEvent::ThoughtChunk { text } => match self.items.back_mut() {
                Some(Item::Thought {
                    text: last,
                    streaming: true,
                }) => last.push_str(text),
                _ => {
                    self.end_streaming();
                    self.push(Item::Thought {
                        text: text.clone(),
                        streaming: true,
                    });
                }
            },
            AgentEvent::ToolCall(call) => {
                self.end_streaming();
                let card = ToolCard::new(call.clone());
                self.note_tool_files(&card);
                self.records.push(Record::Tool(card.call.title.clone()));
                self.push(Item::Tool(card));
            }
            AgentEvent::ToolCallUpdate(patch) => {
                let mut updated = None;
                if let Some(card) = self.tool_mut(&patch.id) {
                    card.patch(patch);
                    updated = Some(card.clone());
                }
                if let Some(card) = updated {
                    self.note_tool_files(&card);
                }
            }
            AgentEvent::Plan(entries) => {
                // One plan per turn, updated in place.
                let since_user = self
                    .items
                    .iter()
                    .rposition(|item| matches!(item, Item::User { .. }))
                    .map_or(0, |i| i + 1);
                let existing = (since_user..self.items.len())
                    .rev()
                    .find(|&i| matches!(self.items[i], Item::Plan(_)));
                match existing {
                    Some(i) => self.items[i] = Item::Plan(entries.clone()),
                    None => {
                        self.end_streaming();
                        self.push(Item::Plan(entries.clone()));
                    }
                }
            }
            AgentEvent::AvailableCommands(commands) => self.commands = commands.clone(),
            AgentEvent::ConfigOptions(configs) => self.configs = configs.clone(),
            AgentEvent::ModeChanged { mode_id } => {
                if let Some(modes) = &mut self.modes {
                    modes.current = mode_id.clone();
                }
            }
            AgentEvent::Usage { used, size, .. } => self.usage = Some((*used, *size)),
            AgentEvent::TitleChanged { title } => self.title = title.clone(),
            AgentEvent::PermissionRequested(request) => {
                self.end_streaming();
                self.push(Item::Permission(PermissionCard {
                    request: request.clone(),
                    state: PermissionState::Pending,
                }));
                self.status = Status::Awaiting;
            }
            AgentEvent::FileWritten { path } => {
                self.changed_files
                    .entry(path.clone())
                    .or_insert(FileChange {
                        added: 0,
                        removed: 0,
                        new_file: false,
                    });
            }
            AgentEvent::TurnEnded { turn, outcome } => {
                if self.turn != Some(*turn) {
                    return;
                }
                self.turn = None;
                self.end_streaming();
                self.cancel_permissions();
                self.close_login(match outcome {
                    TurnOutcome::Cancelled | TurnOutcome::Failed(_) => LoginState::Cancelled,
                    _ => LoginState::Done,
                });
                for item in self.items.iter_mut() {
                    if let Item::Tool(card) = item
                        && matches!(
                            card.call.status,
                            ToolStatus::Pending | ToolStatus::InProgress
                        )
                        && !matches!(outcome, TurnOutcome::EndTurn)
                    {
                        card.call.status = ToolStatus::Failed;
                    }
                }
                self.status = match outcome {
                    TurnOutcome::Failed(message) => {
                        self.last_error = Some(message.clone());
                        self.push(Item::Notice {
                            text: message.clone(),
                            error: true,
                        });
                        Status::Error
                    }
                    TurnOutcome::Cancelled => {
                        self.push(Item::Notice {
                            text: "已中断".into(),
                            error: false,
                        });
                        Status::Idle
                    }
                    TurnOutcome::MaxTokens | TurnOutcome::MaxTurnRequests => {
                        self.push(Item::Notice {
                            text: "Agent 达到了长度或轮次上限，回复可能不完整".into(),
                            error: false,
                        });
                        Status::Idle
                    }
                    TurnOutcome::Refusal => {
                        self.push(Item::Notice {
                            text: "Agent 拒绝了这个请求".into(),
                            error: true,
                        });
                        Status::Idle
                    }
                    TurnOutcome::EndTurn => Status::Idle,
                };
                if !visible {
                    self.unread = true;
                }
            }
            AgentEvent::AuthRequired { methods } => {
                self.end_streaming();
                match self.pending_login() {
                    Some(card) => {
                        card.methods = methods.clone();
                        card.retried = true;
                    }
                    None => self.push(Item::Login(LoginCard {
                        methods: methods.clone(),
                        state: LoginState::Pending,
                        retried: false,
                    })),
                }
                self.status = Status::Awaiting;
            }
            AgentEvent::Exited { reason } => {
                if self.close_login(LoginState::Cancelled) && self.turn.is_none() {
                    self.status = Status::Idle;
                }
                if let ExitReason::Crashed { stderr_tail, .. } = reason {
                    let tail: String = stderr_tail
                        .lines()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n");
                    let text = if tail.is_empty() {
                        "Agent 进程意外退出，下次发送时会重新启动".to_string()
                    } else {
                        format!("Agent 进程意外退出，下次发送时会重新启动：\n{tail}")
                    };
                    self.push(Item::Notice { text, error: true });
                    self.status = Status::Error;
                }
            }
            AgentEvent::Progress { message } => {
                if self.progress_shown
                    && let Some(Item::Notice { text, .. }) = self.items.back_mut()
                {
                    *text = message.clone();
                } else {
                    self.push(Item::Notice {
                        text: message.clone(),
                        error: false,
                    });
                    self.progress_shown = true;
                }
            }
            AgentEvent::Error { message } => {
                self.last_error = Some(message.clone());
                self.push(Item::Notice {
                    text: message.clone(),
                    error: true,
                });
            }
        }
    }

    /// Whether the thread has something worth a badge: running, waiting or unread.
    pub fn is_active(&self) -> bool {
        matches!(self.status, Status::Running | Status::Awaiting) || self.unread
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{Location, PermissionOption, PlanStatus};

    fn request(id: u64, title: &str, kind: ToolKind) -> PermissionRequest {
        PermissionRequest {
            id,
            tool_call: ToolCallPatch {
                id: format!("t{id}"),
                title: Some(title.into()),
                kind: Some(kind),
                ..Default::default()
            },
            options: vec![PermissionOption {
                id: "allow".into(),
                name: "Allow".into(),
                kind: PermissionKind::AllowOnce,
            }],
        }
    }

    #[test]
    fn turn_starts_keep_steering_in_one_turn_and_remain_bounded() {
        let mut thread = Thread::new();
        thread.push_user("task".into(), vec![], 1);
        thread.push_notice("detail", false);
        thread.push_user("steering".into(), vec![], 1);
        assert_eq!(thread.turn_starts.iter().copied().collect::<Vec<_>>(), [0]);
        thread.push_user("next task".into(), vec![], 2);
        assert_eq!(
            thread.turn_starts.iter().copied().collect::<Vec<_>>(),
            [0, 3]
        );
        for _ in 0..MAX_ITEMS {
            thread.push_notice("detail", false);
        }
        assert_eq!(thread.items.len(), MAX_ITEMS);
        assert_eq!(thread.turn_starts.iter().copied().collect::<Vec<_>>(), [3]);
        thread.push_user("another task".into(), vec![], 3);
        assert_eq!(
            thread.turn_starts.iter().copied().collect::<Vec<_>>(),
            [3, thread.dropped + MAX_ITEMS - 1]
        );
    }

    #[test]
    fn prepending_history_joins_a_turn_cut_at_the_page_boundary() {
        let user = |text: &str| Item::User {
            text: text.into(),
            attachments: vec![],
        };
        let reply = |text: &str| Item::Agent {
            text: text.into(),
            streaming: false,
        };
        let mut thread = Thread::new();
        thread.load(vec![reply("tail"), user("next"), reply("final")], 3);
        thread.prepend(vec![user("first"), reply("progress"), reply("progress 2")]);
        assert_eq!(
            thread.turn_starts.iter().copied().collect::<Vec<_>>(),
            [0, 4]
        );
        assert_eq!(thread.dropped, 0);
    }

    #[test]
    fn login_cards_wait_then_close() {
        let mut t = Thread::new();
        t.push_user("hi".into(), vec![], 1);
        let required = AgentEvent::AuthRequired {
            methods: vec![AuthChoice {
                id: "chat-gpt".into(),
                name: "ChatGPT".into(),
                description: None,
                terminal: false,
            }],
        };
        t.apply(&required, true);
        assert_eq!(t.status, Status::Awaiting);
        t.apply(&required, true);
        let logins = |t: &Thread| {
            t.items
                .iter()
                .filter_map(|i| match i {
                    Item::Login(card) => Some(card.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(logins(&t).len(), 1);
        assert!(logins(&t)[0].retried);
        t.apply(
            &AgentEvent::SessionStarted {
                session_id: "s".into(),
                resumed: false,
                modes: None,
            },
            true,
        );
        assert_eq!(logins(&t)[0].state, LoginState::Done);
        assert_eq!(t.status, Status::Running);

        t.apply(&required, true);
        t.apply(
            &AgentEvent::TurnEnded {
                turn: 1,
                outcome: TurnOutcome::Cancelled,
            },
            true,
        );
        assert_eq!(logins(&t)[1].state, LoginState::Cancelled);
        assert_eq!(t.status, Status::Idle);

        // Asked for by the prompt: the resent prompt's output closes the card.
        t.push_user("again".into(), vec![], 2);
        t.apply(&required, true);
        t.apply(&AgentEvent::MessageChunk { text: "ok".into() }, true);
        assert_eq!(logins(&t)[2].state, LoginState::Done);
        assert_eq!(t.status, Status::Running);
    }

    #[test]
    fn install_progress_replaces_itself() {
        let mut t = Thread::new();
        t.push_user("hi".into(), vec![], 1);
        for step in ["下载 Node.js", "安装适配器"] {
            t.apply(
                &AgentEvent::Progress {
                    message: step.into(),
                },
                true,
            );
        }
        assert_eq!(t.items.len(), 2);
        assert!(matches!(&t.items[1], Item::Notice { text, error: false } if text == "安装适配器"));
        t.apply(&AgentEvent::MessageChunk { text: "ok".into() }, true);
        t.apply(
            &AgentEvent::Progress {
                message: "again".into(),
            },
            true,
        );
        assert_eq!(t.items.len(), 4);
    }

    #[test]
    fn chunks_merge_and_tools_split_messages() {
        let mut t = Thread::new();
        t.push_user("hi".into(), vec![], 1);
        for chunk in ["Hel", "lo"] {
            t.apply(&AgentEvent::ThoughtChunk { text: chunk.into() }, true);
        }
        for chunk in ["原因", "在这里"] {
            t.apply(&AgentEvent::MessageChunk { text: chunk.into() }, true);
        }
        t.apply(
            &AgentEvent::ToolCall(ToolCall {
                id: "t1".into(),
                title: "Edit a.rs".into(),
                kind: ToolKind::Edit,
                status: ToolStatus::Pending,
                locations: vec![Location {
                    path: "/w/a.rs".into(),
                    line: Some(1),
                }],
                content: vec![ToolContent::Diff {
                    path: "/w/a.rs".into(),
                    new_file: false,
                    added: 2,
                    removed: 1,
                }],
            }),
            true,
        );
        t.apply(
            &AgentEvent::MessageChunk {
                text: "完成".into(),
            },
            true,
        );
        t.apply(
            &AgentEvent::ToolCallUpdate(ToolCallPatch {
                id: "t1".into(),
                status: Some(ToolStatus::Completed),
                ..Default::default()
            }),
            true,
        );
        t.apply(
            &AgentEvent::TurnEnded {
                turn: 1,
                outcome: TurnOutcome::EndTurn,
            },
            false,
        );
        let kinds: Vec<&str> = t
            .items
            .iter()
            .map(|i| match i {
                Item::User { .. } => "user",
                Item::Thought { .. } => "thought",
                Item::Agent { .. } => "agent",
                Item::Tool(_) => "tool",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["user", "thought", "agent", "tool", "agent"]);
        assert!(
            matches!(&t.items[2], Item::Agent { text, streaming: false } if text == "原因在这里")
        );
        match &t.items[3] {
            Item::Tool(card) => {
                assert_eq!(card.call.status, ToolStatus::Completed);
                assert_eq!((card.added, card.removed), (2, 1));
            }
            other => panic!("{other:?}"),
        }
        let change = &t.changed_files[&PathBuf::from("/w/a.rs")];
        assert_eq!((change.added, change.removed), (2, 1));
        assert_eq!(
            t.take_records(),
            vec![
                Record::Agent("原因在这里".into()),
                Record::Tool("Edit a.rs".into()),
                Record::Agent("完成".into())
            ]
        );
        assert!(t.unread);
        assert_eq!(t.status, Status::Idle);
    }

    #[test]
    fn one_call_editing_several_files_counts_each_on_its_own() {
        let mut t = Thread::new();
        t.push_user("go".into(), vec![], 1);
        let diff = |path: &str, new_file, added, removed| ToolContent::Diff {
            path: PathBuf::from(path),
            new_file,
            added,
            removed,
        };
        let call = ToolCall {
            id: "p1".into(),
            title: "Editing files".into(),
            kind: ToolKind::Edit,
            status: ToolStatus::Completed,
            locations: vec![],
            // a.rs in two hunks, b.rs created.
            content: vec![
                diff("/w/a.rs", false, 2, 1),
                diff("/w/b.rs", true, 5, 0),
                diff("/w/a.rs", false, 1, 1),
            ],
        };
        t.apply(&AgentEvent::ToolCall(call), true);
        let count = |path: &str| {
            let c = &t.changed_files[&PathBuf::from(path)];
            (c.added, c.removed, c.new_file)
        };
        assert_eq!(count("/w/a.rs"), (3, 2, false));
        assert_eq!(count("/w/b.rs"), (5, 0, true));
    }

    #[test]
    fn plans_update_in_place_and_permissions_track_status() {
        let mut t = Thread::new();
        t.push_user("go".into(), vec![], 7);
        let plan = |status| {
            AgentEvent::Plan(vec![PlanEntry {
                content: "step".into(),
                status,
            }])
        };
        t.apply(&plan(PlanStatus::Pending), true);
        t.apply(&AgentEvent::MessageChunk { text: "…".into() }, true);
        t.apply(&plan(PlanStatus::Completed), true);
        assert_eq!(
            t.items
                .iter()
                .filter(|i| matches!(i, Item::Plan(_)))
                .count(),
            1
        );
        t.apply(
            &AgentEvent::PermissionRequested(request(3, "`cargo test`", ToolKind::Execute)),
            true,
        );
        assert_eq!(t.status, Status::Awaiting);
        assert!(t.answer_permission(
            3,
            PermissionState::Answered(PermissionKind::AllowOnce, "Allow".into())
        ));
        assert_eq!(t.status, Status::Running);
        assert!(!t.answer_permission(3, PermissionState::Cancelled));
        t.apply(
            &AgentEvent::PermissionRequested(request(4, "rm -rf x", ToolKind::Execute)),
            true,
        );
        t.apply(
            &AgentEvent::TurnEnded {
                turn: 7,
                outcome: TurnOutcome::Cancelled,
            },
            true,
        );
        assert_eq!(t.pending_permissions().count(), 0);
        assert!(matches!(t.items.back(), Some(Item::Notice { text, .. }) if text == "已中断"));
        // A stale turn id is ignored.
        t.apply(
            &AgentEvent::TurnEnded {
                turn: 7,
                outcome: TurnOutcome::Failed("x".into()),
            },
            true,
        );
        assert_eq!(t.status, Status::Idle);
    }

    #[test]
    fn memory_is_bounded() {
        let mut t = Thread::new();
        for turn in 0..(MAX_ITEMS as u64) {
            t.push_user(format!("q{turn}"), vec![], turn);
            t.apply(&AgentEvent::MessageChunk { text: "a".into() }, true);
        }
        assert_eq!(t.items.len(), MAX_ITEMS);
        assert_eq!(t.dropped, MAX_ITEMS);
        t.prepend(vec![Item::User {
            text: "older".into(),
            attachments: vec![],
        }]);
        assert_eq!(t.items.len(), MAX_ITEMS);
        assert_eq!(t.dropped, MAX_ITEMS - 1);
    }
}
