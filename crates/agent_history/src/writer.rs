//! The single writer: drains the queue in batches, one transaction per batch, keeps the FTS
//! rows in step with the base tables and reclaims space after deletes.

use crate::{
    Error, NewSession, Result, Role, Scope, SessionId, SessionStatus, TitleSource, now_ms,
    placeholder_title, root_key,
    schema::{DOC_FILES, DOC_META, DOC_TITLE, session_doc_rowid},
    text::cjk_bigrams,
    workspace_id,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
};

const BATCH: usize = 512;

pub(crate) enum DeleteTarget {
    Sessions(Vec<SessionId>),
    Archived(Scope),
    Workspace(PathBuf),
}

pub(crate) enum Op {
    CreateSession(Box<NewSession>, mpsc::Sender<Result<SessionId>>),
    PutMessage {
        session: SessionId,
        seq: Option<i64>,
        role: Role,
        text: String,
        at: Option<i64>,
    },
    SetTitle {
        session: SessionId,
        title: String,
        source: TitleSource,
    },
    SetStatus {
        session: SessionId,
        status: SessionStatus,
    },
    SetAcpSession {
        session: SessionId,
        acp_session_id: Option<String>,
    },
    SetRepoBranch {
        session: SessionId,
        repo: Option<String>,
        branch: Option<String>,
    },
    TouchFile {
        session: SessionId,
        path: String,
    },
    Pin(SessionId),
    Unpin(SessionId),
    MovePin {
        session: SessionId,
        before: Option<SessionId>,
    },
    Archive {
        session: SessionId,
        archived: bool,
    },
    Delete(DeleteTarget, mpsc::Sender<Result<usize>>),
    Flush(mpsc::Sender<Result<()>>),
}

enum Reply {
    Created(mpsc::Sender<Result<SessionId>>, Result<SessionId>),
    Deleted(mpsc::Sender<Result<usize>>, Result<usize>),
    Flushed(mpsc::Sender<Result<()>>),
}

pub(crate) fn run(mut conn: Connection, rx: mpsc::Receiver<Op>, errors: Arc<Mutex<Vec<String>>>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while batch.len() < BATCH {
            match rx.try_recv() {
                Ok(op) => batch.push(op),
                Err(_) => break,
            }
        }
        let mut replies = Vec::new();
        let mut deleted = false;
        let outcome = (|| -> Result<()> {
            let tx = conn.transaction()?;
            for op in batch {
                apply(&tx, op, &mut replies, &mut deleted, &errors);
            }
            tx.commit()?;
            Ok(())
        })();
        if let Err(e) = &outcome {
            eprintln!("event=agent_history_commit_failed");
            errors.lock().unwrap().push(e.to_string());
        }
        // Replies only after commit, so a returned id or flush is visible to readers.
        for reply in replies {
            match reply {
                Reply::Created(tx, result) => {
                    let _ = tx.send(outcome.clone().and(result));
                }
                Reply::Deleted(tx, result) => {
                    let _ = tx.send(outcome.clone().and(result));
                }
                Reply::Flushed(tx) => {
                    let _ = tx.send(Ok(()));
                }
            }
        }
        if deleted && outcome.is_ok() {
            reclaim(&conn, &errors);
        }
    }
}

fn record(errors: &Mutex<Vec<String>>, result: Result<()>) {
    if let Err(e) = result {
        eprintln!("event=agent_history_write_failed");
        errors.lock().unwrap().push(e.to_string());
    }
}

