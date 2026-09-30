//! Source Control view.

use super::Prototype;
use crate::theme;
use gpui_kit::{
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    *,
};
use std::sync::atomic::Ordering;

impl Prototype {
    pub(super) fn render_scm(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .px_2()
                    .pb_2()
                    .gap_1()
                    .child(
                        Button::new("refresh-git")
                            .small()
                            .ghost()
                            .label(if self.loading {
                                "重新扫描"
                            } else {
                                "刷新"
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.refresh(window, cx))),
                    )
                    .child(
                        Button::new("cancel-git")
                            .small()
                            .ghost()
                            .label("取消")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel.store(true, Ordering::Relaxed);
                                this.generation += 1;
                                this.refresh_task = None;
                                this.loading = false;
                                this.close_preview(cx);
                                this.message = "已取消；当前结果可能不完整，请重新扫描".into();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_size(theme::TEXT_CAPTION)
                    .child(format!(
                        "全部仓库 · {} · {}",
                        self.groups.len(),
                        if self.loading {
                            "刷新中"
                        } else {
                            "磁盘快照"
                        }
                    )),
            )
            .child(
                uniform_list(
                    "changes",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|index| this.row(index, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }
}
