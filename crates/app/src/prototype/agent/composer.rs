//! The composer's context: selections, attachments and the @ file picker.

use super::*;

impl Prototype {
    // ----- composer context ---------------------------------------------------------------

    /// ⌘L: the editor's selection (or the whole file) becomes a chip and the composer gets
    /// the focus.
    pub(in crate::prototype) fn agent_add_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let attachment = match self.active {
            Pane::Document(id) => self.documents.iter().find(|d| d.id == id).map(|doc| {
                let editor = doc.editor.read(cx);
                let range = editor.selected_range();
                if range.is_empty() {
                    return Attachment::File(doc.path.clone());
                }
                use gpui_kit::component::input::RopeExt;
                let rope = editor.text();
                let start = rope.offset_to_position(range.start);
                let mut end = rope.offset_to_position(range.end);
                // A selection ending at the start of a line does not include that line.
                if end.character == 0 && end.line > start.line {
                    end.line -= 1;
                }
                Attachment::Selection {
                    path: doc.path.clone(),
                    start: start.line + 1,
                    end: end.line + 1,
                    text: rope.slice(range).to_string(),
                }
            }),
            _ => None,
        };
        if !self.agent.visible {
            self.set_agent_panel(true, window, cx);
        }
        if self.agent.view != AgentView::Thread {
            self.agent_show(AgentView::Thread, window, cx);
        }
        if let Some(attachment) = attachment
            && !self.agent.attachments.contains(&attachment)
        {
            self.agent.attachments.push(attachment);
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(in crate::prototype) fn agent_remove_attachment(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if index < self.agent.attachments.len() {
            self.agent.attachments.remove(index);
        }
        cx.notify();
    }

    pub(in crate::prototype) fn agent_composer_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (text, cursor) = {
            let composer = self.agent.composer.read(cx);
            (composer.value().to_string(), composer.cursor())
        };
        let Some((range, query)) = agent_model::mention_at(&text, cursor) else {
            if self.agent.mention.take().is_some() {
                cx.notify();
            }
            return;
        };
        self.agent.mention_generation += 1;
        let generation = self.agent.mention_generation;
        let index = self.index.clone();
        let show_hidden = self.show_hidden;
        let recent: Vec<PathBuf> = self
            .documents
            .iter()
            .rev()
            .map(|d| d.path.clone())
            .collect();
        let shown_query = query.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let results = match index {
                Some(index) if !query.is_empty() => {
                    cx.background_spawn(async move { index.search(&query, show_hidden).paths })
                        .await
                }
                // No index (no folder, or still building): filter the open files by name.
                _ => {
                    let query = query.to_lowercase();
                    recent
                        .into_iter()
                        .filter(|path| agent_model::file_name(path).to_lowercase().contains(&query))
                        .collect()
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(mention) = this.agent.mention.as_mut()
                    && mention.generation == generation
                {
                    mention.results = results;
                    mention.results.truncate(50);
                    mention.selected = 0;
                    cx.notify();
                }
            });
        });
        let results = self
            .agent
            .mention
            .take()
            .map(|m| m.results)
            .unwrap_or_default();
        self.agent.mention = Some(Mention {
            range,
            query: shown_query,
            results,
            selected: 0,
            generation,
            _task: Some(task),
        });
        cx.notify();
    }

    pub(in crate::prototype) fn agent_move_mention(
        &mut self,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        if let Some(mention) = self.agent.mention.as_mut()
            && !mention.results.is_empty()
        {
            let last = mention.results.len() - 1;
            mention.selected = mention.selected.saturating_add_signed(delta).min(last);
            cx.notify();
        }
    }

    /// Replaces `@query` with a file chip.
    pub(in crate::prototype) fn agent_pick_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mention) = self.agent.mention.take() else {
            return;
        };
        let Some(path) = mention
            .results
            .get(index.unwrap_or(mention.selected))
            .cloned()
        else {
            cx.notify();
            return;
        };
        let text = self.agent.composer.read(cx).value().to_string();
        if mention.range.end <= text.len() {
            let mut next = String::with_capacity(text.len());
            next.push_str(&text[..mention.range.start]);
            next.push_str(&text[mention.range.end..]);
            self.agent
                .composer
                .update(cx, |composer, cx| composer.set_value(next, window, cx));
        }
        let attachment = Attachment::File(path);
        if !self.agent.attachments.contains(&attachment) {
            self.agent.attachments.push(attachment);
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(in crate::prototype) fn agent_close_mention(&mut self, cx: &mut Context<Self>) {
        if self.agent.mention.take().is_some() {
            cx.notify();
        }
    }
}
