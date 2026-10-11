//! Selection tools preserve document focus, UTF-8 ranges, drafts and explicit sending.

use super::test_support::{empty_store, open_window};
use super::*;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{TestAppContext, base::Root, test::TestWindowExt};

fn open(
    cx: &mut TestAppContext,
    source: &str,
) -> (WindowHandle<Root>, Entity<Workbench>, Entity<EditorState>) {
    let (handle, this) = open_window(
        cx,
        None,
        crate::settings::Settings::default(),
        empty_store(),
    );
    let editor = cx
        .update_window(handle.into(), |_, window, cx| {
            let editor = this.update(cx, |p, cx| {
                let id = p.new_untitled(window, cx);
                p.document(id).unwrap().editor.clone()
            });
            editor.update(cx, |editor, cx| editor.set_value(source, window, cx));
            cx.bind_keys([
                KeyBinding::new(
                    "alt-secondary-l",
                    ShowSelectionActions,
                    Some("DocumentEditor > Input"),
                ),
                KeyBinding::new("secondary-l", AddSelectionToAgent, Some("WorkspaceEditor")),
            ]);
            window.render_frame(cx);
            editor
        })
        .unwrap();
    (handle, this, editor)
}

#[gpui_kit::test]
async fn selection_tools_cancel_context_and_no_selection_preserve_editor(cx: &mut TestAppContext) {
    let source = "首行\n中文😀内容\n末行";
    let range = "首行\n".len().."首行\n中文😀内容\n".len();
    let (handle, this, editor) = open(cx, source);
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("alt-cmd-l", cx);
        assert!(this.read(cx).selection_actions.is_none());
        editor.update(cx, |state, cx| state.set_selected_range(range.clone(), cx));
        window.render_frame(cx);
        window.press("alt-cmd-l", cx);
        window.render_frame(cx);
        let toolbar = window.find("selection-actions").bounds();
        let input = editor.read(cx).input_bounds();
        assert!(toolbar.left() >= input.left());
        assert!(toolbar.right() <= window.viewport_size().width);
        assert!(toolbar.top() < input.top() + theme::ROW_HEIGHT * 3.);
        window.press("escape", cx);
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_none());
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(editor.read(cx).selected_range(), range);
        assert!(this.read(cx).agent.attachments.is_empty());
        // The same range selected backwards follows the same UTF-8 line rules.
        editor.update(cx, |state, cx| state.set_selected_range(range.end..range.end, cx));
        window.render_frame(cx);
        window.press("shift-up", cx);
        assert_eq!(editor.read(cx).selected_range(), range);
        window.press("alt-cmd-l", cx);
        window.render_frame(cx);
        window.press("right", cx);
        window.press("right", cx);
        window.press("enter", cx);
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_none());
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        assert_eq!(editor.read(cx).selected_range(), range);
        assert_eq!(editor.read(cx).text().to_string(), source);
        assert!(matches!(&this.read(cx).agent.attachments[..], [crate::agent_model::Attachment::Selection { start: 2, end: 2, text, .. }] if text == "中文😀内容\n"));
        assert!(this.read(cx).agent.composer.read(cx).value().is_empty());
        // Context attachments are deduplicated; the existing ⌘L still focuses the composer.
        window.press("cmd-l", cx);
        assert_eq!(this.read(cx).agent.attachments.len(), 1);
        assert!(this.read(cx).agent.composer.read(cx).focus_handle(cx).is_focused(window));
        window.press("alt-cmd-l", cx);
        assert!(this.read(cx).selection_actions.is_none(), "editor-only binding reached composer");
    }).unwrap();
}

