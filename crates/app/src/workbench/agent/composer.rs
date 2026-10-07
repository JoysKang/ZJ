//! The composer's context: selections, attachments, the @ file picker and the / command
//! picker.

use super::*;

impl Workbench {
    // ----- composer context ---------------------------------------------------------------

    pub(super) fn agent_clear_attachments(&mut self, cx: &mut Context<Self>) {
        self.agent.attachments.clear();
        self.agent.image_generation += 1;
        for (_, preview) in self.agent.image_previews.drain() {
            preview.remove_asset(cx);
        }
    }

    /// ⌘L: the editor's selection (or the whole file) becomes a chip and the composer gets
    /// the focus.
    pub(in crate::workbench) fn agent_add_selection(
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

    pub(in crate::workbench) fn agent_remove_attachment(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if index < self.agent.attachments.len()
            && let Attachment::Image { id, .. } = self.agent.attachments.remove(index)
            && !self
                .agent
                .attachments
                .iter()
                .any(|a| matches!(a, Attachment::Image { id: other, .. } if *other == id))
            && let Some(preview) = self.agent.image_previews.remove(&id)
        {
            preview.remove_asset(cx);
        }
        cx.notify();
    }

    pub(in crate::workbench) fn agent_paste_images(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return false;
        };
        let mut handled = false;
        for entry in clipboard.entries {
            match entry {
                ClipboardEntry::Image(image) => {
                    handled = true;
                    let name = format!("粘贴的图片.{}", image.format.extension());
                    self.agent_load_image(
                        move || crate::agent_images::from_bytes(name, None, image.bytes),
                        cx,
                    );
                }
                ClipboardEntry::ExternalPaths(paths)
                    if paths
                        .paths()
                        .iter()
                        .all(|p| crate::agent_images::is_image(p)) =>
                {
                    handled = true;
                    self.agent_attach_images(paths.paths().to_vec(), cx);
                }
                ClipboardEntry::String(text) => {
                    if let Some(paths) = crate::agent_images::pasted_paths(&text.text) {
                        handled = true;
                        self.agent_attach_images(paths, cx);
                    }
                }
                _ => {}
            }
        }
        handled
    }

