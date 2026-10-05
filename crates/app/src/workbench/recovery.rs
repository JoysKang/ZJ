//! Edit recovery in the workbench (see `crate::recovery`): snapshots of edited buffers, 2 s
//! after the last edit, through one background queue; forgetting them once a buffer is saved,
//! reloaded or discarded; and restoring what an abnormal exit left behind.
//!
//! The queue is an app global installed at launch (`install`); without it (most tests)
//! nothing is written. No timer runs unless an edit is waiting to be snapshotted.

use super::*;
use crate::recovery::{self, Op, Record};

/// The background queue that writes and removes snapshots in order.
pub(crate) struct RecoveryQueue {
    ops: async_channel::Sender<Op>,
    dir: PathBuf,
    /// Held while an operation runs; `true` once the app quits, after which the queue writes
    /// nothing (a removal at quit must not be undone by a write still queued).
    closed: Arc<Mutex<bool>>,
    /// The first failure since it was last shown (written by the queue, shown by a window).
    error: Arc<Mutex<Option<String>>>,
    _task: Task<()>,
}

impl Global for RecoveryQueue {}

/// Snapshots found at launch, waiting for a window to take them.
#[derive(Default)]
pub(crate) struct PendingRecovery(pub Vec<Record>);

impl Global for PendingRecovery {}

/// Starts the queue for snapshots in `dir`, and hands the snapshots already there (an abnormal
/// exit) to the windows that open next.
pub(crate) fn install(dir: PathBuf, cx: &mut App) {
    let (records, errors) = recovery::load_all(&dir);
    for error in &errors {
        eprintln!("event=recovery_unreadable error={error}");
    }
    if !records.is_empty() {
        eprintln!("event=recovery_found count={}", records.len());
    }
    cx.set_global(PendingRecovery(records));
    let (ops, incoming) = async_channel::unbounded::<Op>();
    let error = Arc::new(Mutex::new(None));
    let failed = error.clone();
    let closed = Arc::new(Mutex::new(false));
    let gate = closed.clone();
    let queue_dir = dir.clone();
    let task = cx.background_spawn(async move {
        while let Ok(op) = incoming.recv().await {
            let closed = gate.lock().unwrap();
            if *closed {
                continue;
            }
            if let Err(e) = recovery::apply(&queue_dir, &op) {
                eprintln!("event=recovery_write_failed");
                failed.lock().unwrap().get_or_insert(e.to_string());
            }
        }
    });
    cx.set_global(RecoveryQueue {
        ops,
        dir,
        closed,
        error,
        _task: task,
    });
}

fn send(cx: &App, op: Op) {
    if let Some(queue) = cx.try_global::<RecoveryQueue>() {
        let _ = queue.ops.try_send(op);
    }
}

/// Removes one snapshot (for callers that hold a document borrow).
pub(super) fn remove_snapshot(key: String, cx: &App) {
    send(cx, Op::Remove(key));
}

/// Quitting after every window answered: nothing of this session needs recovering. Runs
/// before `cx.quit()`, synchronously: the queue may not get to run again.
pub(crate) fn forget_everything(cx: &mut App) {
    let Some(queue) = cx.try_global::<RecoveryQueue>() else {
        return;
    };
    let (dir, closed) = (queue.dir.clone(), queue.closed.clone());
    let mut keys = Vec::new();
    if let Some(owners) = cx.try_global::<OpenDocuments>().map(|o| o.0.clone()) {
        let mut views: Vec<Entity<Workbench>> = Vec::new();
        for owner in owners.borrow().values() {
            if let Some(view) = owner.view.upgrade()
                && !views.iter().any(|v| v.entity_id() == view.entity_id())
            {
                views.push(view);
            }
        }
        for view in views {
            keys.extend(
                view.read(cx)
                    .documents
                    .iter()
                    .filter_map(|doc| doc.snapshot_on_disk.clone()),
            );
        }
    }
    let mut closed = closed.lock().unwrap();
    *closed = true;
    for key in keys {
        if let Err(e) = recovery::apply(&dir, &Op::Remove(key)) {
            eprintln!("event=recovery_remove_failed error={e}");
        }
    }
}

impl Workbench {
    /// The snapshot key of a buffer: its file, or a key made once for an untitled buffer.
    fn snapshot_key(doc: &mut Document) -> String {
        if doc.untitled {
            doc.untitled_key
                .get_or_insert_with(|| recovery::untitled_key(doc.id.inode))
                .clone()
        } else {
            recovery::file_key(&doc.path)
        }
    }

