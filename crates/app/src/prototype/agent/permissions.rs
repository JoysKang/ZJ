//! Permission requests and 始终允许 rules, session modes and following the settings.

use super::*;

impl Prototype {
    // ----- permissions --------------------------------------------------------------------

    pub(in crate::prototype) fn agent_answer(
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
        let root = session.root.clone();
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
            PermissionChoice::Always(prefix) => (
                allow.map(|o| o.id.clone()),
                PermissionState::Answered(
                    PermissionKind::AllowAlways,
                    format!("已始终允许 {prefix}"),
                ),
            ),
            PermissionChoice::Reject => (
                reject.map(|o| o.id.clone()),
                PermissionState::Answered(PermissionKind::RejectOnce, "已拒绝".into()),
            ),
        };
        if let Some(client) = &session.client {
            client.respond_permission(request, option);
        }
        session.thread.answer_permission(request, state);
        eprintln!(
            "event=agent_permission_answered agent={} choice={}",
            session.preset.id,
            match choice {
                PermissionChoice::Once => "once",
                PermissionChoice::Always(_) => "always",
                PermissionChoice::Reject => "reject",
            }
        );
        if let (PermissionChoice::Always(prefix), Some(root)) = (choice, root) {
            let root = root.to_string_lossy().into_owned();
            self.change_settings(window, cx, move |s| {
                let rules = s.agent.allow.entry(root).or_default();
                if !rules.contains(&prefix) {
                    rules.push(prefix);
                }
            });
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    /// ⌘⇧A: the next session waiting for approval.
    pub(in crate::prototype) fn agent_next_approval(
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

    pub(in crate::prototype) fn agent_remove_rule(
        &mut self,
        rule: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self
            .agent_workspace(cx)
            .map(|r| r.to_string_lossy().into_owned())
        else {
            return;
        };
        self.change_settings(window, cx, move |s| {
            if let Some(rules) = s.agent.allow.get_mut(&root) {
                rules.retain(|r| *r != rule);
            }
        });
        cx.notify();
    }

    // ----- modes and settings -------------------------------------------------------------

    pub(in crate::prototype) fn agent_set_mode(&mut self, mode: String, cx: &mut Context<Self>) {
        if let Some(session) = self.agent.current.and_then(|k| self.agent.session(k))
            && let Some(client) = &session.client
            && !client.set_mode(mode)
        {
            self.message = "这个模式会跳过审批，ZJ 不允许切换到它".into();
        }
        cx.notify();
    }

    /// Another window (or the settings file) changed agent settings.
    pub(in crate::prototype) fn agent_follow_settings(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let presets = presets_from(&settings);
        if presets != self.agent.presets {
            self.agent.presets = presets;
            cx.notify();
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::prototype) enum PermissionChoice {
    Once,
    Always(String),
    Reject,
}
