//! Sending prompts, the event pump that applies an agent's events to its session, and logins.

use super::*;

impl Workbench {
    // ----- sending ------------------------------------------------------------------------

    /// The workspace new sessions run in and the history lists: the window's folder, or the
    /// default workspace.
    pub(in crate::workbench) fn agent_workspace(&self, cx: &App) -> Option<PathBuf> {
        self.root.clone().or_else(|| default_workspace(cx))
    }

    /// The history and search scope chip.
    pub(in crate::workbench) fn agent_scope_label(&self, all_workspaces: bool, cx: &App) -> String {
        match self.agent_workspace(cx) {
            Some(_) if !all_workspaces && self.root.is_none() => DEFAULT_WORKSPACE_NAME.into(),
            Some(_) if !all_workspaces => "本工作区".into(),
            _ => "所有工作区".into(),
        }
    }

    pub(in crate::workbench) fn agent_submit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent.mention.is_some() {
            self.agent_pick_mention(None, window, cx);
            return;
        }
        let text = self.agent.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.agent.current.is_none() {
            self.agent_new_session(None, window, cx);
        }
        let Some(key) = self.agent.current else {
            return;
        };
        let root = self
            .agent
            .session(key)
            .and_then(|s| s.root.clone())
            .or_else(|| self.agent_workspace(cx));
        let Some(root) = root else {
            self.message =
                "找不到数据目录（HOME 未设置），没有默认工作区可用；先打开一个文件夹".into();
            cx.notify();
            return;
        };
        let attachments = std::mem::take(&mut self.agent.attachments);
        let busy = self.agent.session(key).is_some_and(LiveSession::busy);
        if busy {
            self.message = "Agent 正在回复：等这一轮结束，或先点停止".into();
            self.agent.attachments = attachments;
            cx.notify();
            return;
        }
        self.agent
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        let has_client = self.agent.session(key).is_some_and(|s| s.client.is_some());
        if has_client {
            self.agent_prompt(key, text, attachments, window, cx);
        } else {
            if let Some(session) = self.agent.session_mut(key) {
                session.queued = Some((text, attachments));
            }
            self.agent_start_client(key, root, window, cx);
        }
        cx.notify();
    }

    /// Resolves the environment (keychain lookups run `security`) and starts the client off
    /// the UI thread, then sends the queued prompt.
    pub(super) fn agent_start_client(
        &mut self,
        key: u64,
        root: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = cx.global::<crate::settings::Settings>().agent.clone();
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        if session.starting {
            return;
        }
        session.starting = true;
        session.root = Some(root.clone());
        let preset = session.preset.clone();
        let resume = session.resume.clone();
        let overrides = settings.env_for(&preset.id);
        let idle = Duration::from_secs(u64::from(settings.idle_minutes) * 60);
        let buffers = buffer_provider(cx);
        let create = default_workspace(cx).as_ref() == Some(&root);
        let job = cx.background_spawn(async move {
            if create {
                std::fs::create_dir_all(&root)
                    .map_err(|e| format!("无法创建默认工作区 {}：{e}", root.display()))?;
            }
            let env = crate::secrets::resolve(&overrides, |name| std::env::var(name).ok())?;
            let mut options = ClientOptions::new(preset, root);
            options.idle_timeout = idle;
            options.env_overrides = env;
            options.resume_session = resume;
            options.buffers = Some(buffers);
            AgentClient::start(options).map_err(|e| format!("无法启动 Agent：{e}"))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(session) = this.agent.session_mut(key) else {
                    return;
                };
                session.starting = false;
                let queued = session.queued.take();
                match result {
                    Ok(client) => {
                        let client = Arc::new(client);
                        let events = client.events();
                        session.client = Some(client);
                        session.pump = Some(cx.spawn_in(window, async move |this, cx| {
                            while let Ok(first) = events.recv().await {
                                let mut batch = vec![first];
                                while batch.len() < 256 {
                                    match events.try_recv() {
                                        Ok(event) => batch.push(event),
                                        Err(_) => break,
                                    }
                                }
                                if this
                                    .update_in(cx, |this, window, cx| {
                                        this.agent_events(key, batch, window, cx)
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }));
                        if let Some((text, attachments)) = queued {
                            this.agent_prompt(key, text, attachments, window, cx);
                        }
                    }
                    Err(error) => {
                        eprintln!("event=agent_start_failed agent={}", session.preset.id);
                        if let Some((text, attachments)) = queued {
                            let labels = attachments.iter().map(Attachment::label).collect();
                            session.thread.push_user(text, labels, 0);
                            session.thread.apply(
                                &AgentEvent::TurnEnded {
                                    turn: 0,
                                    outcome: workspace_editor_agent::TurnOutcome::Failed(error),
                                },
                                true,
                            );
                        }
                        this.agent_sync_list(false);
                    }
                }
                this.agent_update_spin(window, cx);
                cx.notify();
            });
        })
        .detach();
        self.agent_update_spin(window, cx);
    }

    pub(super) fn agent_prompt(
        &mut self,
        key: u64,
        text: String,
        attachments: Vec<Attachment>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let branch = self.active_branch();
        let store = history(cx);
        let Some(root) = self.agent.session(key).and_then(|s| s.root.clone()) else {
            return;
        };
        let workspace_name = workspace_label(&root, cx);
        let in_default = default_workspace(cx).as_ref() == Some(&root);
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        session.last_active = std::time::Instant::now();
        let Some(client) = session.client.clone() else {
            return;
        };
        let parts = agent_model::prompt_parts(&text, &attachments);
        match client.prompt(parts) {
            Ok(turn) => {
                let labels: Vec<String> = attachments.iter().map(Attachment::label).collect();
                let stored = if labels.is_empty() {
                    text.clone()
                } else {
                    format!(
                        "{}\n{text}",
                        attachments
                            .iter()
                            .map(|a| format!("@{}", a.path().display()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                session.thread.push_user(text.clone(), labels, turn);
                session.turns += 1;
                if session.branch.is_none() {
                    session.branch = branch.clone();
                }
                eprintln!(
                    "event=agent_prompt agent={} turn={turn} attachments={}",
                    session.preset.id,
                    attachments.len()
                );
                // Persist: the first prompt creates the database row.
                if let Some(store) = store {
                    match session.db {
                        Some(id) => {
                            background_history(cx, store, move |h| {
                                h.append_message(id, HistoryRole::User, stored);
                                for attachment in attachments {
                                    h.touch_file(id, attachment.path().display().to_string());
                                }
                            });
                        }
                        None => {
                            session.pending.push((HistoryRole::User, stored));
                            if !session.db_creating {
                                session.db_creating = true;
                                let new = NewSession {
                                    repo: (!in_default)
                                        .then(|| root.file_name())
                                        .flatten()
                                        .map(|n| n.to_string_lossy().into_owned()),
                                    workspace_root: root,
                                    workspace_name: Some(workspace_name),
                                    agent_id: session.preset.id.clone(),
                                    acp_session_id: session.thread.session_id.clone(),
                                    title: session.thread.title.clone(),
                                    first_prompt: Some(text),
                                    branch,
                                    created_at: Some(session.started_at),
                                };
                                let job = cx.background_spawn({
                                    let store = store.clone();
                                    async move { store.create_session(new) }
                                });
                                cx.spawn_in(window, async move |this, cx| {
                                    let created = job.await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.agent_session_created(key, created, cx)
                                    });
                                })
                                .detach();
                            }
                        }
                    }
                }
            }
            Err(error) => {
                self.message = error.to_string();
            }
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    pub(super) fn agent_session_created(
        &mut self,
        key: u64,
        created: workspace_editor_agent_history::Result<SessionId>,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = history(cx) else { return };
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        session.db_creating = false;
        match created {
            Ok(id) => {
                session.db = Some(id);
                let pending = std::mem::take(&mut session.pending);
                let status = agent_model::stored_status(session.thread.status, session.turns > 0);
                session.stored_status = Some(status);
                let title = session.thread.title.clone();
                let acp = session.thread.session_id.clone();
                let files: Vec<String> = session
                    .thread
                    .changed_files
                    .keys()
                    .map(|p| p.display().to_string())
                    .collect();
                background_history(cx, store, move |h| {
                    for (role, text) in pending {
                        h.append_message(id, role, text);
                    }
                    h.set_status(id, status);
                    if let Some(title) = title {
                        h.set_auto_title(id, title);
                    }
                    if acp.is_some() {
                        h.set_acp_session_id(id, acp);
                    }
                    for file in files {
                        h.touch_file(id, file);
                    }
                });
            }
            Err(error) => {
                eprintln!("event=agent_history_create_failed");
                session.pending.clear();
                self.message = format!("会话未能写入历史：{error}");
            }
        }
        cx.notify();
    }

    pub(in crate::workbench) fn agent_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.agent.current.and_then(|key| self.agent.session(key))
            && let Some(client) = &session.client
        {
            client.cancel();
        }
        cx.notify();
    }

    // ----- events -------------------------------------------------------------------------

    pub(super) fn agent_events(
        &mut self,
        key: u64,
        batch: Vec<AgentEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = self.agent.visible
            && self.agent.current == Some(key)
            && self.agent.view == AgentView::Thread
            && window.is_window_active();
        let store = history(cx);
        let mut written = Vec::new();
        let mut recount = false;
        let mut turn_ended = false;
        let mut review_changed = false;
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        session.last_active = std::time::Instant::now();
        let before = session.thread.items.len() + session.thread.dropped;
        let mut touched: Vec<String> = Vec::new();
        let mut title = None;
        let mut acp_id = None;
        for event in &batch {
            session.thread.apply(event, visible);
            match event {
                AgentEvent::PermissionRequested(_) => {
                    // Every request is the user's to answer; the agent keeps its own
                    // "always allow" rules (ADR 0004).
                    eprintln!("event=agent_permission_asked agent={}", session.preset.id);
                }
                AgentEvent::FileWritten { path } => {
                    written.push(path.clone());
                    touched.push(path.display().to_string());
                    recount = true;
                    review_changed = true;
                }
                AgentEvent::ToolCall(call) => {
                    touched.extend(call.locations.iter().map(|l| l.path.display().to_string()));
                }
                AgentEvent::TitleChanged { title: Some(t) } => title = Some(t.clone()),
                AgentEvent::SessionStarted { session_id, .. } => {
                    acp_id = Some(session_id.clone());
                    session.resume = Some(session_id.clone());
                }
                AgentEvent::TurnEnded { .. } => {
                    turn_ended = true;
                    recount = true;
                }
                _ => {}
            }
        }
        // Persist what the thread produced.
        let records: Vec<(HistoryRole, String)> = session
            .thread
            .take_records()
            .iter()
            .map(record_role)
            .collect();
        let status = agent_model::stored_status(session.thread.status, session.turns > 0);
        let status_changed = session.stored_status != Some(status);
        match (session.db, store) {
            (Some(id), Some(store)) => {
                if status_changed {
                    session.stored_status = Some(status);
                }
                if !records.is_empty()
                    || status_changed
                    || !touched.is_empty()
                    || title.is_some()
                    || acp_id.is_some()
                {
                    background_history(cx, store, move |h| {
                        for (role, text) in records {
                            h.append_message(id, role, text);
                        }
                        if status_changed {
                            h.set_status(id, status);
                        }
                        for path in touched {
                            h.touch_file(id, path);
                        }
                        if let Some(title) = title {
                            h.set_auto_title(id, title);
                        }
                        if acp_id.is_some() {
                            h.set_acp_session_id(id, acp_id);
                        }
                    });
                }
            }
            // Kept until the record is created; without a history there is nothing to wait for.
            (None, Some(_)) => session.pending.extend(records),
            (_, None) => {}
        }
        self.agent_parse_from(key, before.saturating_sub(1), cx);
        if recount {
            self.agent_recount(key, Some(written.clone()), window, cx);
        }
        for path in written {
            self.reload_document_from_disk(&path, window, cx);
        }
        // An open review follows the agent's writes as they happen, not only at turn end.
        if turn_ended || review_changed {
            self.agent_reload_review(key, window, cx);
        }
        if turn_ended && self.agent.view == AgentView::History {
            self.agent_reload_history(window, cx);
        }
        if turn_ended {
            self.agent_reclaim_idle(cx);
        }
        self.agent_sync_list(false);
        self.agent_update_spin(window, cx);
        cx.notify();
    }

    /// Parses agent replies from absolute item index `from` on; finished replies are then
    /// highlighted in the background.
    pub(super) fn agent_parse_from(&mut self, key: u64, from: usize, cx: &mut Context<Self>) {
        let theme = gpui_kit::component::Theme::global(cx)
            .highlight_theme
            .clone();
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        let dropped = session.thread.dropped;
        session.md.retain(|&index, _| index >= dropped);
        let mut finished: Vec<(usize, Vec<Block>)> = Vec::new();
        for (i, item) in session.thread.items.iter().enumerate() {
            let index = dropped + i;
            if index < from {
                continue;
            }
            if let Item::Agent { text, streaming } = item {
                // A streaming reply only grows: parse just its unfinished tail.
                let blocks = if *streaming {
                    let (at, parser) = session
                        .streaming_md
                        .get_or_insert_with(|| (index, markdown::Streaming::default()));
                    if *at != index {
                        *at = index;
                        *parser = markdown::Streaming::default();
                    }
                    parser.update(text)
                } else {
                    if session
                        .streaming_md
                        .as_ref()
                        .is_some_and(|(at, _)| *at == index)
                    {
                        session.streaming_md = None;
                    }
                    let parsed = markdown::parse(text);
                    if parsed.iter().any(|b| {
                        matches!(
                            b,
                            Block::Code {
                                language: Some(_),
                                ..
                            }
                        )
                    }) {
                        finished.push((index, parsed.clone()));
                    }
                    markdown::Blocks::from(parsed)
                };
                session.md.insert(index, Rc::new(blocks));
            }
        }
        if finished.is_empty() {
            return;
        }
        let job = cx.background_spawn(async move {
            for (_, blocks) in &mut finished {
                markdown::highlight(blocks, &theme);
            }
            finished
        });
        // Several batches may finish replies; each task only fills in its own indexes.
        let task = cx.spawn(async move |this, cx| {
            let highlighted = job.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this.agent.session_mut(key) {
                    for (index, blocks) in highlighted {
                        if session.md.contains_key(&index) {
                            session
                                .md
                                .insert(index, Rc::new(markdown::Blocks::from(blocks)));
                        }
                    }
                    cx.notify();
                }
            });
        });
        if let Some(session) = self.agent.session_mut(key) {
            match session.highlight_task.take() {
                Some(previous) => {
                    previous.detach();
                    session.highlight_task = Some(task);
                }
                None => session.highlight_task = Some(task),
            }
        }
    }

    /// Re-highlights every reply (the appearance changed).
    pub(in crate::workbench) fn agent_rehighlight(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<u64> = self.agent.sessions.iter().map(|s| s.key).collect();
        for key in keys {
            self.agent_parse_from(key, 0, cx);
        }
    }

    /// Keeps the thread list's item count in step with the current thread.
    pub(in crate::workbench) fn agent_sync_list(&mut self, switched: bool) {
        let Some(session) = self.agent.current() else {
            self.agent.thread_list.reset(0);
            self.agent.list_shape = (0, 0, 0, false);
            return;
        };
        let older = self.agent.older_row();
        let shape = (
            session.key,
            session.thread.dropped,
            session.thread.items.len(),
            older,
        );
        let count = shape.2 + usize::from(older);
        let old = self.agent.list_shape;
        if switched || old.0 != shape.0 || old.1 != shape.1 || old.3 != shape.3 || shape.2 < old.2 {
            self.agent.thread_list.reset(count);
            if switched {
                self.agent.thread_list.set_follow_mode(FollowMode::Tail);
            }
        } else if shape.2 > old.2 {
            let at = old.2 + usize::from(older);
            self.agent.thread_list.splice(at..at, shape.2 - old.2);
            self.agent.thread_list.remeasure();
        } else {
            self.agent.thread_list.remeasure();
        }
        self.agent.list_shape = shape;
    }

    /// Ticks the spinner only while it can be seen.
    pub(in crate::workbench) fn agent_update_spin(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let spinning = self.agent.visible
            && window.is_window_active()
            && self
                .agent
                .sessions
                .iter()
                .any(|s| s.starting || s.thread.status == agent_thread::Status::Running);
        if !spinning {
            self.agent.spin_task = None;
            return;
        }
        if self.agent.spin_task.is_some() {
            return;
        }
        self.agent.spin_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SPIN_FRAME).await;
                if this
                    .update(cx, |this, cx| {
                        this.agent.spin = (this.agent.spin + 1) % SPIN_FRAMES;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    // ----- login ---------------------------------------------------------------------------

    pub(in crate::workbench) fn agent_login(
        &mut self,
        key: u64,
        method: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(client) = self.agent.session_mut(key).and_then(|s| s.client.as_ref()) {
            client.login(method);
        }
        cx.notify();
    }

    /// Restarts the agent with its variables read again from the settings, so a key added
    /// after the session started (e.g. `ANTHROPIC_API_KEY`) is used.
    pub(in crate::workbench) fn agent_retry_login(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(session) = self.agent.session(key) else {
            return;
        };
        let Some(client) = session.client.clone() else {
            return;
        };
        let overrides = cx
            .global::<crate::settings::Settings>()
            .agent
            .env_for(&session.preset.id);
        // `keychain:` may wait for the system's "allow access" dialog.
        let job = cx.background_spawn(async move {
            crate::secrets::resolve(&overrides, |name| std::env::var(name).ok())
        });
        cx.spawn(async move |this, cx| match job.await {
            Ok(env) => client.retry_login(env),
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    if let Some(session) = this.agent.session_mut(key) {
                        session.thread.push_notice(error, true);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}
