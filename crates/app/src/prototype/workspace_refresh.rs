//! Event-driven refresh orchestration; idle windows have no polling task or timer.
use super::Prototype;
use crate::watch::WatchService;
use gpui_kit::*;
use std::time::{Duration, Instant};

const QUIET_PERIOD: Duration = Duration::from_millis(200);
const MAX_MERGE: Duration = Duration::from_secs(1);

impl Prototype {
    pub(super) fn start_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = &self.root else { return };
        let subscription = match cx.global::<WatchService>().subscribe(root) {
            Ok(subscription) => subscription,
            Err(error) => {
                self.watch_error = Some(format!("文件监听不可用：{error}；请手动刷新"));
                return;
            }
        };
        self.watch_error = subscription.take_pending().error;
        self.watch = Some(subscription.clone());
        self.watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let mut notice = subscription.receive().await;
                if !notice.changed && notice.error.is_none() {
                    continue;
                }
                let started = Instant::now();
                if this
                    .update_in(cx, |this, _, cx| {
                        this.watch_debouncing = true;
                        this.workspace_refresh_pending = true;
                        this.invalidate_preview(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                loop {
                    cx.background_executor().timer(QUIET_PERIOD).await;
                    let next = subscription.take_pending();
                    let another_change = next.changed;
                    notice.merge(next);
                    if !another_change || started.elapsed() >= MAX_MERGE {
                        break;
                    }
                }
                if this
                    .update_in(cx, |this, window, cx| {
                        if let Some(error) = notice.error {
                            this.watch_error = Some(format!("文件监听错误：{error}；请手动刷新"));
                        }
                        this.watch_debouncing = false;
                        this.flush_workspace_refresh(window, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(super) fn flush_workspace_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.workspace_refresh_pending
            || self.watch_debouncing
            || self.loading
            || self.index_task.is_some()
            || !self.tree_tasks.is_empty()
        {
            return;
        }
        self.workspace_refresh_pending = false;
        // Re-enumeration preserves expansion and never forces open a manually collapsed path.
        let reveal_pending = self.reveal_pending;
        self.refresh_tree(window, cx);
        self.reveal_pending = reveal_pending;
        self.refresh(window, cx);
    }

    pub(super) fn invalidate_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.preview_generation += 1;
        self.preview_task = None;
        if self.preview_diff.is_some() {
            self.preview_stale = true;
        }
        cx.notify();
    }
}
