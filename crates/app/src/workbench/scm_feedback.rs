//! Results that need more than a status-bar line: one dialog with a bounded, selectable
//! and copyable detail that stays until the user closes it. Failed Git writes and the
//! repository errors a scan could not read both use it.
use super::{SINGLE_LINE, Workbench};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    base::{AlertDialog, AlertDialogBackdrop, AlertDialogPopup, SelectableText, TestSupportExt},
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex, v_flex,
    },
    *,
};

pub(super) struct Notice {
    /// Names what went wrong, e.g. "kt_stu_api：推送失败" or "2 项仓库错误".
    pub(super) title: String,
    /// One sentence on what the user can do next.
    note: &'static str,
    /// The raw output; what 复制 puts on the clipboard.
    pub(super) detail: String,
    /// Whether opening it moves the keyboard into the dialog. A failure the user did not ask
    /// for (auto-save) shows the same dialog but leaves them typing.
    take_focus: bool,
}

impl Workbench {
    /// Success, no change and hints need no dialog: one line in the status bar names what
    /// changed or what the user has to do, and the repository itself shows the result.
    pub(super) fn note_git_result(&mut self, g: usize, summary: &str, cx: &mut Context<Self>) {
        let Some(line) = self.git_result_line(g, summary) else {
            return;
        };
        self.message = line;
        cx.notify();
    }

    /// The status-bar line for one finished write, repository name first.
    pub(super) fn git_result_line(&self, g: usize, summary: &str) -> Option<String> {
        let group = self.groups.get(g)?;
        let name = repo_name(&group.repo.worktree);
        Some(if name.is_empty() {
            summary.to_owned()
        } else {
            format!("{name}：{summary}")
        })
    }

    /// Shows the results that finished while another repository's 「正在…」 line held the
    /// status bar, together and in order, the moment the line is free. A slow push must not
    /// hide the commit that completed behind it.
    pub(super) fn release_write_results(&mut self, cx: &mut Context<Self>) {
        if self.write_progress.is_some() || self.deferred_results.is_empty() {
            return;
        }
        self.message = self.deferred_results.join(" · ");
        self.deferred_results.clear();
        cx.notify();
    }

    /// Anything that fails while writing keeps the reason and the raw text until the user
    /// closes it; the status bar alone would be overwritten by the next message.
    pub(super) fn report_failure(
        &mut self,
        title: String,
        note: &'static str,
        detail: String,
        take_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_notice(
            Notice {
                title,
                note,
                detail,
                take_focus,
            },
            window,
            cx,
        );
    }

    /// A failed write keeps the reason and the raw output until the user closes it;
    /// refreshing is offered right there, because the write may have stopped half way.
    pub(super) fn report_git_failure(
        &mut self,
        title: String,
        detail: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = workspace_editor_git::redact_git_output(&title);
        let detail = workspace_editor_git::redact_git_output(&detail);
        self.report_failure(
            title,
            "操作可能部分完成；刷新状态后检查仓库。",
            detail,
            true,
            window,
            cx,
        );
    }

    /// A save that never reached the disk. The buffer is untouched, so the reason is the
    /// only thing the user needs from here. `take_focus` is false for auto-save.
    pub(super) fn report_save_failure(
        &mut self,
        title: String,
        detail: String,
        take_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.report_failure(
            title,
            "文件没有写入磁盘；编辑器里的内容仍然保留。",
            detail,
            take_focus,
            window,
            cx,
        );
    }