fn apply(
    tx: &Transaction<'_>,
    op: Op,
    replies: &mut Vec<Reply>,
    deleted: &mut bool,
    errors: &Mutex<Vec<String>>,
) {
    match op {
        Op::CreateSession(new, reply) => {
            let result = create_session(tx, &new);
            replies.push(Reply::Created(reply, result));
        }
        Op::PutMessage {
            session,
            seq,
            role,
            text,
            at,
        } => record(errors, put_message(tx, session, seq, role, &text, at)),
        Op::SetTitle {
            session,
            title,
            source,
        } => record(errors, set_title(tx, session, &title, source)),
        Op::SetStatus { session, status } => record(
            errors,
            exec(
                tx,
                "UPDATE sessions SET status = ?2 WHERE id = ?1",
                params![session.0, status.as_str()],
            ),
        ),
        Op::SetAcpSession {
            session,
            acp_session_id,
        } => record(
            errors,
            exec(
                tx,
                "UPDATE sessions SET acp_session_id = ?2 WHERE id = ?1",
                params![session.0, acp_session_id],
            ),
        ),
        Op::SetRepoBranch {
            session,
            repo,
            branch,
        } => record(
            errors,
            exec(
                tx,
                "UPDATE sessions SET repo = ?2, branch = ?3 WHERE id = ?1",
                params![session.0, repo, branch],
            )
            .and_then(|_| reindex_meta(tx, session.0)),
        ),
        Op::TouchFile { session, path } => record(errors, touch_file(tx, session, &path)),
        Op::Pin(session) => record(errors, pin_top(tx, session)),
        Op::Unpin(session) => record(
            errors,
            exec(
                tx,
                "UPDATE sessions SET pinned_rank = NULL WHERE id = ?1",
                params![session.0],
            ),
        ),
        Op::MovePin { session, before } => record(errors, move_pin(tx, session, before)),
        Op::Archive { session, archived } => record(
            errors,
            exec(
                tx,
                "UPDATE sessions SET archived = ?2 WHERE id = ?1",
                params![session.0, archived as i64],
            ),
        ),
        Op::Delete(target, reply) => {
            let result = delete(tx, &target);
            if matches!(result, Ok(n) if n > 0) {
                *deleted = true;
            }
            replies.push(Reply::Deleted(reply, result));
        }
        Op::Flush(reply) => replies.push(Reply::Flushed(reply)),
    }
}

fn exec(tx: &Transaction<'_>, sql: &str, params: impl rusqlite::Params) -> Result<()> {
    tx.execute(sql, params)?;
    Ok(())
}

fn index_doc(tx: &Transaction<'_>, rowid: i64, text: &str) -> Result<()> {
    unindex_doc(tx, rowid)?;
    if text.is_empty() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO fts_tri(rowid, body) VALUES (?1, ?2)",
        params![rowid, text],
    )?;
    let bigrams = cjk_bigrams(text);
    if !bigrams.is_empty() {
        tx.execute(
            "INSERT INTO fts_cjk(rowid, body) VALUES (?1, ?2)",
            params![rowid, bigrams],
        )?;
    }
    Ok(())
}

fn unindex_doc(tx: &Transaction<'_>, rowid: i64) -> Result<()> {
    tx.execute("DELETE FROM fts_tri WHERE rowid = ?1", [rowid])?;
    tx.execute("DELETE FROM fts_cjk WHERE rowid = ?1", [rowid])?;
    Ok(())
}

fn create_session(tx: &Transaction<'_>, new: &NewSession) -> Result<SessionId> {
    let workspace = workspace_id(tx, &new.workspace_root, new.workspace_name.as_deref())?;
    let (title, source) = match &new.title {
        Some(title) => (title.clone(), 1),
        None => (
            placeholder_title(new.first_prompt.as_deref().unwrap_or_default()),
            0,
        ),
    };
    let at = new.created_at.unwrap_or_else(now_ms);
    tx.execute(
        "INSERT INTO sessions(workspace_id, agent_id, acp_session_id, title, title_source, repo, \
         branch, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            workspace,
            new.agent_id,
            new.acp_session_id,
            title,
            source,
            new.repo,
            new.branch,
            at
        ],
    )?;
    let id = tx.last_insert_rowid();
    index_doc(tx, session_doc_rowid(id, DOC_TITLE), &title)?;
    reindex_meta(tx, id)?;
    Ok(SessionId(id))
}

