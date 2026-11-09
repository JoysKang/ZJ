//! Bounded, independent histories for search and replacement fields.

use gpui_kit::{component::input::InputState, *};

const LIMIT: usize = 30;
const MAX_ENTRY_BYTES: usize = 16 * 1024;

gpui_kit::actions!(search_history, [HistoryPrevious, HistoryNext]);

#[derive(Default)]
#[cfg_attr(test, derive(Debug))]
pub(in crate::workbench) struct InputHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
    draft: String,
}

impl InputHistory {
    pub fn remember(&mut self, value: &str) {
        self.remember_value(value, false);
    }

    pub fn remember_replacement(&mut self, value: &str) {
        self.remember_value(value, true);
    }

    fn remember_value(&mut self, value: &str, allow_empty: bool) {
        self.cursor = None;
        if (!allow_empty && value.is_empty()) || value.len() > MAX_ENTRY_BYTES {
            return;
        }
        self.entries.retain(|entry| entry != value);
        self.entries.insert(0, value.to_owned());
        self.entries.truncate(LIMIT);
    }

    fn step(&mut self, previous: bool, current: &str) -> Option<String> {
        if current.len() > MAX_ENTRY_BYTES {
            return None;
        }
        if self.cursor.is_some_and(|i| self.entries[i] != current) {
            self.cursor = None;
        }
        if self.entries.is_empty() {
            return None;
        }
        if self.cursor.is_none() {
            if !previous {
                return None;
            }
            self.draft = current.to_owned();
            // The current confirmed query is already on screen.
            let first = usize::from(self.entries[0] == current);
            if first == self.entries.len() {
                return None;
            }
            self.cursor = Some(first);
        } else if previous {
            self.cursor = self.cursor.map(|i| (i + 1).min(self.entries.len() - 1));
        } else if self.cursor == Some(0) {
            self.cursor = None;
            return Some(self.draft.clone());
        } else {
            self.cursor = self.cursor.map(|i| i - 1);
        }
        self.cursor.map(|i| self.entries[i].clone())
    }

    pub fn recall(
        &mut self,
        previous: bool,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        input.update(cx, |state, cx| {
            // Let the input method finish its own candidate selection first.
            if state.marked_text_range(window, cx).is_some() {
                return;
            }
            if let Some(value) = self.step(previous, &state.value()) {
                state.set_value(value, window, cx);
                state.select_all(window, cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use core::prelude::v1::test;

    #[test]
    fn history_is_bounded_deduplicated_and_restores_the_draft() {
        let mut history = InputHistory::default();
        for i in 0..100 {
            history.remember(&format!("query {i}"));
        }
        history.remember("query 98");
        assert_eq!(history.entries.len(), LIMIT);
        assert_eq!(history.step(true, "unfinished"), Some("query 98".into()));
        assert_eq!(history.step(true, "query 98"), Some("query 99".into()));
        assert_eq!(history.step(false, "query 99"), Some("query 98".into()));
        assert_eq!(history.step(false, "query 98"), Some("unfinished".into()));
        assert_eq!(history.step(true, "query 98"), Some("query 99".into()));
        assert_eq!(history.step(true, "edited draft"), Some("query 98".into()));
        history.remember("");
        history.remember(&"x".repeat(MAX_ENTRY_BYTES + 1));
        assert_eq!(history.step(true, &"x".repeat(MAX_ENTRY_BYTES + 1)), None);
        assert_eq!(history.entries.len(), LIMIT);
        let mut replacements = InputHistory::default();
        replacements.remember_replacement("");
        replacements.remember_replacement("value");
        assert_eq!(replacements.step(true, "value"), Some(String::new()));
    }
}
