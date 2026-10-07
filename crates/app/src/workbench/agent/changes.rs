//! Files an agent changed: line counts against the snapshot, saves, and accepting or reverting them.

use super::*;
use crate::workbench::diff_view::DiffList;

impl Workbench {
    // ----- changed files ------------------------------------------------------------------

    /// The user saved `path`: sessions that changed it recount, and an open review of it
    /// reloads (it compares the snapshot with the file on disk).
    pub(in crate::workbench) fn agent_file_saved(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<u64> = self
            .agent
            .sessions
            .iter()
            .filter(|s| {
                s.thread.changed_files.contains_key(path)
                    || s.client
                        .as_ref()
                        .is_some_and(|c| c.snapshot(path).is_some())
            })
            .map(|s| s.key)
            .collect();
        for key in keys {
            self.agent_recount(key, Some(vec![path.to_path_buf()]), window, cx);
            self.agent_reload_review(key, window, cx);
        }
    }

    /// Recomputes line counts against the review base, in the background: of `only` (the
    /// files just written or saved), or of every changed file. Requests made while a recount
    /// runs wait for it and go together.
    pub(in crate::workbench) fn agent_recount(
        &mut self,
        key: u64,
        only: Option<Vec<PathBuf>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = history(cx);
        let Some(session) = self.agent.session_mut(key) else {
            return;
        };
        match (&mut session.recount_pending, only) {
            (Some(pending), Some(only)) => pending.extend(only),
            (pending, _) => *pending = None,
        }
        if session.stats_task.is_some() {
            return;
        }
        let Some(client) = session.client.clone() else {
            return;
        };
        let paths: Vec<PathBuf> = match session.recount_pending.replace(Default::default()) {
            Some(pending) if pending.is_empty() => return,
            Some(pending) => pending.into_iter().collect(),
            None => {
                let mut paths: Vec<PathBuf> =
                    session.thread.changed_files.keys().cloned().collect();
                paths.extend(client.snapshot_paths());
                paths.sort();
                paths.dedup();
                paths
            }
        };
        let db = session.db;
        let job = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|path| {
                    let change = review_texts(&client, &path).map(|(before, after)| {
                        let (added, removed) = line_counts(before.as_deref().unwrap_or(""), &after);
                        FileChange {
                            added,
                            removed,
                            new_file: before.is_none(),
                        }
                    });
                    (path, change.filter(|c| c.added + c.removed > 0))
                })
                .collect::<Vec<_>>()
        });
        session.stats_task = Some(cx.spawn_in(window, async move |this, cx| {
            let changes = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(session) = this.agent.session_mut(key) else {
                    return;
                };
                session.stats_task = None;
                for (path, change) in changes {
                    session.thread.set_file_change(path, change);
                }
                let (added, removed) = session
                    .thread
                    .changed_files
                    .values()
                    .fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
                if let (Some(id), Some(store)) = (db, store) {
                    background_history(cx, store, move |h| {
                        h.set_line_counts(id, added as i64, removed as i64)
                    });
                }
                // What came in meanwhile.
                this.agent_recount(key, Some(Vec::new()), window, cx);
                cx.notify();
            });
        }));
    }

    /// Accepts or rejects every changed file of the current session.
    pub(in crate::workbench) fn agent_resolve_all(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.agent.current else {
            return;
        };
        let paths: Vec<PathBuf> = self
            .agent
            .session(key)
            .map(|s| s.thread.changed_files.keys().cloned().collect())
            .unwrap_or_default();
        if paths.is_empty() {
            return;
        }
        if accept {
            self.agent_resolve_files(key, paths, true, window, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("拒绝 Agent 对 {} 个文件的全部修改？", paths.len()),
            Some("已写入磁盘的修改会被还原成 Agent 改动之前的内容；尚未接受的建议会被丢弃。"),
            &crate::workbench::prompt_buttons(&["全部拒绝", "取消"]),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.agent_resolve_files(key, paths, false, window, cx)
                });
            }
        })
        .detach();
    }

    pub(in crate::workbench) fn agent_resolve_files(
        &mut self,
        key: u64,
        paths: Vec<PathBuf>,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.agent.session(key).and_then(|s| s.client.clone()) else {
            return;
        };
        let file_generation = self.file_generation;
        let job = cx.background_spawn({
            let paths = paths.clone();
            async move {
                let mut errors = Vec::new();
                let mut resolved = Vec::new();
                for path in &paths {
                    let source = if accept {
                        Some(DiffList::Modified)
                    } else {
                        client.snapshot(path).flatten().map(|_| DiffList::Original)
                    };
                    if let Err(e) = resolve_file(&client, path, accept) {
                        errors.push(e);
                    } else {
                        resolved.push((path.clone(), source));
                    }
                }
                (errors, resolved)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let (errors, resolved) = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(first) = errors.first() {
                    this.message = first.clone();
                }
                for path in &paths {
                    this.reload_document_from_disk(path, window, cx);
                }
                for (path, source) in resolved {
                    this.agent_finish_resolved_review(
                        key,
                        &path,
                        file_generation,
                        source,
                        window,
                        cx,
                    );
                }
                this.agent_recount(key, Some(paths.clone()), window, cx);
                this.agent_reload_review(key, window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// An open editor follows a file the agent (or a review) changed on disk: silently when it
    /// has no edits, or when its edits are what the agent read; otherwise the banner asks.
    pub(in crate::workbench) fn reload_document_from_disk(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.documents.iter().find(|d| d.path == path).map(|d| d.id) {
            self.follow_agent_write(id, window, cx);
        }
    }
}
