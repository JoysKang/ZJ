//! Session history: the list, filters, opening stored sessions, older pages and pin / archive / rename / delete.

use super::*;

impl Workbench {
    // ----- history ------------------------------------------------------------------------

    pub(in crate::workbench) fn agent_reload_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = history(cx) else {
            self.agent.history.error = Some("没有可用的数据目录，会话历史不会保存".into());
            return;
        };
        let filter = self.agent.history.filter.clone();
        let root = self.agent_workspace(cx);
        self.agent.history.generation += 1;
        let generation = self.agent.history.generation;
        let job = cx.background_spawn(async move {
            use workspace_editor_agent_history::{Archived, Filter, Scope};
            let scope = match (&root, filter.all_workspaces) {
                (Some(root), false) => Scope::Workspace(root.clone()),
                _ => Scope::All,
            };
            let base = Filter {
                agent_id: filter.agent.clone(),
                archived: if filter.archived {
                    Archived::Only
                } else {
                    Archived::Exclude
                },
                ..Default::default()
            };
            let rows = store.list(&scope, &base, 2000)?;
            let count = |scope: &Scope, archived: Archived| {
                store.count(
                    scope,
                    &Filter {
                        archived,
                        ..Default::default()
                    },
                )
            };
            let here = match &root {
                Some(root) => count(&Scope::Workspace(root.clone()), Archived::Exclude)?,
                None => 0,
            };
            let all = count(&Scope::All, Archived::Exclude)?;
            let archived = count(&scope, Archived::Only)?;
            Ok::<_, workspace_editor_agent_history::Error>((rows, (here, all, archived)))
        });
        self.agent.history.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.agent.history.generation != generation {
                    return;
                }
                match result {
                    Ok((rows, counts)) => {
                        this.agent.history.rows = rows;
                        this.agent.history.counts = counts;
                        this.agent.history.error = None;
                    }
                    Err(error) => this.agent.history.error = Some(error.to_string()),
                }
                this.agent.history.loaded = true;
                this.agent_regroup_history();
                cx.notify();
            });
        }));
    }

    /// Applies the live filters (进行中 / 待处理) and groups the rows.
    pub(in crate::workbench) fn agent_regroup_history(&mut self) {
        let now = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now);
        let filter = self.agent.history.filter.clone();
        let live: HashMap<SessionId, agent_model::RowStatus> = self
            .agent
            .sessions
            .iter()
            .filter_map(|s| Some((s.db?, s.row_status())))
            .collect();
        let keep = |s: &SessionSummary| {
            let status = live
                .get(&s.id)
                .copied()
                .unwrap_or(agent_model::RowStatus::None);
            (!filter.running || status == agent_model::RowStatus::Running)
                && (!filter.pending
                    || matches!(
                        status,
                        agent_model::RowStatus::Awaiting | agent_model::RowStatus::Unread
                    ))
        };
        let visible: Vec<SessionSummary> = self
            .agent
            .history
            .rows
            .iter()
            .filter(|s| keep(s))
            .cloned()
            .collect();
        self.agent.history.grouped = agent_model::group_rows(&visible, now, offset);
        self.agent.history.rows = visible
            .into_iter()
            .chain(self.agent.history.rows.iter().filter(|s| !keep(s)).cloned())
            .collect();
        self.agent
            .history
            .list
            .reset(self.agent.history.grouped.len());
    }

    pub(in crate::workbench) fn agent_set_filter(
        &mut self,
        change: impl FnOnce(&mut ListFilter),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let before = self.agent.history.filter.clone();
        change(&mut self.agent.history.filter);
        let after = &self.agent.history.filter;
        if before.all_workspaces != after.all_workspaces
            || before.agent != after.agent
            || before.archived != after.archived
        {
            self.agent_reload_history(window, cx);
        } else {
            self.agent_regroup_history();
        }
        cx.notify();
    }

    /// Opens a stored session: switches to it if it is live in this window, otherwise loads
    /// its last messages (the agent restarts with `session/load` on the next prompt).
    pub(in crate::workbench) fn agent_open_stored(
        &mut self,
        id: SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = self
            .agent
            .sessions
            .iter()
            .find(|s| s.db == Some(id))
            .map(|s| s.key)
        {
            self.agent_select(key, window, cx);
            return;
        }
        let Some(store) = history(cx) else { return };
        let job = cx.background_spawn(async move {
            let summary = store
                .session(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "会话已不存在".to_string())?;
            let count = store.message_count(id).map_err(|e| e.to_string())?;
            let page = store
                .messages_page(id, None, HISTORY_PAGE)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((summary, count, page))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok((summary, count, page)) => this.agent_restore(summary, count, page, window, cx),
                Err(error) => {
                    this.message = format!("无法打开会话：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn agent_restore(
        &mut self,
        summary: SessionSummary,
        count: usize,
        page: Vec<workspace_editor_agent_history::Message>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preset = self
            .agent
            .preset(&summary.agent_id)
            .cloned()
            .or_else(|| AgentPreset::find_builtin(&summary.agent_id));
        let Some(preset) = preset else {
            self.message = format!("找不到 Agent「{}」，无法继续这个会话", summary.agent_id);
            cx.notify();
            return;
        };
        let key = self.agent.next_key;
        self.agent.next_key += 1;
        let mut session = LiveSession::new(key, preset);
        session.db = Some(summary.id);
        session.started_at = summary.created_at;
        session.branch = summary.branch.clone();
        session.root = Some(summary.workspace_root.clone());
        session.resume = summary.acp_session_id.clone();
        session.thread.session_id = summary.acp_session_id.clone();
        session.thread.title = Some(summary.title.clone());
        session.oldest_seq = page.first().map(|m| m.seq);
        session.turns = page.iter().filter(|m| m.role == HistoryRole::User).count();
        session.stored_status = Some(summary.status);
        let items = agent_model::items_from_messages(&page);
        session.thread.load(items, count.saturating_sub(page.len()));
        // `load` counts older rows as dropped; the history row says how to get them.
        session.thread.dropped = 0;
        self.agent.sessions.push(session);
        self.agent.current = Some(key);
        self.agent_parse_from(key, 0, cx);
        self.agent_sync_list(true);
        self.agent_show(AgentView::Thread, window, cx);
    }

    /// "加载更早的消息": the previous page from the database.
    pub(in crate::workbench) fn agent_load_older(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.agent.current else {
            return;
        };
        let Some(store) = history(cx) else { return };
        let Some(session) = self.agent.session(key) else {
            return;
        };
        let (Some(id), Some(before)) = (session.db, session.oldest_seq) else {
            // A live thread longer than the memory cap. Reopening it from history replaces
            // the session, so only when that loses nothing: not while its agent runs, nor
            // while its changes can still be reviewed (the snapshots live in the client).
            if session.client.is_some() || !session.thread.changed_files.is_empty() {
                self.message =
                    "会话进行中，较早的消息请用 ⌘J 搜索；结束并审阅完改动后可从历史重新打开".into();
                cx.notify();
                return;
            }
            if let Some(id) = session.db {
                self.agent.sessions.retain(|s| s.key != key || s.busy());
                if self.agent.session(key).is_none() {
                    self.agent.current = None;
                    self.agent_open_stored(id, window, cx);
                }
            }
            return;
        };
        // Older pages never push the newest items out of memory.
        let room = agent_thread::MAX_ITEMS.saturating_sub(session.thread.items.len());
        if room == 0 {
            self.message = "这个会话在内存里的消息已到上限；更早的内容可在 ⌘J 搜索里找到".into();
            cx.notify();
            return;
        }
        let limit = HISTORY_PAGE.min(room);
        let job = cx.background_spawn(async move { store.messages_page(id, Some(before), limit) });
        cx.spawn_in(window, async move |this, cx| {
            let page = match job.await {
                Ok(page) => page,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.message = format!("没能读取更早的消息：{error}");
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| {
                if let Some(session) = this.agent.session_mut(key) {
                    session.oldest_seq = page.first().map(|m| m.seq).or(Some(1));
                    session
                        .thread
                        .prepend(agent_model::items_from_messages(&page));
                    session.thread.dropped = 0;
                    session.md.clear();
                }
                this.agent_parse_from(key, 0, cx);
                this.agent_sync_list(true);
                this.agent.thread_list.set_follow_mode(FollowMode::Normal);
                this.agent.thread_list.scroll_to(ListOffset {
                    item_ix: page.len(),
                    offset_in_item: Pixels::ZERO,
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn agent_history_op(
        &mut self,
        op: HistoryOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = history(cx) else { return };
        if let HistoryOp::Delete(id, title) = &op {
            let id = *id;
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("永久删除会话「{title}」？"),
                Some("会话的消息、文件记录和搜索索引都会从本机删除，无法恢复。"),
                &crate::workbench::prompt_buttons(&["删除", "取消"]),
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if answer.await != Ok(0) {
                    return;
                }
                let job = cx.background_spawn(async move { store.delete_session(id) });
                let result = job.await;
                let _ = this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok(_) => {
                            for session in
                                this.agent.sessions.iter_mut().filter(|s| s.db == Some(id))
                            {
                                session.db = None;
                            }
                            this.message = "会话已删除".into();
                        }
                        Err(error) => this.message = format!("删除失败：{error}"),
                    }
                    this.agent_reload_history(window, cx);
                });
            })
            .detach();
            return;
        }
        let job = cx.background_spawn(async move {
            match op {
                HistoryOp::Pin(id) => store.pin(id),
                HistoryOp::Unpin(id) => store.unpin(id),
                HistoryOp::MovePin(id, before) => store.move_pin(id, before),
                HistoryOp::Archive(id, archived) => store.set_archived(id, archived),
                HistoryOp::Rename(id, title) => store.rename(id, title),
                HistoryOp::DeleteArchived(scope) => {
                    store.delete_archived(&scope).map(|_| ())?;
                }
                HistoryOp::Delete(..) => {}
            }
            store.flush()
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    this.message = format!("会话历史未能更新：{error}");
                }
                this.agent_reload_history(window, cx);
            });
        })
        .detach();
    }

    pub(in crate::workbench) fn agent_start_rename(
        &mut self,
        id: SessionId,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let title = input.read(cx).value().trim().to_string();
                    this.agent.history.renaming = None;
                    if !title.is_empty() {
                        for session in this.agent.sessions.iter_mut().filter(|s| s.db == Some(id)) {
                            session.thread.title = Some(title.clone());
                        }
                        this.agent_history_op(HistoryOp::Rename(id, title), window, cx);
                    }
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.agent.history.renaming = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        self.agent.history.renaming = Some((id, input, subscription));
        cx.notify();
    }

    pub(in crate::workbench) fn agent_delete_archived(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use workspace_editor_agent_history::Scope;
        let scope = match (
            self.agent_workspace(cx),
            self.agent.history.filter.all_workspaces,
        ) {
            (Some(root), false) => Scope::Workspace(root),
            _ => Scope::All,
        };
        let count = self.agent.history.counts.2;
        if count == 0 {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("永久删除 {count} 个已归档会话？"),
            Some("删除后释放磁盘空间，无法恢复。"),
            &crate::workbench::prompt_buttons(&["删除", "取消"]),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.agent_history_op(HistoryOp::DeleteArchived(scope), window, cx)
                });
            }
        })
        .detach();
    }
}