fn reindex_meta(tx: &Transaction<'_>, session: i64) -> Result<()> {
    let meta: Option<String> = tx
        .query_row(
            "SELECT agent_id || ' ' || coalesce(repo, '') || ' ' || coalesce(branch, '') \
             FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .optional()?;
    match meta {
        Some(meta) => index_doc(tx, session_doc_rowid(session, DOC_META), meta.trim()),
        None => Err(Error::new(format!("会话 {session} 不存在"))),
    }
}

fn put_message(
    tx: &Transaction<'_>,
    session: SessionId,
    seq: Option<i64>,
    role: Role,
    text: &str,
    at: Option<i64>,
) -> Result<()> {
    let at = at.unwrap_or_else(now_ms);
    let seq = match seq {
        Some(seq) => seq,
        None => tx.query_row(
            "SELECT coalesce(max(seq) + 1, 0) FROM messages WHERE session_id = ?1",
            [session.0],
            |r| r.get(0),
        )?,
    };
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM messages WHERE session_id = ?1 AND seq = ?2",
            params![session.0, seq],
            |r| r.get(0),
        )
        .optional()?;
    let id = match existing {
        Some(id) => {
            tx.execute(
                "UPDATE messages SET role = ?2, text = ?3 WHERE id = ?1",
                params![id, role.as_str(), text],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO messages(session_id, seq, role, text, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session.0, seq, role.as_str(), text, at],
            )?;
            tx.last_insert_rowid()
        }
    };
    index_doc(tx, id, text)?;
    tx.execute(
        "UPDATE sessions SET updated_at = max(updated_at, ?2) WHERE id = ?1",
        params![session.0, at],
    )?;
    Ok(())
}

fn set_title(
    tx: &Transaction<'_>,
    session: SessionId,
    title: &str,
    source: TitleSource,
) -> Result<()> {
    let changed = match source {
        TitleSource::User => tx.execute(
            "UPDATE sessions SET title = ?2, title_source = 2 WHERE id = ?1",
            params![session.0, title],
        )?,
        _ => tx.execute(
            "UPDATE sessions SET title = ?2, title_source = 1 WHERE id = ?1 AND title_source < 2",
            params![session.0, title],
        )?,
    };
    if changed > 0 {
        index_doc(tx, session_doc_rowid(session.0, DOC_TITLE), title)?;
    }
    Ok(())
}

fn touch_file(tx: &Transaction<'_>, session: SessionId, path: &str) -> Result<()> {
    let added = tx.execute(
        "INSERT OR IGNORE INTO session_files(session_id, path) VALUES (?1, ?2)",
        params![session.0, path],
    )?;
    if added == 0 {
        return Ok(());
    }
    let joined: String = tx.query_row(
        "SELECT group_concat(path, char(10)) FROM session_files WHERE session_id = ?1",
        [session.0],
        |r| r.get(0),
    )?;
    index_doc(tx, session_doc_rowid(session.0, DOC_FILES), &joined)
}

fn pin_top(tx: &Transaction<'_>, session: SessionId) -> Result<()> {
    let top: Option<f64> = tx.query_row(
        "SELECT min(pinned_rank) FROM sessions WHERE pinned_rank IS NOT NULL AND id != ?1",
        [session.0],
        |r| r.get(0),
    )?;
    exec(
        tx,
        "UPDATE sessions SET pinned_rank = ?2 WHERE id = ?1",
        params![session.0, top.map(|t| t - 1.0).unwrap_or(0.0)],
    )
}