    /// The status bar has room for a count only; this lists every error with its path.
    pub(super) fn show_repository_errors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issues.is_empty() {
            return;
        }
        self.show_notice(
            Notice {
                title: format!("{} 项仓库错误", self.issues.len()),
                note: "刷新状态重新读取仓库；错误原文如下。",
                detail: self.issues.join("\n"),
                take_focus: true,
            },
            window,
            cx,
        );
    }

    fn show_notice(&mut self, notice: Notice, window: &mut Window, cx: &mut Context<Self>) {
        // An auto-save that keeps failing would otherwise stack the same reason again and
        // again; one already on screen or waiting adds nothing.
        let repeated =
            |other: &Notice| other.title == notice.title && other.detail == notice.detail;
        if self.notice.as_ref().is_some_and(repeated)
            || self.queued_notices.iter().any(repeated)
            || self.deferred_notices.iter().any(repeated)
        {
            return;
        }
        if !notice.take_focus {
            // Nothing may take the keyboard: the status bar carries the entry until the
            // user opens it, so a failing auto-save cannot steal a keystroke.
            // The dialog's own handle is not somewhere to go back to later.
            if self.deferred_notices.is_empty() && !self.notice_focus.is_focused(window) {
                self.deferred_return_focus = window.focused(cx);
            }
            self.deferred_notices.push(notice);
            cx.notify();
            return;
        }
        self.message.clear();
        if self.notice.is_some() {
            // Writes to different repositories can fail at the same time; the next reason
            // follows the current dialog instead of replacing it unseen.
            self.queued_notices.push_back(notice);
            cx.notify();
            return;
        }
        let take_focus = notice.take_focus;
        // Opening deferred notices already knows where the user was; a click on the status
        // bar is not it.
        self.notice_return_focus = self
            .notice_return_focus
            .take()
            .or_else(|| window.focused(cx));
        self.notice = Some(notice);
        if take_focus {
            self.notice_focus.focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_notice(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.queued_notices.pop_front() {
            // A quiet notice only means "do not take the keyboard from an editor"; when the
            // keyboard is already on the dialog, it stays there.
            let take_focus = next.take_focus || self.notice_focus.is_focused(window);
            self.notice = Some(next);
            if take_focus {
                self.notice_focus.focus(window, cx);
            }
            cx.notify();
            return;
        }
        self.notice = None;
        // Back to where the user was: a failure usually arrives while they are editing,
        // and taking focus into the sidebar would swallow their next keystrokes.
        let back = match self.notice_return_focus.take() {
            Some(back) => back,
            None => self
                .active_editor()
                .map(|editor| editor.read(cx).focus_handle(cx))
                .unwrap_or_else(|| self.sidebar_focus.clone()),
        };
        back.focus(window, cx);
        cx.notify();
    }

    /// Opens the failures that waited in the status bar. This is a deliberate action, so
    /// the first one takes the keyboard and the rest follow it.
    pub(super) fn show_deferred_notices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notices = std::mem::take(&mut self.deferred_notices);
        if notices.is_empty() {
            return;
        }
        if self.notice.is_none() {
            self.notice_return_focus = self.deferred_return_focus.take();
        }
        for mut notice in notices {
            notice.take_focus = true;
            self.show_notice(notice, window, cx);
        }
    }

    pub(super) fn render_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.notice.as_ref()?;
        let colors = theme::colors(cx);
        let detail = notice.detail.clone();
        let copy = detail.clone();
        let view = cx.entity();
        let close = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            view.update(cx, |this, cx| this.close_notice(window, cx));
        };
        Some(
            AlertDialog::new(cx)
                .focus_handle(self.notice_focus.clone())
                .backdrop(
                    AlertDialogBackdrop::new()
                        .absolute()
                        .size_full()
                        .bg(colors.scrim),
                )
                .popup(
                    AlertDialogPopup::new().child(
                        v_flex()
                            .id("scm-notice")
                            .test_support()
                            .w(theme::DIALOG_WIDTH)
                            .min_w_0()
                            .gap_3()
                            .p_4()
                            .rounded(theme::RADIUS_OVERLAY)
                            .border_1()
                            .border_color(colors.card_border)
                            .bg(colors.card)
                            .shadow(vec![theme::overlay_shadow(colors.shadow)])
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(theme::ICON_SIZE)
                                            .text_color(colors.deleted),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .text_size(theme::TEXT_BODY)
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(notice.title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(theme::TEXT_CAPTION)
                                    .text_color(colors.muted)
                                    .child(notice.note),
                            )
                            .child(
                                div()
                                    .id("scm-notice-detail")
                                    .test_support()
                                    .w_full()
                                    .min_w_0()
                                    .max_h(theme::DIALOG_DETAIL_MAX)
                                    .overflow_y_scroll()
                                    .p_2()
                                    .rounded(theme::RADIUS)
                                    .bg(colors.editor)
                                    .text_size(theme::TEXT_CAPTION)
                                    .text_color(colors.foreground)
                                    .line_height(theme::SCM_DETAIL_LINE)
                                    .cursor_text()
                                    .child(SelectableText::new("scm-notice-text", detail)),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("scm-notice-copy")
                                            .small()
                                            .ghost()
                                            .label("复制")
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy.clone(),
                                                ))
                                            }),
                                    )
                                    .child(
                                        Button::new("scm-notice-refresh")
                                            .small()
                                            .ghost()
                                            .label("刷新状态")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.close_notice(window, cx);
                                                this.refresh(window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("scm-notice-close")
                                            .small()
                                            .label("关闭")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.close_notice(window, cx);
                                            })),
                                    ),
                            ),
                    ),
                )
                .on_close(close)
                .into_any_element(),
        )
    }
}

fn repo_name(worktree: &std::path::Path) -> String {
    worktree
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(SINGLE_LINE, "⏎")
}