pub(in crate::workbench) enum HistoryOp {
    Pin(SessionId),
    Unpin(SessionId),
    MovePin(SessionId, Option<SessionId>),
    Archive(SessionId, bool),
    Rename(SessionId, String),
    Delete(SessionId, String),
    DeleteArchived(workspace_editor_agent_history::Scope),
}

type HistoryJob = Box<dyn FnOnce(&History) + Send>;

/// One queue for every history write of the app, run in order by a single background task:
/// separate tasks per batch could reach the writer out of order (a reply's chunks swapped, a
/// "running" status landing after "completed").
struct HistoryQueue {
    jobs: async_channel::Sender<(
        Arc<History>,
        HistoryJob,
        async_channel::Sender<Option<String>>,
    )>,
    _task: Task<()>,
}

impl Global for HistoryQueue {}

/// Queues a history write off the UI thread, after every write queued before it. A failed
/// write (reported by the writer thread a little later) shows in the status bar of the
/// window that queued it instead of being dropped.
pub(super) fn background_history(
    cx: &mut Context<Workbench>,
    store: Arc<History>,
    work: impl FnOnce(&History) + Send + 'static,
) {
    if !cx.has_global::<HistoryQueue>() {
        let (jobs, incoming) = async_channel::unbounded::<(
            Arc<History>,
            HistoryJob,
            async_channel::Sender<Option<String>>,
        )>();
        let task = cx.background_spawn(async move {
            while let Ok((store, work, done)) = incoming.recv().await {
                work(&store);
                let _ = done.try_send(store.take_error());
            }
        });
        cx.set_global(HistoryQueue { jobs, _task: task });
    }
    let (done, result) = async_channel::bounded(1);
    let _ = cx
        .global::<HistoryQueue>()
        .jobs
        .try_send((store, Box::new(work), done));
    cx.spawn(async move |this, cx| {
        if let Ok(Some(error)) = result.recv().await {
            let _ = this.update(cx, |this, cx| {
                this.message = format!("Agent 会话记录没能保存：{error}");
                cx.notify();
            });
        }
    })
    .detach();
}