    /// An edit (or a deletion on disk) left the buffer unsaved: snapshot it once edits pause.
    pub(super) fn schedule_snapshot(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cx.has_global::<RecoveryQueue>() {
            return;
        }
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        let ticket = doc.snapshot.poke();
        doc.snapshot_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(recovery::SNAPSHOT_DELAY)
                .await;
            let _ = this.update(cx, |this, cx| this.write_snapshot(id, ticket, cx));
        }));
    }

    fn write_snapshot(&mut self, id: DocumentId, ticket: u64, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        if !doc.dirty || !doc.snapshot.fire(ticket) {
            return;
        }
        let key = Self::snapshot_key(doc);
        let record = Record {
            key: key.clone(),
            path: doc.path.clone(),
            untitled: doc.untitled,
            root,
            text: doc.editor.read(cx).text().to_string(),
            written_at: workspace_editor_agent_history::now_ms(),
        };
        // Saved under another name (untitled → file): the old snapshot goes.
        if let Some(old) = doc.snapshot_on_disk.replace(key.clone())
            && old != key
        {
            send(cx, Op::Remove(old));
        }
        send(cx, Op::Write(record));
        self.show_recovery_error(cx);
    }

    /// The buffer was saved, reloaded or discarded: its snapshot (and any pending one) goes.
    pub(super) fn forget_snapshot(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        doc.snapshot.poke();
        doc.snapshot_task = None;
        doc.recovered = false;
        if let Some(key) = doc.snapshot_on_disk.take() {
            send(cx, Op::Remove(key));
        }
        self.show_recovery_error(cx);
    }

    /// A failed snapshot write is shown in the status bar, not dropped (开发说明 R11).
    fn show_recovery_error(&mut self, cx: &mut Context<Self>) {
        let error = cx
            .try_global::<RecoveryQueue>()
            .and_then(|queue| queue.error.lock().unwrap().take());
        if let Some(error) = error {
            self.message = format!("恢复记录没能写入：{error}");
            cx.notify();
        }
    }

    /// Takes the launch's snapshots of files in this window's folder; with `leftovers`, also
    /// the ones no window claimed (files elsewhere, untitled buffers of a closed window).
    pub(super) fn claim_recovery(
        &mut self,
        leftovers: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.root.clone();
        let mine: Vec<Record> = {
            if !cx.has_global::<PendingRecovery>() {
                return;
            }
            let pending = cx.global_mut::<PendingRecovery>();
            let (mine, rest) = std::mem::take(&mut pending.0).into_iter().partition(|r| {
                leftovers
                    || root.as_ref().is_some_and(|root| {
                        r.root.as_ref() == Some(root) || (!r.untitled && r.path.starts_with(root))
                    })
            });
            pending.0 = rest;
            mine
        };
        if mine.is_empty() {
            return;
        }
        self.message = format!("已恢复 {} 个文件上次未保存的修改", mine.len());
        for record in mine {
            self.restore_record(record, window, cx);
        }
        cx.notify();
    }

    fn restore_record(&mut self, record: Record, window: &mut Window, cx: &mut Context<Self>) {
        if record.untitled || !record.path.is_file() {
            // An untitled buffer, or a file that is gone: the text comes back untitled.
            let id = self.new_untitled(window, cx);
            self.apply_recovered(id, record, window, cx);
            return;
        }
        let path = record.path.clone();
        self.open_in_background(
            path,
            window,
            cx,
            move |this, opened, window, cx| match opened {
                Ok(id) => this.apply_recovered(id, record, window, cx),
                Err(error) => {
                    this.message = format!(
                        "没能恢复 {}：{error}；恢复记录保留在磁盘上",
                        record.path.display()
                    );
                    cx.notify();
                }
            },
        );
    }

    /// Puts a snapshot's text into an opened buffer: edited, with the recovery banner, and
    /// keeping the snapshot (under the same key) until the buffer is saved or discarded.
    fn apply_recovered(
        &mut self,
        id: DocumentId,
        record: Record,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        let editor = doc.editor.clone();
        doc.dirty = true;
        doc.version += 1;
        doc.recovered = true;
        if doc.untitled {
            doc.untitled_key = Some(record.key.clone());
        }
        doc.snapshot_on_disk = Some(record.key);
        editor.update(cx, |state, cx| state.set_value(record.text, window, cx));
        self.markdown_refresh(id, cx);
        cx.notify();
    }

    /// 保留: keep the recovered text (the banner goes; the buffer stays edited).
    pub(super) fn keep_recovered(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        if let Some(doc) = self.document_mut(id) {
            doc.recovered = false;
        }
        cx.notify();
    }
}

#[cfg(test)]
#[path = "recovery_ui_tests.rs"]
mod ui_tests;