#[gpui_kit::test]
async fn selection_tools_prepare_requests_keep_drafts_and_do_not_send(cx: &mut TestAppContext) {
    use gpui_kit::component::input::{InlineToken, InputContent};
    let (handle, this, editor) = open(cx, "fn main() {}\n");
    let prefix = "保留这条要求😀";
    let draft = format!("{prefix}[图 1]");
    let token_range = prefix.len()..draft.len();
    cx.update_window(handle.into(), |_, window, cx| {
        this.update(cx, |p, cx| {
            p.agent_ensure_session(cx);
            p.agent
                .image_references
                .insert("image-reference:0".into(), None);
            p.agent.composer.update(cx, |input, cx| {
                let content = InputContent::new(draft.clone())
                    .with_token(
                        token_range.clone(),
                        InlineToken::new("image-reference:0", "[图 1]").with_label("图 1"),
                    )
                    .unwrap();
                input.set_value(content, window, cx);
            });
        });
        for index in [0usize, 1] {
            editor.update(cx, |state, cx| {
                state.focus(window, cx);
                state.set_selected_range(0..12, cx);
            });
            window.render_frame(cx);
            window.press("alt-cmd-l", cx);
            window.render_frame(cx);
            window.click(("selection-action", index), cx);
            window.render_frame(cx);
            assert!(this.read(cx).selection_actions.is_none());
            assert!(
                this.read(cx)
                    .agent
                    .composer
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(this.read(cx).agent.attachments.len(), 1);
            assert!(
                this.read(cx)
                    .agent
                    .sessions
                    .iter()
                    .all(|session| session.thread.items.is_empty() && session.client.is_none())
            );
        }
        let input = this.read(cx).agent.composer.clone();
        assert_eq!(
            input.read(cx).value().as_ref(),
            format!("{draft}\n\n请解释这段代码。\n\n请结合上面的要求修改这段代码。")
        );
        assert_eq!(input.read(cx).tokens().len(), 1);
        assert_eq!(input.read(cx).tokens()[0].range(), token_range);
        window.press("cmd-z", cx);
        window.render_frame(cx);
        assert_eq!(
            input.read(cx).value().as_ref(),
            format!("{draft}\n\n请解释这段代码。")
        );
        window.press("cmd-z", cx);
        window.render_frame(cx);
        assert_eq!(input.read(cx).value().as_ref(), draft);
        assert_eq!(input.read(cx).tokens()[0].range(), token_range);
        assert_eq!(editor.read(cx).selected_range(), 0..12);
        assert_eq!(editor.read(cx).text().to_string(), "fn main() {}\n");
    })
    .unwrap();
}

#[gpui_kit::test]
async fn selection_tools_drop_stale_selection_and_tab_switch(cx: &mut TestAppContext) {
    let (handle, this, editor) = open(cx, "原始代码");
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |state, cx| state.set_selected_range(0..12, cx));
        window.render_frame(cx);
        window.press("alt-cmd-l", cx);
        window.render_frame(cx);
        // A background replacement must not attach a snapshot of stale contents.
        editor.update(cx, |state, cx| {
            state.set_value("变化代码", window, cx);
            state.set_selected_range(0..12, cx);
        });
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_none());
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        assert!(this.read(cx).agent.attachments.is_empty());
        assert!(this.read(cx).agent.composer.read(cx).value().is_empty());
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_range(0..12, cx);
        });
        window.render_frame(cx);
        window.press("alt-cmd-l", cx);
        window.render_frame(cx);
        assert!(window.try_find("selection-actions").is_some());
        this.update(cx, |p, cx| {
            p.new_untitled(window, cx);
        });
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_none());
        assert!(window.try_find("selection-actions").is_none());
        assert!(this.read(cx).agent.attachments.is_empty());
    })
    .unwrap();
}

#[gpui_kit::test]
async fn selection_tools_drop_stale_line_numbers(cx: &mut TestAppContext) {
    let (handle, this, editor) = open(cx, "a\n代码");
    cx.update_window(handle.into(), |_, window, cx| {
        editor.update(cx, |state, cx| state.set_selected_range(2..8, cx));
        window.render_frame(cx);
        window.press("alt-cmd-l", cx);
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_some());
        // Equal-length prefix edits can change the lines without changing text or byte range.
        editor.update(cx, |state, cx| {
            state.set_value("ab代码", window, cx);
            state.set_selected_range(2..8, cx);
        });
        assert_eq!(editor.read(cx).selected_range(), 2..8);
        assert_eq!(editor.read(cx).selected_text(), "代码");
        window.render_frame(cx);
        assert!(this.read(cx).selection_actions.is_none());
        assert!(editor.read(cx).focus_handle(cx).is_focused(window));
        assert!(this.read(cx).agent.attachments.is_empty());
        assert!(this.read(cx).agent.composer.read(cx).value().is_empty());
    })
    .unwrap();
}
