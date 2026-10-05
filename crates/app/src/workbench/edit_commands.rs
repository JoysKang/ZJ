//! VS Code's line editing keys on a document's editor; the logic is in [`crate::editing`].
//!
//! Kit only exposes the primary selection (`selected_range` / `set_selected_range`), so these
//! commands act on it and drop any extra cursors.

use super::{Document, Pane, Workbench};
use crate::editing::{self, Edit};
use crate::replace;
use gpui_kit::{EntityInputHandler, KeyBinding, *};
use std::ops::Range;

gpui_kit::actions!(
    editor,
    [
        ToggleLineComment,
        MoveLinesUp,
        MoveLinesDown,
        CopyLinesUp,
        CopyLinesDown,
        SelectNextOccurrence
    ]
);

/// Key context around a document's editor. Bindings in `DocumentEditor > Input` reach only
/// document buffers (not the find widget or quick open) and match at the same depth as Kit's
/// own `Input` bindings, where the later registration wins.
pub(super) const CONTEXT: &str = "DocumentEditor";

/// Registered after `gpui_kit::init`, so they win over Kit's defaults for the same keys.
pub fn key_bindings() -> Vec<KeyBinding> {
    let input = Some("DocumentEditor > Input");
    vec![
        KeyBinding::new("secondary-/", ToggleLineComment, input),
        KeyBinding::new("alt-up", MoveLinesUp, input),
        KeyBinding::new("alt-down", MoveLinesDown, input),
        // Kit binds these to add cursors off macOS; ⌥⌘↑ / ⌥⌘↓ still add cursors.
        KeyBinding::new("shift-alt-up", CopyLinesUp, input),
        KeyBinding::new("shift-alt-down", CopyLinesDown, input),
        KeyBinding::new("secondary-d", SelectNextOccurrence, input),
    ]
}

impl Workbench {
    /// ⌘/: comments out or uncomments the selected lines.
    pub(super) fn toggle_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, |doc, text, selection| {
            let comment = crate::languages::comment_for(&doc.path)?;
            editing::toggle_comment(text, selection, comment, doc.indent.width)
        });
    }

    /// ⌥↑ / ⌥↓: moves the selected lines past the line above or below.
    pub(super) fn move_lines(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, |_, text, selection| {
            editing::move_lines(text, selection, down)
        });
    }

    /// ⇧⌥↑ / ⇧⌥↓: duplicates the selected lines and selects the copy.
    pub(super) fn copy_lines(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, |_, text, selection| {
            Some(editing::copy_lines(text, selection, down))
        });
    }

    /// ⌘D: selects the word at the cursor, then moves the selection to the next occurrence.
    /// Kit cannot add a selection from outside, so this moves the one selection (VS Code's
    /// ⌘K ⌘D) instead of adding one.
    pub(super) fn select_next_occurrence(&mut self, cx: &mut Context<Self>) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(editor) = self.document(id).map(|doc| doc.editor.clone()) else {
            return;
        };
        let (text, selection) = {
            let state = editor.read(cx);
            (state.text().to_string(), state.selected_range())
        };
        let next = if selection.is_empty() {
            let word = editing::word_at(&text, selection.start);
            self.whole_word_selection = word.clone().map(|word| (id, word));
            word
        } else {
            let whole_word = self.whole_word_selection == Some((id, selection.clone()));
            let next = editing::next_occurrence(&text, selection, whole_word);
            if whole_word && let Some(next) = &next {
                self.whole_word_selection = Some((id, next.clone()));
            }
            next
        };
        if let Some(range) = next {
            editor.update(cx, |state, cx| state.set_selected_range(range, cx));
        }
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
