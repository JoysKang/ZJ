//! Local variable tinting (the bindings come from [`crate::symbols`]): a binding's definition
//! and every reference that resolves to it share one color, so two same-named locals read
//! apart. Rebuilt in the background after typing pauses, for the visible document only; while a
//! parse is pending the previous decorations keep tracking edits on their own.

use super::*;
use crate::symbols::{self, Binding};
use gpui_kit::{
    HighlightStyle,
    component::input::{TextDecoration, TextDecorationCollection},
};

/// How long typing pauses before the bindings are parsed again.
const TINT_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
pub(super) struct LocalDoc {
    /// The text version the decorations were built from.
    version: Option<u64>,
    decorations: Option<TextDecorationCollection>,
    task: Option<Task<()>>,
}

impl Document {
    /// What the local tint last did: the text version it ran for, and whether it painted
    /// anything (tests).
    #[cfg(test)]
    pub(super) fn locals_tint(&self) -> (Option<u64>, bool) {
        (self.locals.version, self.locals.decorations.is_some())
    }
}

impl Workbench {
    /// Tints the active document's local bindings, once typing has paused. Only the visible
    /// document is parsed; the others are tinted when their tab is shown.
    pub(super) fn tint_locals(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_document_id() else {
            return;
        };
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        if doc.locals.version == Some(doc.version) {
            return;
        }
        // Replacing the task drops the pending one, so each edit restarts the pause. The text is
        // copied inside the task, after the pause, not on the keystroke itself.
        doc.locals.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TINT_DELAY).await;
            let prepared = this.update(cx, |this, cx| {
                let doc = this.document(id)?;
                let language = crate::languages::for_path(&doc.path).0;
                let text = doc.editor.read(cx).text().to_string();
                Some((doc.version, language, text))
            });
            let Ok(Some((version, language, text))) = prepared else {
                return;
            };
            let bindings = cx
                .background_spawn(async move { symbols::bindings(language, &text) })
                .await;
            let _ = this.update(cx, |this, cx| this.show_bindings(id, version, bindings, cx));
        }));
    }

    fn show_bindings(
        &mut self,
        id: DocumentId,
        version: u64,
        bindings: Vec<Binding>,
        cx: &mut Context<Self>,
    ) {
        let colors = theme::colors(cx);
        let Some(doc) = self.document_mut(id).filter(|doc| doc.version == version) else {
            return;
        };
        let mut decorations = Vec::new();
        for (index, binding) in bindings.iter().enumerate() {
            let color = colors.bindings[index % colors.bindings.len()];
            for range in std::iter::once(&binding.definition).chain(&binding.references) {
                decorations.push(TextDecoration::new(
                    range.clone(),
                    HighlightStyle {
                        color: Some(color),
                        ..Default::default()
                    },
                ));
            }
        }
        doc.locals.version = Some(version);
        match &doc.locals.decorations {
            Some(collection) => collection.set(decorations, cx),
            None if !decorations.is_empty() => {
                let collection = doc.editor.update(cx, |state, cx| {
                    state.create_decorations_collection(decorations, cx)
                });
                doc.locals.decorations = Some(collection);
            }
            None => {}
        }
    }
}
