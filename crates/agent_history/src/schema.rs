//! Versioned schema. Each entry runs once, in order, inside one transaction; `user_version`
//! records how many have been applied.

use rusqlite::Connection;

/// Document kinds of session-level FTS rows. Message rows use the message id (positive);
/// session rows use `-(session_id * 4 + kind)` so both live in the same contentless tables.
pub(crate) const DOC_TITLE: i64 = 1;
pub(crate) const DOC_FILES: i64 = 2;
pub(crate) const DOC_META: i64 = 3;

pub(crate) fn session_doc_rowid(session: i64, kind: i64) -> i64 {
    -(session * 4 + kind)
}

/// Inverse of [`session_doc_rowid`] for negative rowids.
pub(crate) fn decode_session_doc(rowid: i64) -> (i64, i64) {
    let v = -rowid;
    (v / 4, v % 4)
}

const MIGRATIONS: &[&str] = &[
    // 1: initial schema.
    r#"
    CREATE TABLE workspaces (
        id         INTEGER PRIMARY KEY,
        root       TEXT NOT NULL UNIQUE,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE sessions (
        id             INTEGER PRIMARY KEY,
        workspace_id   INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
        agent_id       TEXT NOT NULL,
        acp_session_id TEXT,
        title          TEXT NOT NULL,
        -- 0 placeholder, 1 automatic, 2 set by the user (never overwritten)
        title_source   INTEGER NOT NULL DEFAULT 0,
        repo           TEXT,
        branch         TEXT,
        created_at     INTEGER NOT NULL,
        updated_at     INTEGER NOT NULL,
        pinned_rank    REAL,
        archived       INTEGER NOT NULL DEFAULT 0,
        status         TEXT NOT NULL DEFAULT 'idle'
            CHECK (status IN ('idle','running','awaiting','completed','failed'))
    );
    CREATE INDEX sessions_by_workspace ON sessions(workspace_id, archived, updated_at DESC);
    CREATE INDEX sessions_pinned ON sessions(pinned_rank) WHERE pinned_rank IS NOT NULL;
    CREATE TABLE messages (
        id         INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        seq        INTEGER NOT NULL,
        role       TEXT NOT NULL CHECK (role IN ('user','agent','thought','tool','system')),
        text       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        UNIQUE (session_id, seq)
    );
    CREATE TABLE session_files (
        session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        path       TEXT NOT NULL,
        PRIMARY KEY (session_id, path)
    ) WITHOUT ROWID;
    -- Contentless: the text lives once, in messages / sessions; the index only stores tokens.
    CREATE VIRTUAL TABLE fts_tri USING fts5(
        body, tokenize = 'trigram case_sensitive 0', content = '', contentless_delete = 1
    );
    CREATE VIRTUAL TABLE fts_cjk USING fts5(
        body, tokenize = 'unicode61', content = '', contentless_delete = 1
    );
    "#,
];

pub(crate) fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let target = MIGRATIONS.len() as i64;
    if version > target {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "数据库版本 {version} 比程序支持的 {target} 新"
        )));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_doc_rowids_round_trip() {
        for (session, kind) in [(1, DOC_TITLE), (77, DOC_FILES), (123_456, DOC_META)] {
            let rowid = session_doc_rowid(session, kind);
            assert!(rowid < 0);
            assert_eq!(decode_session_doc(rowid), (session, kind));
        }
    }
}
