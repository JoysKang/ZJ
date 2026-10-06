//! Permission requests, session modes and following the settings.

use super::*;

impl Workbench {
    // ----- permissions --------------------------------------------------------------------

    pub(in crate::workbench) fn agent_answer(
        &mut self,
        key: u64,
        request: workspace_editor_agent::PermissionId,
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
        };
        // The client answered `cancelled` already (a stop, the agent exited): the card waits
        // for the turn's end instead of claiming an answer the agent never got.
        let delivered = session
            .client
            .as_ref()
            .is_some_and(|client| client.respond_permission(request, option));
        if !delivered {
            self.message = "这个请求已失效（会话已停止或 Agent 已退出）".into();
            cx.notify();
            return;
        }
        session.thread.answer_permission(request, state);
        eprintln!(
            "event=agent_permission_answered agent={} choice={}",
            session.preset.id,
            match choice {
                PermissionChoice::Once => "once",
                PermissionChoice::Always => "always",
                PermissionChoice::Reject => "reject",
            }
        );
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
}
