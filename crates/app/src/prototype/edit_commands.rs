//! VS Code's line editing keys on a document's editor; the logic is in [`crate::editing`].
//!
//! Kit only exposes the primary selection (`selected_range` / `set_selected_range`), so these
//! commands act on it and drop any extra cursors.

use super::{Document, Pane, Prototype};
use crate::editing::{self, Edit};
use crate::replace;
use gpui_kit::{EntityInputHandler, KeyBinding, *};
use std::ops::Range;

gpui_kit::actions!(editor, [ToggleLineComment]);

/// Key context around a document's editor. Bindings in `DocumentEditor > Input` reach only
/// document buffers (not the find widget or quick open) and match at the same depth as Kit's
/// own `Input` bindings, where the later registration wins.
pub(super) const CONTEXT: &str = "DocumentEditor";

/// Registered after `gpui_kit::init`, so they win over Kit's defaults for the same keys.
pub fn key_bindings() -> Vec<KeyBinding> {
    let input = Some("DocumentEditor > Input");
    vec![KeyBinding::new("secondary-/", ToggleLineComment, input)]
}

impl Prototype {
    /// ⌘/: comments out or uncomments the selected lines.
    pub(super) fn toggle_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, |doc, text, selection| {
            let comment = crate::languages::comment_for(&doc.path)?;
            editing::toggle_comment(text, selection, comment, doc.indent.width)
        });
    }

    /// Computes one edit from the active buffer's text and primary selection and applies it as
    /// a single undo step.
    fn edit_active(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        compute: impl FnOnce(&Document, &str, Range<usize>) -> Option<Edit>,
    ) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(doc) = self.document(id) else {
            return;
        };
        let editor = doc.editor.clone();
        let (text, selection) = {
            let state = editor.read(cx);
            (state.text().to_string(), state.selected_range())
        };
        let Some(edit) = compute(doc, &text, selection) else {
            return;
        };
        editor.update(cx, |state, cx| {
            let range = replace::utf16_offset(&text, edit.range.start)
                ..replace::utf16_offset(&text, edit.range.end);
            state.replace_text_in_range(Some(range), &edit.text, window, cx);
            state.set_selected_range(edit.selection, cx);
        });
    }
}
