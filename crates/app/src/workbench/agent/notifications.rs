//! Native notifications identify one live session; clicking only opens that session.
use super::*;
use std::sync::OnceLock;

fn prefix(owner: EntityId) -> String {
    static LAUNCH: OnceLock<u128> = OnceLock::new();
    let launch = LAUNCH.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    });
    format!("zj-agent-{launch}-{owner}-")
}

pub fn init(cx: &App) {
    cx.on_system_notification_response(|response, cx| {
        for handle in cx.windows() {
            let Some(view) = handle
                .downcast::<gpui_kit::base::Root>()
                .and_then(|handle| handle.read(cx).ok())
                .and_then(|root| root.view().clone().downcast::<Workbench>().ok())
            else {
                continue;
            };
            let Some(key) = response
                .tag
                .strip_prefix(&prefix(view.entity_id()))
                .and_then(|key| key.parse::<u64>().ok())
                .filter(|key| view.read(cx).agent.session(*key).is_some())
            else {
                continue;
            };
            cx.dismiss_system_notification(&response.tag);
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |this, cx| {
                    window.activate_window();
                    this.set_agent_panel(true, window, cx);
                    if this.agent.current == Some(key) {
                        this.agent_show(AgentView::Thread, window, cx);
                    } else {
                        this.agent_select(key, window, cx);
                    }
                });
            });
            break;
        }
    });
}

impl Workbench {
    pub(in crate::workbench) fn agent_dismiss_notification(&self, key: u64, cx: &Context<Self>) {
        cx.dismiss_system_notification(&format!("{}{key}", prefix(cx.entity_id())));
    }

    pub(in crate::workbench) fn agent_notify(&self, key: u64, awaiting: bool, cx: &Context<Self>) {
        let Some(session) = self.agent.session(key) else {
            return;
        };
        let state = if awaiting {
            "等待你的确认"
        } else if session.thread.status == agent_thread::Status::Error {
            "运行失败"
        } else {
            "已完成"
        };
        let workspace = session
            .root
            .as_deref()
            .or(self.root.as_deref())
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "默认工作区".into());
        cx.show_system_notification(SystemNotification {
            tag: format!("{}{key}", prefix(cx.entity_id())).into(),
            title: format!("{} {state}", session.preset.display_name).into(),
            body: format!("{workspace} · {}", session.title()).into(),
            actions: Vec::new(),
        });
    }
}
