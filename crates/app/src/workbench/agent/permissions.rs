//! Permission requests (questions too), session modes and following the settings.

use super::*;
use gpui_kit::component::input::InputEvent;
use workspace_editor_agent::{PermissionId, PermissionRequest, thread::permission_question};

/// A question being answered in its card: the picked answers (a multi-select) and the typed
/// text (a question that takes one).
pub(in crate::workbench) struct QuestionDraft {
    pub picks: std::collections::BTreeSet<usize>,
    pub input: Option<Entity<InputState>>,
    _enter: Option<Subscription>,
}

impl Workbench {
    // ----- permissions --------------------------------------------------------------------

    /// A question that needs more than one click (several picks, typed text) gets a draft
    /// when it arrives; ⏎ in its text field submits it.
    pub(in crate::workbench) fn agent_question_draft(
        &mut self,
        key: u64,
        request: &PermissionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(question) = permission_question(request) else {
            return;
        };
        if !question.multi && !question.typed {
            return;
        }
        let id = request.id;
        let (input, enter) = if question.typed {
            let placeholder = if question.answers.is_empty() {
                "输入答案，⏎ 提交"
            } else {
                "其他答案或补充说明（可选）"
            };
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .masked(question.secret)
            });
            let enter = cx.subscribe_in(
                &input,
                window,
                move |this, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => {
                            this.agent_answer(key, id, PermissionChoice::Submit, window, cx)
                        }
                        // The submit button follows the text.
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    }
                },
            );
            (Some(input), Some(enter))
        } else {
            (None, None)
        };
        let draft = QuestionDraft {
            picks: Default::default(),
            input,
            _enter: enter,
        };
        self.agent.questions.insert((key, id), draft);
    }

    /// A multi-select's answer clicked: picked or not.
    pub(in crate::workbench) fn agent_toggle_pick(
        &mut self,
        key: u64,
        id: PermissionId,
        n: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = self.agent.questions.get_mut(&(key, id))
            && !draft.picks.remove(&n)
        {
            draft.picks.insert(n);
        }
        cx.notify();
    }

    /// What a question's card holds now.
    pub(in crate::workbench) fn agent_question_input(
        &self,
        key: u64,
        id: PermissionId,
        cx: &App,
    ) -> QuestionInput {
        let Some(draft) = self.agent.questions.get(&(key, id)) else {
            return QuestionInput::default();
        };
        let text = draft
            .input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        QuestionInput {
            picks: draft.picks.iter().copied().collect(),
            text,
        }
    }

    pub(in crate::workbench) fn agent_answer(
        &mut self,
        key: u64,
        request: PermissionId,
        choice: PermissionChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let Some(card) = session
            .thread
            .pending_permissions()
            .find(|card| card.request.id == request)
            .cloned()
        else {
            return;
        };
        if permission_question(&card.request).is_some() {
            return self.agent_answer_question(key, &card.request, choice, window, cx);
        }
        let options = &card.request.options;
        let allow = options
            .iter()
            .find(|o| o.kind == PermissionKind::AllowOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|o| o.kind == PermissionKind::AllowAlways)
            });
        let reject = options
            .iter()
            .find(|o| o.kind == PermissionKind::RejectOnce)
            .or_else(|| {
                options
                    .iter()
                    .find(|o| o.kind == PermissionKind::RejectAlways)
            });
        let (option, state) = match &choice {
            PermissionChoice::Once => (
                allow.map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::AllowOnce, "已允许一次".into()),
            ),
            // The agent's own "always allow": it decides what the rule covers.
            PermissionChoice::Always => (
                options
                    .iter()
                    .find(|o| o.kind == PermissionKind::AllowAlways)
                    .map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::AllowAlways, "已始终允许".into()),
            ),
            PermissionChoice::Reject => (
                reject.map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::RejectOnce, "已拒绝".into()),
            ),
            // Questions only.
            PermissionChoice::Answer(_) | PermissionChoice::Submit => return,
        };
        let delivered = session
            .client
            .as_ref()
            .is_some_and(|client| client.respond_permission(request, option));
        self.agent_answered(
            key,
            request,
            delivered.then_some(state),
            &choice,
            window,
            cx,
        );
    }

    /// A question's card answered:
    /// - an answer clicked: that one (with the typed text as a note, if any);
    /// - ⏎ in the composer: what the card holds, else a single-select's first answer, else
    ///   into the text field (a question only typed text answers);
    /// - "submit" or ⏎ in the field: the picks and the text;
    /// - "skip" / Esc: skipped.
    fn agent_answer_question(
        &mut self,
        key: u64,
        request: &PermissionRequest,
        choice: PermissionChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(question) = permission_question(request) else {
            return;
        };
        let id = request.id;
        let mut input = self.agent_question_input(key, id, cx);
        let pick = |option: &str| -> Option<usize> {
            option
                .strip_prefix(workspace_editor_agent::thread::ANSWER)?
                .parse()
                .ok()
        };
        let one = match choice {
            PermissionChoice::Reject => {
                let skip = request
                    .options
                    .iter()
                    .find(|o| o.kind == PermissionKind::RejectOnce)
                    .map(|o| o.id.clone());
                let session = self.agent.session(key);
                let delivered = session
                    .and_then(|s| s.client.as_ref())
                    .is_some_and(|client| client.respond_permission(id, skip));
                let state = PermissionState::Answered(PermissionKind::RejectOnce, "已跳过".into());
                return self.agent_answered(
                    key,
                    id,
                    delivered.then_some(state),
                    &choice,
                    window,
                    cx,
                );
            }
            PermissionChoice::Answer(option) if input.text.is_empty() => Some(option),
            PermissionChoice::Answer(option) => {
                input.picks = pick(&option).into_iter().collect();
                None
            }
            PermissionChoice::Once if input.is_empty() => {
                if let Some(first) = question.answers.first().filter(|_| !question.multi) {
                    Some(first.option.clone())
                } else if let Some(field) = self
                    .agent
                    .questions
                    .get(&(key, id))
                    .and_then(|d| d.input.clone())
                {
                    field.update(cx, |field, cx| field.focus(window, cx));
                    return;
                } else {
                    self.message = "先选择答案，再点「提交」".into();
                    cx.notify();
                    return;
                }
            }
            PermissionChoice::Once | PermissionChoice::Submit => None,
            PermissionChoice::Always => return,
        };
        if let Some(option) = one {
            let label = question
                .answers
                .iter()
                .find(|a| a.option == option)
                .map(|a| a.label.clone())
                .unwrap_or_default();
            let session = self.agent.session(key);
            let delivered = session
                .and_then(|s| s.client.as_ref())
                .is_some_and(|client| client.respond_permission(id, Some(option)));
            let state =
                PermissionState::Answered(PermissionKind::AllowOnce, format!("已回答：{label}"));
            let choice = PermissionChoice::Answer(String::new());
            return self.agent_answered(key, id, delivered.then_some(state), &choice, window, cx);
        }
        if input.is_empty() {
            self.message = "选择答案或输入内容后再提交".into();
            cx.notify();
            return;
        }
        let session = self.agent.session(key);
        let delivered = session
            .and_then(|s| s.client.as_ref())
            .is_some_and(|client| client.answer_question(id, &input.picks, &input.text));
        let mut parts: Vec<String> = input
            .picks
            .iter()
            .filter_map(|n| question.answers.get(*n).map(|a| a.label.clone()))
            .collect();
        if !input.text.is_empty() {
            // A secret never shows, here or in the history.
            parts.push(if question.secret {
                "（已隐藏的输入）".into()
            } else {
                input.text
            });
        }
        let label = format!("已回答：{}", parts.join("、"));
        let state = PermissionState::Answered(PermissionKind::AllowOnce, label);
        let choice = PermissionChoice::Submit;
        self.agent_answered(key, id, delivered.then_some(state), &choice, window, cx);
    }

    /// After an answer went to the agent (`state`: what the card shows), or didn't (`None`).
    fn agent_answered(
        &mut self,
        key: u64,
        request: PermissionId,
        state: Option<PermissionState>,
        choice: &PermissionChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The client answered `cancelled` already (a stop, the agent exited): the card waits
        // for the turn's end instead of claiming an answer the agent never got.
        let Some(state) = state else {
            self.message = "这个请求已失效（会话已停止或 Agent 已退出）".into();
            cx.notify();
            return;
        };
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        session.thread.answer_permission(request, state);
        eprintln!(
            "event=agent_permission_answered agent={} choice={}",
            session.preset.id,
            choice.name()
        );
        if session.thread.pending_permissions().next().is_none() {
            self.agent_dismiss_notification(key, cx);
        }
        self.agent.questions.remove(&(key, request));
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    /// ⌘⇧A: the next session waiting for approval.
    pub(in crate::workbench) fn agent_next_approval(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.agent.current;
        let waiting: Vec<u64> = self
            .agent
            .sessions
            .iter()
            .filter(|s| s.thread.status == agent_thread::Status::Awaiting)
            .map(|s| s.key)
            .collect();
        let next = waiting
            .iter()
            .find(|&&k| Some(k) > current)
            .or_else(|| waiting.first())
            .copied();
        if let Some(key) = next {
            if !self.agent.visible {
                self.set_agent_panel(true, window, cx);
            }
            self.agent_select(key, window, cx);
        } else {
            self.message = "没有待批准的请求".into();
            cx.notify();
        }
    }

    // ----- modes and settings -------------------------------------------------------------

    pub(in crate::workbench) fn agent_set_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        if let Some(session) = self.agent.current.and_then(|k| self.agent.session(k))
            && let Some(client) = &session.client
            && !client.set_mode(mode)
        {
            self.message = "这个模式会跳过审批，ZJ 不允许切换到它".into();
        }
        cx.notify();
    }

    /// Picks a model setting (model, effort…) offered by the current session's agent.
    pub(in crate::workbench) fn agent_set_config(
        &mut self,
        id: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.agent.current.and_then(|k| self.agent.session(k))
            && let Some(client) = &session.client
            && !client.set_config_option(id, value)
        {
            self.message = "Agent 已不再提供这个选项".into();
        }
        cx.notify();
    }

    /// Another window (or the settings file) changed agent settings.
    pub(in crate::workbench) fn agent_follow_settings(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let presets = presets_from(&settings);
        if presets != self.agent.presets {
            self.agent.presets = presets;
            cx.notify();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::workbench) enum PermissionChoice {
    Once,
    /// The agent's own "always allow" option (offered only when the agent has one).
    Always,
    Reject,
    /// One of a question's answers (its option id).
    Answer(String),
    /// A question's picks and typed text, from its draft.
    Submit,
}

impl PermissionChoice {
    /// For the log.
    fn name(&self) -> &'static str {
        match self {
            PermissionChoice::Once => "once",
            PermissionChoice::Always => "always",
            PermissionChoice::Reject => "reject",
            PermissionChoice::Answer(_) => "answer",
            PermissionChoice::Submit => "submit",
        }
    }
}

/// What a question's card holds: the picked answers and the typed text (trimmed).
#[derive(Default)]
pub(in crate::workbench) struct QuestionInput {
    pub picks: Vec<usize>,
    pub text: String,
}

impl QuestionInput {
    pub fn is_empty(&self) -> bool {
        self.picks.is_empty() && self.text.is_empty()
    }
}
