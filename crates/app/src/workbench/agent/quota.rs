//! The Codex account quota under the composer, read from Codex's logs in the background when
//! a Codex session is shown, when one of its turns ends and when the card is opened. No timer.

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
                if this.agent.quota != quota {
                    this.agent.quota = quota;
                    cx.notify();
                }
            });
        }));
    }

    pub(in crate::workbench) fn agent_toggle_quota(&mut self, cx: &mut Context<Self>) {
        self.agent.quota_open = !self.agent.quota_open;
        if self.agent.quota_open {
            self.agent_refresh_quota(cx);
        }
        cx.notify();
    }
}
