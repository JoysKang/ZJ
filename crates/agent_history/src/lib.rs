//! Local agent chat history: SQLite in WAL mode, a background writer that batches inserts, and
//! FTS5 search that handles Chinese (trigram + CJK bigrams + `LIKE` fallback). No GPUI.
//!
//! Everything is opened lazily: constructing a [`History`] does no IO; the database file, the
//! writer thread and the read connection appear on first use. Calls block; the UI runs them on
//! its background executor.

mod schema;
mod search;
mod text;
mod writer;

pub use search::{HitKind, Kinds, SearchHit, SearchQuery};
pub use text::{Snippet, placeholder_title};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    fmt, fs, io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use writer::Op;

/// Pages kept per connection (negative = KiB). Two connections → about 2 MiB at most.
const CACHE_KIB: i64 = 1024;
const QUEUE_DEPTH: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self(format!("会话历史数据库出错：{e}"))
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self(format!("会话历史文件出错：{e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub struct SessionId(pub i64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    User,
    Agent,
    Thought,
    Tool,
    System,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Agent => "agent",
            Role::Thought => "thought",
            Role::Tool => "tool",
            Role::System => "system",
        }
    }
    fn parse(s: &str) -> Role {
        match s {
            "user" => Role::User,
            "thought" => Role::Thought,
            "tool" => Role::Tool,
            "system" => Role::System,
            _ => Role::Agent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStatus {
    Idle,
    Running,
    /// Waiting for the user to answer a permission request.
    Awaiting,
    Completed,
    Failed,
}

impl SessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Idle => "idle",
            SessionStatus::Running => "running",
            SessionStatus::Awaiting => "awaiting",
            SessionStatus::Completed => "completed",
            SessionStatus::Failed => "failed",
        }
    }
    fn parse(s: &str) -> SessionStatus {
        match s {
            "running" => SessionStatus::Running,
            "awaiting" => SessionStatus::Awaiting,
            "completed" => SessionStatus::Completed,
            "failed" => SessionStatus::Failed,
            _ => SessionStatus::Idle,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TitleSource {
    Placeholder,
    Auto,
    User,
}

#[derive(Clone, Debug, Default)]
pub struct NewSession {
    pub workspace_root: PathBuf,
    /// Display name for the workspace; defaults to the root's last component.
    pub workspace_name: Option<String>,
    pub agent_id: String,
    pub acp_session_id: Option<String>,
    /// `None` stores [`placeholder_title`] of `first_prompt` (or 「新会话」).
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// Unix milliseconds; `None` means now. Tests and imports set it explicitly.
    pub created_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SessionSummary {
    pub id: SessionId,
    pub workspace_root: PathBuf,
    pub agent_id: String,
    pub acp_session_id: Option<String>,
    pub title: String,
    pub title_source: TitleSource,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub pinned_rank: Option<f64>,
    pub archived: bool,
    pub status: SessionStatus,
    /// Lines the agent added / removed (as last reported by the panel).
    pub lines_added: i64,
    pub lines_removed: i64,
}

impl SessionSummary {
    pub fn pinned(&self) -> bool {
        self.pinned_rank.is_some()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub seq: i64,
    pub role: Role,
    pub text: String,
    pub created_at: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// Only sessions of this workspace root (the default view).
    Workspace(PathBuf),
    #[default]
    All,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Archived {
    #[default]
    Exclude,
    Only,
    Include,
}

#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub agent_id: Option<String>,
    pub repo: Option<String>,
    /// Inclusive bounds on `updated_at`, Unix milliseconds.
    pub updated_after: Option<i64>,
    pub updated_before: Option<i64>,
    pub status: Option<SessionStatus>,
    pub pinned_only: bool,
    pub archived: Archived,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `~/Library/Application Support/ZJ/agent.db` on macOS, `$XDG_DATA_HOME/zj/agent.db` (or
/// `~/.local/share/zj/agent.db`) elsewhere. `ZJ_AGENT_DB` overrides the path.
pub fn default_db_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZJ_AGENT_DB").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home?.join("Library/Application Support/ZJ/agent.db"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")))?;
    Some(data.join("zj/agent.db"))
}

struct Inner {
    queue: Option<mpsc::SyncSender<Op>>,
    reader: Mutex<Connection>,
    writer: Option<thread::JoinHandle<()>>,
    /// Write failures since the last [`History::flush`]; never silently dropped.
    errors: Arc<Mutex<Vec<String>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the queue lets the writer commit what it has and exit.
        self.queue.take();
        if let Some(handle) = self.writer.take() {
            let _ = handle.join();
        }
    }
}

pub struct History {
    path: PathBuf,
    inner: Mutex<Option<Arc<Inner>>>,
}

impl History {
    /// No IO until the first call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            inner: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the database has been opened yet (for tests and resource accounting).
    pub fn is_open(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    fn inner(&self) -> Result<Arc<Inner>> {
        let mut slot = self.inner.lock().unwrap();
        if let Some(inner) = slot.as_ref() {
            return Ok(inner.clone());
        }
        let inner = Arc::new(open_inner(&self.path)?);
        *slot = Some(inner.clone());
        Ok(inner)
    }

    fn send(&self, op: Op) -> Result<()> {
        let inner = self.inner()?;
        inner
            .queue
            .as_ref()
            .ok_or_else(|| Error::new("会话历史写入线程已停止"))?
            .send(op)
            .map_err(|_| Error::new("会话历史写入线程已停止"))
    }

    fn call<T>(&self, make: impl FnOnce(mpsc::Sender<Result<T>>) -> Op) -> Result<T> {
        let (tx, rx) = mpsc::channel();
        self.send(make(tx))?;
        rx.recv()
            .map_err(|_| Error::new("会话历史写入线程已停止"))?
    }

    /// Creates the session (and its workspace row) and returns its id once committed.
    pub fn create_session(&self, new: NewSession) -> Result<SessionId> {
        self.call(|reply| Op::CreateSession(Box::new(new), reply))
    }

    /// Appends a message at the next sequence number. Batched; see [`History::flush`].
    pub fn append_message(&self, session: SessionId, role: Role, text: impl Into<String>) {
        self.queue_op(Op::PutMessage {
            session,
            seq: None,
            role,
            text: text.into(),
            at: None,
        });
    }

    /// Inserts or replaces the message at `seq` (streamed replies are rewritten in place).
    pub fn put_message(
        &self,
        session: SessionId,
        seq: i64,
        role: Role,
        text: impl Into<String>,
        at: Option<i64>,
    ) {
        self.queue_op(Op::PutMessage {
            session,
            seq: Some(seq),
            role,
            text: text.into(),
            at,
        });
    }

    /// Replaces a placeholder or automatic title; a title the user set is kept.
    pub fn set_auto_title(&self, session: SessionId, title: impl Into<String>) {
        self.queue_op(Op::SetTitle {
            session,
            title: title.into(),
            source: TitleSource::Auto,
        });
    }

    /// User rename; later automatic titles no longer apply.
    pub fn rename(&self, session: SessionId, title: impl Into<String>) {
        self.queue_op(Op::SetTitle {
            session,
            title: title.into(),
            source: TitleSource::User,
        });
    }

    pub fn set_status(&self, session: SessionId, status: SessionStatus) {
        self.queue_op(Op::SetStatus { session, status });
    }

    pub fn set_acp_session_id(&self, session: SessionId, acp_session_id: Option<String>) {
        self.queue_op(Op::SetAcpSession {
            session,
            acp_session_id,
        });
    }

    pub fn set_repo_branch(
        &self,
        session: SessionId,
        repo: Option<String>,
        branch: Option<String>,
    ) {
        self.queue_op(Op::SetRepoBranch {
            session,
            repo,
            branch,
        });
    }

    /// Records a path the agent read or changed (searchable as a file hit).
    pub fn touch_file(&self, session: SessionId, path: impl Into<String>) {
        self.queue_op(Op::TouchFile {
            session,
            path: path.into(),
        });
    }

    /// Lines added / removed by the session's changes (shown in the list).
    pub fn set_line_counts(&self, session: SessionId, added: i64, removed: i64) {
        self.queue_op(Op::SetLineCounts {
            session,
            added,
            removed,
        });
    }

    /// Pins at the top of the pinned group.
    pub fn pin(&self, session: SessionId) {
        self.queue_op(Op::Pin(session));
    }

    pub fn unpin(&self, session: SessionId) {
        self.queue_op(Op::Unpin(session));
    }

    /// Moves a pinned session just before `before` (another pinned session), or to the end of
    /// the pinned group when `before` is `None`. Pins `session` if it was not pinned.
    pub fn move_pin(&self, session: SessionId, before: Option<SessionId>) {
        self.queue_op(Op::MovePin { session, before });
    }

    pub fn set_archived(&self, session: SessionId, archived: bool) {
        self.queue_op(Op::Archive { session, archived });
    }

    /// Hard delete: the session, its messages, files, pin and every FTS row, in one
    /// transaction; freed pages are then returned to the file system.
    pub fn delete_sessions(&self, sessions: &[SessionId]) -> Result<usize> {
        let ids = sessions.to_vec();
        self.call(|reply| Op::Delete(writer::DeleteTarget::Sessions(ids), reply))
    }

    pub fn delete_session(&self, session: SessionId) -> Result<usize> {
        self.delete_sessions(&[session])
    }

    /// Hard-deletes every archived session in `scope`.
    pub fn delete_archived(&self, scope: &Scope) -> Result<usize> {
        let scope = scope.clone();
        self.call(|reply| Op::Delete(writer::DeleteTarget::Archived(scope), reply))
    }

    /// Hard-deletes all sessions of a workspace and the workspace row.
    pub fn delete_workspace(&self, root: &Path) -> Result<usize> {
        let root = root.to_path_buf();
        self.call(|reply| Op::Delete(writer::DeleteTarget::Workspace(root), reply))
    }

    /// Waits until every queued write is committed. Returns the first write error since the
    /// previous flush, if any.
    pub fn flush(&self) -> Result<()> {
        self.call(Op::Flush)?;
        let inner = self.inner()?;
        let mut errors = inner.errors.lock().unwrap();
        if errors.is_empty() {
            return Ok(());
        }
        let first = errors.remove(0);
        errors.clear();
        Err(Error::new(first))
    }

    fn queue_op(&self, op: Op) {
        if let Err(e) = self.send(op) {
            // Surfaced on the next flush (and through the error this logs).
            eprintln!("event=agent_history_queue_failed");
            if let Ok(inner) = self.inner() {
                inner.errors.lock().unwrap().push(e.0);
            }
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let inner = self.inner()?;
        let conn = inner.reader.lock().unwrap();
        f(&conn)
    }

    pub fn session(&self, id: SessionId) -> Result<Option<SessionSummary>> {
        self.read(|conn| {
            Ok(conn
                .query_row(
                    &format!("{SUMMARY_SELECT} WHERE s.id = ?1"),
                    [id.0],
                    summary_from_row,
                )
                .optional()?)
        })
    }

    pub fn messages(&self, id: SessionId) -> Result<Vec<Message>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, role, text, created_at FROM messages WHERE session_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt.query_map([id.0], |r| {
                Ok(Message {
                    seq: r.get(0)?,
                    role: Role::parse(&r.get::<_, String>(1)?),
                    text: r.get(2)?,
                    created_at: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The `limit` messages before `before_seq` (or the newest ones), oldest first, so a long
    /// thread loads its tail and pages backwards.
    pub fn messages_page(
        &self,
        id: SessionId,
        before_seq: Option<i64>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, role, text, created_at FROM messages WHERE session_id = ?1 \
                 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(
                params![id.0, before_seq.unwrap_or(i64::MAX), limit as i64],
                |r| {
                    Ok(Message {
                        seq: r.get(0)?,
                        role: Role::parse(&r.get::<_, String>(1)?),
                        text: r.get(2)?,
                        created_at: r.get(3)?,
                    })
                },
            )?;
            let mut page: Vec<Message> = rows.collect::<rusqlite::Result<_>>()?;
            page.reverse();
            Ok(page)
        })
    }

    pub fn message_count(&self, id: SessionId) -> Result<usize> {
        self.read(|conn| {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM messages WHERE session_id = ?1",
                [id.0],
                |r| r.get(0),
            )?;
            Ok(n as usize)
        })
    }

    pub fn files(&self, id: SessionId) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut stmt =
                conn.prepare("SELECT path FROM session_files WHERE session_id = ?1 ORDER BY path")?;
            let rows = stmt.query_map([id.0], |r| r.get(0))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Pinned sessions first (by pin order), then most recently updated.
    pub fn list(
        &self,
        scope: &Scope,
        filter: &Filter,
        limit: usize,
    ) -> Result<Vec<SessionSummary>> {
        self.read(|conn| list_sessions(conn, scope, filter, Some(limit)))
    }

    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        self.read(|conn| search::run(conn, query))
    }

    /// Row counts of the FTS indexes (for orphan checks and diagnostics).
    pub fn index_counts(&self) -> Result<(i64, i64)> {
        self.read(|conn| {
            let tri = conn.query_row("SELECT count(*) FROM fts_tri_docsize", [], |r| r.get(0))?;
            let cjk = conn.query_row("SELECT count(*) FROM fts_cjk_docsize", [], |r| r.get(0))?;
            Ok((tri, cjk))
        })
    }
}

pub(crate) const SUMMARY_SELECT: &str = "SELECT s.id, w.root, s.agent_id, s.acp_session_id, \
     s.title, s.title_source, s.repo, s.branch, s.created_at, s.updated_at, s.pinned_rank, \
     s.archived, s.status, s.lines_added, s.lines_removed FROM sessions s JOIN workspaces w ON w.id = s.workspace_id";

pub(crate) fn summary_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionSummary> {
    Ok(SessionSummary {
        id: SessionId(r.get(0)?),
        workspace_root: PathBuf::from(r.get::<_, String>(1)?),
        agent_id: r.get(2)?,
        acp_session_id: r.get(3)?,
        title: r.get(4)?,
        title_source: match r.get::<_, i64>(5)? {
            0 => TitleSource::Placeholder,
            1 => TitleSource::Auto,
            _ => TitleSource::User,
        },
        repo: r.get(6)?,
        branch: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
        pinned_rank: r.get(10)?,
        archived: r.get::<_, i64>(11)? != 0,
        status: SessionStatus::parse(&r.get::<_, String>(12)?),
        lines_added: r.get(13)?,
        lines_removed: r.get(14)?,
    })
}

/// Builds the WHERE clause shared by listing and search.
pub(crate) fn filter_sql(scope: &Scope, filter: &Filter) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut push = |clause: &'static str, value: Option<Value>| {
        clauses.push(clause);
        if let Some(v) = value {
            values.push(v);
        }
    };
    if let Scope::Workspace(root) = scope {
        push("w.root = ?", Some(Value::Text(root_key(root))));
    }
    if let Some(agent) = &filter.agent_id {
        push("s.agent_id = ?", Some(Value::Text(agent.clone())));
    }
    if let Some(repo) = &filter.repo {
        push("s.repo = ?", Some(Value::Text(repo.clone())));
    }
    if let Some(after) = filter.updated_after {
        push("s.updated_at >= ?", Some(Value::Integer(after)));
    }
    if let Some(before) = filter.updated_before {
        push("s.updated_at <= ?", Some(Value::Integer(before)));
    }
    if let Some(status) = filter.status {
        push("s.status = ?", Some(Value::Text(status.as_str().into())));
    }
    if filter.pinned_only {
        push("s.pinned_rank IS NOT NULL", None);
    }
    match filter.archived {
        Archived::Exclude => push("s.archived = 0", None),
        Archived::Only => push("s.archived = 1", None),
        Archived::Include => {}
    }
    let sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    (sql, values)
}

pub(crate) fn list_sessions(
    conn: &Connection,
    scope: &Scope,
    filter: &Filter,
    limit: Option<usize>,
) -> Result<Vec<SessionSummary>> {
    let (where_sql, values) = filter_sql(scope, filter);
    let limit_sql = limit.map(|l| format!(" LIMIT {l}")).unwrap_or_default();
    let sql = format!(
        "{SUMMARY_SELECT}{where_sql} ORDER BY s.pinned_rank IS NULL, s.pinned_rank, \
         s.updated_at DESC, s.id DESC{limit_sql}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), summary_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Workspace roots are stored as given (already canonical in the app), without a trailing `/`.
pub(crate) fn root_key(root: &Path) -> String {
    let s = root.to_string_lossy();
    let trimmed = s.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.into()
    }
}

fn open_inner(path: &Path) -> Result<Inner> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.exists()
    {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    // Create the file ourselves so it is 0600 from the start; SQLite gives -wal / -shm the
    // same mode as the main file.
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    drop(file);

    let mut writer_conn = open_connection(path)?;
    schema::migrate(&mut writer_conn)?;
    let reader = open_connection(path)?;
    let errors = Arc::new(Mutex::new(Vec::new()));
    let (queue, rx) = mpsc::sync_channel(QUEUE_DEPTH);
    let writer_errors = errors.clone();
    let writer = thread::Builder::new()
        .name("agent-history-writer".into())
        .spawn(move || writer::run(writer_conn, rx, writer_errors))?;
    eprintln!("event=agent_history_open");
    Ok(Inner {
        queue: Some(queue),
        reader: Mutex::new(reader),
        writer: Some(writer),
        errors,
    })
}

fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    // Only takes effect before the first table exists, i.e. on a fresh file.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::new(format!(
            "会话历史数据库无法切换到 WAL（{mode}）"
        )));
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -CACHE_KIB)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "mmap_size", 0)?;
    Ok(conn)
}

/// Finds or creates the workspace row.
pub(crate) fn workspace_id(conn: &Connection, root: &Path, name: Option<&str>) -> Result<i64> {
    let key = root_key(root);
    if let Some(id) = conn
        .query_row("SELECT id FROM workspaces WHERE root = ?1", [&key], |r| {
            r.get(0)
        })
        .optional()?
    {
        return Ok(id);
    }
    let name = name.map(str::to_string).unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| key.clone())
    });
    conn.execute(
        "INSERT INTO workspaces(root, name, created_at) VALUES (?1, ?2, ?3)",
        params![key, name, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}
