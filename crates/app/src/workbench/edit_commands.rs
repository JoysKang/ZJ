//! VS Code's line editing keys on a document's editor; the logic is in [`crate::editing`].
//!
//! Kit's ZJ selection interface retains every cursor and groups each command into one undo step.

#[path = "multi_edit.rs"]
mod multi_edit;

use super::{Pane, Workbench};
use crate::editing;
use gpui_kit::{KeyBinding, *};
use multi_edit::Command;

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
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(comment) = crate::languages::comment_for(&doc.path) else {
            return;
        };
        self.edit_active(window, cx, Command::Comment(comment, doc.indent.width));
    }

    /// ⌥↑ / ⌥↓: moves the selected lines past the line above or below.
    pub(super) fn move_lines(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, Command::Move(down));
    }

    /// ⇧⌥↑ / ⇧⌥↓: duplicates the selected lines and selects the copy.
    pub(super) fn copy_lines(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_active(window, cx, Command::Copy(down));
    }

    /// ⌘D: expand empty cursors to words, then add each next unselected occurrence.
    pub(super) fn select_next_occurrence(&mut self, cx: &mut Context<Self>) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(editor) = self.document(id).map(|doc| doc.editor.clone()) else {
            return;
        };
        let (text, mut selections) = {
            let state = editor.read(cx);
            (state.text().to_string(), state.selected_ranges())
        };
        if selections.iter().any(|(range, _)| range.is_empty()) {
            for (range, _) in &mut selections {
                if range.start == range.end
                    && let Some(word) = editing::word_at(&text, range.start)
                {
                    *range = word;
                }
            }
            self.whole_word_selection = Some((id, selections[0].0.clone()));
        } else {
            let primary = selections[0].0.clone();
            let whole_word = self.whole_word_selection == Some((id, primary.clone()));
            let mut candidate = primary.clone();
            let mut next = None;
            for _ in 0..=selections.len() {
                let Some(found) = editing::next_occurrence(&text, candidate, whole_word) else {
                    break;
                };
                if !selections
                    .iter()
                    .any(|(range, _)| range.start < found.end && found.start < range.end)
                {
                    next = Some(found);
                    break;
                }
                if found == primary {
                    break;
                }
                candidate = found;
            }
            let Some(next) = next else {
                return;
            };
            if whole_word {
                self.whole_word_selection = Some((id, next.clone()));
            }
            selections.insert(0, (next, false));
        }
        editor.update(cx, |state, cx| {
            state.set_selected_ranges(&selections, cx);
        });
    }

    fn edit_active(&mut self, window: &mut Window, cx: &mut Context<Self>, command: Command) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(editor) = self.document(id).map(|doc| doc.editor.clone()) else {
            return;
        };
        let (text, selections) = {
            let state = editor.read(cx);
            if !state.is_editable() {
                return;
            }
            (state.text().to_string(), state.selected_ranges())
        };
        let (edits, after) = multi_edit::compute(&text, &selections, command);
        editor.update(cx, |state, cx| {
            state.replace_ranges_with_selections(&edits, &after, window, cx);
        });
    }
}