/// Fractional ranks: a move rewrites one row; ranks are renumbered when a gap gets too small.
fn move_pin(tx: &Transaction<'_>, session: SessionId, before: Option<SessionId>) -> Result<()> {
    let ranks = |tx: &Transaction<'_>| -> Result<Vec<(i64, f64)>> {
        let mut stmt = tx.prepare(
            "SELECT id, pinned_rank FROM sessions WHERE pinned_rank IS NOT NULL AND id != ?1 \
             ORDER BY pinned_rank, id",
        )?;
        let rows = stmt.query_map([session.0], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let mut others = ranks(tx)?;
    let position = match before {
        Some(target) => others
            .iter()
            .position(|(id, _)| *id == target.0)
            .unwrap_or(others.len()),
        None => others.len(),
    };
    let lower = position.checked_sub(1).map(|i| others[i].1);
    let upper = others.get(position).map(|o| o.1);
    let mut rank = match (lower, upper) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a + 1.0,
        (None, Some(b)) => b - 1.0,
        (None, None) => 0.0,
    };
    if let (Some(a), Some(b)) = (lower, upper)
        && (b - a) < 1e-9
    {
        for (index, (id, _)) in others.iter_mut().enumerate() {
            let new_rank = if index < position {
                index as f64
            } else {
                index as f64 + 1.0
            };
            tx.execute(
                "UPDATE sessions SET pinned_rank = ?2 WHERE id = ?1",
                params![*id, new_rank],
            )?;
        }
        rank = position as f64;
    }
    exec(
        tx,
        "UPDATE sessions SET pinned_rank = ?2 WHERE id = ?1",
        params![session.0, rank],
    )
}

fn delete(tx: &Transaction<'_>, target: &DeleteTarget) -> Result<usize> {
    let ids: Vec<i64> = match target {
        DeleteTarget::Sessions(ids) => ids.iter().map(|s| s.0).collect(),
        DeleteTarget::Archived(scope) => {
            let (sql, values): (&str, Vec<String>) = match scope {
                Scope::All => ("SELECT id FROM sessions WHERE archived = 1", vec![]),
                Scope::Workspace(root) => (
                    "SELECT s.id FROM sessions s JOIN workspaces w ON w.id = s.workspace_id \
                     WHERE s.archived = 1 AND w.root = ?1",
                    vec![root_key(root)],
                ),
            };
            let mut stmt = tx.prepare(sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        }
        DeleteTarget::Workspace(root) => {
            let mut stmt = tx.prepare(
                "SELECT s.id FROM sessions s JOIN workspaces w ON w.id = s.workspace_id \
                 WHERE w.root = ?1",
            )?;
            let rows = stmt.query_map([root_key(root)], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        }
    };
    let mut removed = 0;
    {
        let mut message_ids = tx.prepare("SELECT id FROM messages WHERE session_id = ?1")?;
        let mut del_tri = tx.prepare("DELETE FROM fts_tri WHERE rowid = ?1")?;
        let mut del_cjk = tx.prepare("DELETE FROM fts_cjk WHERE rowid = ?1")?;
        for &id in &ids {
            let rows: Vec<i64> = message_ids
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for rowid in rows
                .into_iter()
                .chain([DOC_TITLE, DOC_FILES, DOC_META].map(|k| session_doc_rowid(id, k)))
            {
                del_tri.execute([rowid])?;
                del_cjk.execute([rowid])?;
            }
            // messages and session_files go with the session (ON DELETE CASCADE).
            removed += tx.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        }
    }
    if let DeleteTarget::Workspace(root) = target {
        tx.execute("DELETE FROM workspaces WHERE root = ?1", [root_key(root)])?;
    }
    Ok(removed)
}

/// After deletes: merge FTS segments so tombstoned entries are dropped, hand free pages back
/// to the file system and truncate the WAL.
fn reclaim(conn: &Connection, errors: &Mutex<Vec<String>>) {
    let result = (|| -> Result<()> {
        conn.execute_batch(
            "INSERT INTO fts_tri(fts_tri) VALUES ('optimize');
             INSERT INTO fts_cjk(fts_cjk) VALUES ('optimize');",
        )?;
        // Each step frees pages; execute_batch would stop after the first step.
        let mut stmt = conn.prepare("PRAGMA incremental_vacuum")?;
        let mut rows = stmt.query([])?;
        while rows.next()?.is_some() {}
        drop(rows);
        drop(stmt);
        // Busy (a reader mid-query) only leaves the WAL longer; not an error.
        let _: (i64, i64, i64) = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        Ok(())
    })();
    record(errors, result);
}
