//! The composer's context: selections, attachments, the @ path picker and the / command
//! picker.

use super::*;
use gpui_kit::component::{WindowExt as _, input::InlineToken};

impl Workbench {
    // ----- composer context ---------------------------------------------------------------

    pub(super) fn agent_clear_attachments(&mut self, cx: &mut Context<Self>) {
        self.agent.attachments.clear();
        self.agent.image_references.clear();
        self.agent.next_image_reference = 0;
        self.agent.image_generation += 1;
        self.agent.image_preview_task = None;
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent.image_preview_task = None;
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
        let attachments = &self.agent.attachments;
        self.agent.image_references.retain(|_, id| {
            id.is_none_or(|id| {
                attachments
                    .iter()
                    .any(|a| matches!(a, Attachment::Image { id: other, .. } if *other == id))
            })
        });
        self.agent_sync_image_references(window, cx);
        cx.notify();
    }

    fn agent_sync_image_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let labels = agent_model::attachment_labels(&self.agent.attachments);
        let images: HashMap<_, _> = self
            .agent
            .attachments
            .iter()
            .zip(labels)
            .filter_map(|(attachment, label)| match attachment {
                Attachment::Image { id, .. } => Some((*id, label)),
                _ => None,
            })
            .collect();
        self.agent.composer.update(cx, |input, cx| {
            let tokens = input.tokens().to_vec();
            for span in tokens.into_iter().rev() {
                let token = span.token();
                if !token.id().starts_with("image-reference:") {
                    continue;
                }
                let reference = self.agent.image_references.get(token.id().as_ref());
                if matches!(reference, Some(None)) {
                    continue;
                }
                let label = reference.and_then(|id| id.and_then(|id| images.get(&id)));
                if label.is_some_and(|label| token.label().as_ref() == label) {
                    continue;
                }
                let replacement = label.map(|label| {
                    InlineToken::new(token.id().clone(), format!("[{label}]"))
                        .with_label(label.clone())
                });
                let _ = replace_image_reference(input, span.range(), replacement, window, cx);
            }
        });
    }

    pub(in crate::workbench) fn agent_preview_image(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent.image_preview_task.is_some() || window.has_active_dialog(cx) {
            return;
        }
        let Some(Attachment::Image { id, data, .. }) = self.agent.attachments.get(index) else {
            return;
        };
        let id = *id;
        let data = data.clone();
        let generation = self.agent.image_generation;
        let job = cx.background_spawn(async move { crate::agent_images::enlarged(&data) });
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.agent.image_generation != generation {
                    return;
                }
                this.agent.image_preview_task = None;
                let Some(index) =
                    this.agent.attachments.iter().position(
                        |a| matches!(a, Attachment::Image { id: other, .. } if *other == id),
                    )
                else {
                    return;
                };
                let label = agent_model::attachment_labels(&this.agent.attachments)[index].clone();
                match result {
                    Ok(image) if !window.has_active_dialog(cx) => {
                        window.open_dialog(cx, move |dialog, window, cx| {
                            let image = image.clone();
                            let viewport = window.viewport_size();
                            let margin = theme::AGENT_IMAGE_PREVIEW_MARGIN;
                            let width = (viewport.width - margin * 2.)
                                .min(theme::AGENT_IMAGE_PREVIEW_WIDTH);
                            let height = (viewport.height - margin * 4.)
                                .min(theme::AGENT_IMAGE_PREVIEW_HEIGHT);
                            dialog
                                .title(label.clone())
                                .width(width)
                                .margin_top((viewport.height - height - margin * 2.) / 2.)
                                .overlay_closable(true)
                                .child(
                                    div()
                                        .id("agent-enlarged-image")
                                        .test_support()
                                        .w_full()
                                        .h(height)
                                        .bg(theme::colors(cx).editor)
                                        .child(
                                            img(image.clone())
                                                .size_full()
                                                .object_fit(ObjectFit::Contain),
                                        ),
                                )
                                .on_close(move |_, _, cx| image.clone().remove_asset(cx))
                        });
                    }
                    Ok(_) => {}
                    Err(error) => this.message = error,
                }
                cx.notify();
            });
        });
        self.agent.image_preview_task = Some(task);
        cx.notify();
    }

    pub(in crate::workbench) fn agent_paste_images(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
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
                        window,
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
                    self.agent_attach_images(paths.paths().to_vec(), window, cx);
                }
                ClipboardEntry::String(text) => {
                    if let Some(paths) = crate::agent_images::pasted_paths(&text.text) {
                        handled = true;
                        self.agent_attach_images(paths, window, cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for path in paths {
            self.agent_load_image(move || crate::agent_images::from_path(path), window, cx);
        }
    }

    fn agent_load_image(
        &mut self,
        load: impl FnOnce() -> Result<(Attachment, Arc<Image>), String> + Send + 'static,
        window: &mut Window,
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
        // Reserve the insertion point before decoding, so later typing cannot move the image.
        let ordinal = self.agent.next_image_reference + 1;
        let reference = format!("image-reference:{}:{ordinal}", self.agent.image_generation);
        self.agent.next_image_reference = ordinal;
        let label = format!("图 {ordinal}");
        let token = InlineToken::new(reference.clone(), format!("[{label}]")).with_label(label);
        if let Err(error) = self.agent.composer.update(cx, |input, cx| {
            // An image reference is inserted at the caret; selected draft text is retained.
            let selection = input.selected_range();
            let cursor = input.cursor();
            input.set_selected_range(cursor..cursor, cx);
            let result = input.replace_with_token(token, window, cx);
            if result.is_err() {
                input.set_selected_range(selection, cx);
            }
            result
        }) {
            self.message = format!("无法插入图片引用：{error}");
            cx.notify();
            return;
        }
        self.agent.image_references.insert(reference.clone(), None);
        self.agent.images_loading += 1;
        let generation = self.agent.image_generation;
        let job = cx.background_spawn(async move { load() });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.agent.images_loading -= 1;
                if this.agent.image_generation != generation {
                    cx.notify();
                    return;
                }
                let result = result.and_then(|(attachment, preview)| {
                    let total: usize = this
                        .agent
                        .attachments
                        .iter()
                        .map(|a| match a {
                            Attachment::Image { data, .. } => data.len(),
                            _ => 0,
                        })
                        .sum();
                    if let Attachment::Image { id, data, .. } = &attachment
                        && !this.agent.attachments.iter().any(
                            |a| matches!(a, Attachment::Image { id: other, .. } if other == id),
                        )
                        && total + data.len()
                            > crate::md_images::MAX_FILE_BYTES as usize * 4 / 3 + 4
                    {
                        return Err("所有图片的总大小不能超过 16 MB".into());
                    }
                    Ok((attachment, preview))
                });
                match result {
                    Ok((mut attachment, preview)) => {
                        let Attachment::Image {
                            id,
                            ordinal: number,
                            ..
                        } = &mut attachment
                        else {
                            unreachable!()
                        };
                        let id = *id;
                        *number = Some(ordinal);
                        this.agent
                            .image_references
                            .insert(reference.clone(), Some(id));
                        if !this.agent.attachments.iter().any(
                            |a| matches!(a, Attachment::Image { id: other, .. } if *other == id),
                        ) {
                            this.agent.image_previews.insert(id, preview);
                            // Keep paste/drop order even when a later image finishes decoding first.
                            let index = this
                                .agent
                                .attachments
                                .iter()
                                .position(|a| {
                                    matches!(a, Attachment::Image { ordinal: Some(other), .. } if *other > ordinal)
                                })
                                .unwrap_or(this.agent.attachments.len());
                            this.agent.attachments.insert(index, attachment);
                        }
                    }
                    Err(error) => {
                        this.agent.image_references.remove(&reference);
                        this.message = error;
                    }
                }
                this.agent_sync_image_references(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::workbench) fn agent_pick_images(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("添加图片".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = answer.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(paths))) => this.agent_attach_images(paths, window, cx),
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
        self.agent_sync_image_references(window, cx);
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
            let results: std::io::Result<Vec<files::Entry>> = match index {
                Some(index) => {
                    cx.background_spawn(async move {
                        let mut results = index.mention_entries(&query, show_hidden)?;
                        if query.is_empty() {
                            results.splice(
                                0..0,
                                recent.into_iter().map(|path| files::Entry {
                                    path,
                                    directory: false,
                                    symlink: false,
                                }),
                            );
                        }
                        Ok(results)
                    })
                    .await
                }
                // No index (no folder, or still building): filter the open files by name.
                _ => {
                    let query = query.to_lowercase();
                    Ok(recent
                        .into_iter()
                        .filter(|path| agent_model::file_name(path).to_lowercase().contains(&query))
                        .map(|path| files::Entry {
                            path,
                            directory: false,
                            symlink: false,
                        })
                        .collect())
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(mention) = this.agent.mention.as_mut()
                    && mention.generation == generation
                {
                    match results {
                        Ok(results) => mention.results = results,
                        Err(error) => {
                            mention.results.clear();
                            this.message = format!("读取引用目录失败：{error}");
                        }
                    }
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

    /// Replaces `@query` with a file or folder chip.
    pub(in crate::workbench) fn agent_pick_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mention) = self.agent.mention.take() else {
            return;
        };
        let Some(entry) = mention
            .results
            .get(index.unwrap_or(mention.selected))
            .cloned()
        else {
            cx.notify();
            return;
        };
        let text = self.agent.composer.read(cx).value().to_string();
        if mention.range.end <= text.len() {
            self.agent.composer.update(cx, |composer, cx| {
                composer.set_selected_range(mention.range, cx);
                composer.replace("", window, cx);
            });
        }
        if !entry.directory && crate::agent_images::is_image(&entry.path) {
            self.agent_attach_images(vec![entry.path], window, cx);
        } else {
            let attachment = if entry.directory {
                Attachment::Directory(entry.path)
            } else {
                Attachment::File(entry.path)
            };
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
            let trimmed = text.trim_start();
            let word_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            let suffix_len = trimmed[word_end..].trim_start().len();
            self.agent.composer.update(cx, |composer, cx| {
                composer.set_selected_range(0..text.len() - suffix_len, cx);
                composer.replace(&next[..next.len() - suffix_len], window, cx);
                composer.set_selected_range(next.len()..next.len(), cx);
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

/// Keep the user's selection when an attachment changes a reference elsewhere in the draft.
fn replace_image_reference(
    input: &mut TextareaState,
    range: std::ops::Range<usize>,
    replacement: Option<InlineToken>,
    window: &mut Window,
    cx: &mut Context<TextareaState>,
) -> Result<(), gpui_kit::component::input::InlineTokenError> {
    let mut selection = input.selected_range();
    let len = replacement.as_ref().map_or(0, |token| token.text().len());
    if let Some(token) = replacement {
        input.replace_range_with_token(range.clone(), token, window, cx)?;
    } else {
        input.set_selected_range(range.clone(), cx);
        input.replace("", window, cx);
    }
    for offset in [&mut selection.start, &mut selection.end] {
        if *offset >= range.end {
            *offset = offset.saturating_add_signed(len as isize - range.len() as isize);
        } else if *offset > range.start {
            *offset = range.start + len;
        }
    }
    input.set_selected_range(selection, cx);
    Ok(())
}
