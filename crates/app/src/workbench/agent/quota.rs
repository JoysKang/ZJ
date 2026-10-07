//! Passive quota updates use local logs; opening the anchored card queries the live account.

use super::*;

impl Workbench {
    pub(in crate::workbench) fn agent_shows_quota(&self) -> bool {
        self.agent.current().is_some_and(|s| s.preset.id == "codex")
    }

    pub(in crate::workbench) fn agent_refresh_quota(&mut self, cx: &mut Context<Self>) {
        if !self.agent_shows_quota() {
            return;
        }
        let Some(home) = crate::quota::codex_home() else {
            return;
        };
        let job = cx.background_spawn(async move { crate::quota::read_codex(&home) });
        self.agent.quota_task = Some(cx.spawn(async move |this, cx| {
            let quota = job.await;
            let _ = this.update(cx, |this, cx| {
                if quota.as_ref().is_some_and(|q| {
                    this.agent
                        .quota
                        .as_ref()
                        .is_none_or(|old| q.updated_ms > old.updated_ms)
                }) {
                    this.agent.quota = quota;
                    cx.notify();
                }
            });
        }));
    }

    pub(in crate::workbench) fn agent_query_quota(&mut self, cx: &mut Context<Self>) {
        if self.agent.quota_loading || !self.agent_shows_quota() {
            return;
        }
        self.agent.quota_loading = true;
        self.agent.quota_error = None;
        let overrides = cx
            .global::<crate::settings::Settings>()
            .agent
            .env_for("codex");
        let requested_env = overrides.clone();
        let job = cx.background_spawn(async move {
            let env = crate::secrets::resolve(&overrides, |name| std::env::var(name).ok())?;
            let result = workspace_editor_agent::read_codex_quota(env)?;
            crate::quota::from_live(&result, workspace_editor_agent_history::now_ms())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.agent.quota_loading = false;
                if cx
                    .global::<crate::settings::Settings>()
                    .agent
                    .env_for("codex")
                    != requested_env
                {
                    this.agent.quota = None;
                    this.agent.quota_error = None;
                    cx.notify();
                    return;
                }
                match result {
                    Ok(quota) => this.agent.quota = Some(quota),
                    Err(error) => this.agent.quota_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