    pub(in crate::workbench) fn agent_attach_images(
        &mut self,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            self.agent_load_image(move || crate::agent_images::from_path(path), cx);
        }
    }

    fn agent_load_image(
        &mut self,
        load: impl FnOnce() -> Result<(Attachment, Arc<Image>), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let count = self
            .agent
            .attachments
            .iter()
            .filter(|a| matches!(a, Attachment::Image { .. }))
            .count();
        if count + self.agent.images_loading >= crate::agent_images::MAX_IMAGES {
            self.message = format!("最多添加 {} 张图片", crate::agent_images::MAX_IMAGES);
            cx.notify();
            return;
        }
        self.agent.images_loading += 1;
        let generation = self.agent.image_generation;
        let job = cx.background_spawn(async move { load() });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.agent.images_loading -= 1;
                if this.agent.image_generation != generation {
                    cx.notify();
                    return;
                }
                match result {
                    Ok((attachment, preview)) => {
                        if this.agent.attachments.contains(&attachment) {
                            cx.notify();
                            return;
                        }
                        let total: usize = this
                            .agent
                            .attachments
                            .iter()
                            .map(|a| match a {
                                Attachment::Image { data, .. } => data.len(),
                                _ => 0,
                            })
                            .sum();
                        if let Attachment::Image { id, data, .. } = &attachment {
                            if total + data.len()
                                > crate::md_images::MAX_FILE_BYTES as usize * 4 / 3 + 4
                            {
                                this.message = "所有图片的总大小不能超过 16 MB".into();
                                cx.notify();
                                return;
                            }
                            this.agent.image_previews.insert(*id, preview);
                        }
                        if !this.agent.attachments.contains(&attachment) {
                            this.agent.attachments.push(attachment);
                        }
                    }
                    Err(error) => this.message = error,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn agent_pick_images(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("添加图片".into()),
        });
        cx.spawn(async move |this, cx| {
            let result = answer.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(Some(paths))) => this.agent_attach_images(paths, cx),
                Ok(Ok(None)) => {}
                Ok(Err(e)) => {
                    this.message = format!("无法选择图片：{e}");
                    cx.notify();
                }
                Err(e) => {
                    this.message = format!("无法选择图片：{e}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn agent_composer_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (text, cursor) = {
            let composer = self.agent.composer.read(cx);
            (composer.value().to_string(), composer.cursor())
        };
        let slash = agent_model::slash_at(&text, cursor).map(|_| 0);
        if slash != self.agent.slash {
            if slash.is_some() && self.agent_commands(cx).is_empty() {
                self.agent_warm_up(window, cx);
            }
            self.agent.slash = slash;
            cx.notify();
        }
        let Some((range, query)) = agent_model::mention_at(&text, cursor) else {
            if self.agent.mention.take().is_some() {
                cx.notify();
            }
            return;
        };
        self.agent.mention_generation += 1;
        let generation = self.agent.mention_generation;
        let index = self.index.clone();
        let show_hidden = self.explorer.show_hidden;
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

    pub(in crate::workbench) fn agent_move_mention(
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
    pub(in crate::workbench) fn agent_pick_mention(
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
        if crate::agent_images::is_image(&path) {
            self.agent_attach_images(vec![path], cx);
        } else {
            let attachment = Attachment::File(path);
            if !self.agent.attachments.contains(&attachment) {
                self.agent.attachments.push(attachment);
            }
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn agent_close_mention(&mut self, cx: &mut Context<Self>) {
        if self.agent.mention.take().is_some() {
            cx.notify();
        }
    }

    /// The current session's commands, or the ones its agent last listed in this workspace
    /// while it has not started.
    pub(in crate::workbench) fn agent_commands(&self, cx: &App) -> &[AgentCommand] {
        let session = self.agent.current();
        if let Some(session) = session
            && !session.thread.commands.is_empty()
        {
            return &session.thread.commands;
        }
        let preset = session.map_or(&self.agent.agent_id, |s| &s.preset.id);
        session
            .and_then(|s| s.root.clone())
            .or_else(|| self.agent_workspace(cx))
            .and_then(|root| self.agent.known_commands.get(&(preset.clone(), root)))
            .map_or(&[], Vec::as_slice)
    }

    /// `/` with no commands to list: the agent starts now (instead of with the first message)
    /// and sends them.
    fn agent_warm_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.current.is_none() {
            self.agent_new_session(None, window, cx);
        }
        let Some(key) = self.agent.current else {
            return;
        };
        let root = self
            .agent
            .session(key)
            .filter(|s| s.client.is_none() && !s.starting)
            .and_then(|s| s.root.clone().or_else(|| self.agent_workspace(cx)));
        if let Some(root) = root {
            self.agent_start_client(key, root, window, cx);
        }
    }

    /// The current agent's commands matching the `/command` being typed (as many as are
    /// shown).
    pub(in crate::workbench) fn agent_slash_matches(&self, cx: &App) -> Vec<AgentCommand> {
        let composer = self.agent.composer.read(cx);
        let text = composer.value();
        let Some(query) = agent_model::slash_at(&text, composer.cursor()) else {
            return Vec::new();
        };
        agent_model::slash_matches(self.agent_commands(cx), query)
            .into_iter()
            .take(theme::AGENT_MENTION_ROWS)
            .cloned()
            .collect()
    }

    pub(in crate::workbench) fn agent_move_slash(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.agent_slash_matches(cx).len();
        if let Some(selected) = self.agent.slash.as_mut()
            && count > 0
        {
            *selected = selected.saturating_add_signed(delta).min(count - 1);
            cx.notify();
        }
    }

    /// Replaces the first word with the picked `/command`.
    pub(in crate::workbench) fn agent_pick_slash(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selected) = self.agent.slash.take() else {
            return;
        };
        if let Some(command) = self.agent_slash_matches(cx).get(index.unwrap_or(selected)) {
            let text = self.agent.composer.read(cx).value().to_string();
            let next = agent_model::with_command(&text, &command.name);
            let end = next.len();
            self.agent.composer.update(cx, |composer, cx| {
                composer.set_value(next, window, cx);
                composer.set_selected_range(end..end, cx);
            });
            self.agent.slash = None;
        }
        self.agent_focus_composer(window, cx);
        cx.notify();
    }

    pub(in crate::workbench) fn agent_close_slash(&mut self, cx: &mut Context<Self>) {
        if self.agent.slash.take().is_some() {
            cx.notify();
        }
    }
}
