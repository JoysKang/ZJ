use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};
use workspace_editor_agent_history::{
    Archived, Filter, History, HitKind, Kinds, NewSession, Role, Scope, SearchQuery, SessionId,
    SessionStatus, TitleSource, now_ms,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "zj-history-{tag}-{}-{n}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn db(&self) -> PathBuf {
        self.0.join("data/agent.db")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session(history: &History, root: &str, agent: &str, prompt: &str) -> SessionId {
    history
        .create_session(NewSession {
            workspace_root: PathBuf::from(root),
            agent_id: agent.into(),
            first_prompt: Some(prompt.into()),
            repo: Some("zj".into()),
            branch: Some("main".into()),
            ..Default::default()
        })
        .unwrap()
}

fn search(
    history: &History,
    text: &str,
    scope: Scope,
) -> Vec<workspace_editor_agent_history::SearchHit> {
    history.search(&SearchQuery::new(text, scope)).unwrap()
}

fn marked(hit: &workspace_editor_agent_history::SearchHit) -> Vec<String> {
    hit.snippet
        .highlights
        .iter()
        .map(|r| hit.snippet.text[r.clone()].to_string())
        .collect()
}

#[test]
fn opens_lazily_with_private_wal_files() {
    let dir = TempDir::new("open");
    let history = History::new(dir.db());
    assert!(!history.is_open());
    assert!(!dir.db().exists());
    let id = session(&history, "/w/a", "claude-code", "你好");
    history.append_message(id, Role::User, "你好");
    history.flush().unwrap();
    assert!(history.is_open());
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", dir.db().display()));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
    }
    let dir_mode = std::fs::metadata(dir.db().parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    drop(history);
    // Reopen: migrations are idempotent and data survives.
    let history = History::new(dir.db());
    assert_eq!(history.messages(id).unwrap().len(), 1);
}

#[test]
fn chinese_and_english_queries() {
    let dir = TempDir::new("cjk");
    let h = History::new(dir.db());
    let a = session(&h, "/w/a", "claude-code", "修复 WebSocket 断线重连");
    h.append_message(
        a,
        Role::User,
        "WebSocket 断线以后要自动重连，并且恢复快照。",
    );
    h.append_message(
        a,
        Role::Agent,
        "我会在 reconnect() 里加入指数退避，然后重放快照 snapshot。",
    );
    let b = session(&h, "/w/a", "codex", "整理设置文件的读写");
    h.append_message(b, Role::User, "settings.json 写入失败时要提示用户");
    h.touch_file(b, "crates/app/src/settings.rs");
    let c = session(&h, "/w/a", "gemini", "重命名变量");
    h.append_message(c, Role::Agent, "已经把 conn 重命名为 connection。");
    h.flush().unwrap();

    // Two-character Chinese (bigram table).
    let hits = search(&h, "重连", Scope::All);
    assert_eq!(
        hits.iter().map(|h| h.session.id).collect::<Vec<_>>(),
        vec![a]
    );
    assert!(marked(&hits[0]).iter().all(|m| m == "重连"));

    // Three or more characters (trigram), Chinese and English, case-insensitive.
    assert_eq!(search(&h, "自动重连", Scope::All)[0].session.id, a);
    let hits = search(&h, "websocket", Scope::All);
    assert_eq!(hits.len(), 1);
    assert_eq!(marked(&hits[0]), vec!["WebSocket".to_string()]);
    assert_eq!(search(&h, "Reconnect", Scope::All)[0].session.id, a);

    // Every term must match somewhere in the session (different messages are fine).
    let hits = search(&h, "重连 快照", Scope::All);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session.id, a);
    assert!(search(&h, "重连 settings", Scope::All).is_empty());

    // One character: LIKE over titles / files / metadata only.
    let hits = search(&h, "重", Scope::All);
    let mut ids: Vec<_> = hits.iter().map(|h| h.session.id).collect();
    ids.sort();
    assert_eq!(ids, vec![a, c]);
    assert!(hits.iter().all(|h| h.kind == HitKind::Title));

    // File paths.
    let hits = search(&h, "settings.rs", Scope::All);
    assert_eq!(hits[0].session.id, b);
    assert_eq!(hits[0].kind, HitKind::File);
    assert_eq!(marked(&hits[0]), vec!["settings.rs".to_string()]);

    // Agent id (meta), and kinds filter.
    assert_eq!(search(&h, "gemini", Scope::All)[0].session.id, c);
    let mut q = SearchQuery::new("settings", Scope::All);
    q.kinds = Kinds {
        title: true,
        message: false,
        file: false,
        meta: false,
    };
    assert!(h.search(&q).unwrap().is_empty());
    q.kinds.message = true;
    assert!(matches!(
        h.search(&q).unwrap()[0].kind,
        HitKind::Message {
            role: Role::User,
            ..
        }
    ));

    // Special characters are literal, not FTS syntax.
    assert!(search(&h, "\"OR\" NEAR(", Scope::All).is_empty());
    assert!(search(&h, "%", Scope::All).is_empty());
}

#[test]
fn workspace_scope_separates_history() {
    let dir = TempDir::new("scope");
    let h = History::new(dir.db());
    let a = session(&h, "/w/alpha", "claude-code", "alpha 里的重连");
    let b = session(&h, "/w/beta/", "claude-code", "beta 里的重连");
    h.flush().unwrap();
    let alpha = Scope::Workspace("/w/alpha".into());
    // Trailing slash is normalized.
    let beta = Scope::Workspace("/w/beta".into());
    let ids = |scope: &Scope| -> Vec<SessionId> {
        h.list(scope, &Filter::default(), 100)
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect()
    };
    assert_eq!(ids(&alpha), vec![a]);
    assert_eq!(ids(&beta), vec![b]);
    assert_eq!(ids(&Scope::All).len(), 2);
    assert_eq!(search(&h, "重连", alpha.clone()).len(), 1);
    assert_eq!(search(&h, "重连", Scope::All).len(), 2);
    assert_eq!(
        h.session(a).unwrap().unwrap().workspace_root,
        Path::new("/w/alpha")
    );
}

#[test]
fn pins_come_first_and_reorder() {
    let dir = TempDir::new("pins");
    let h = History::new(dir.db());
    let ids: Vec<SessionId> = (0..5)
        .map(|i| {
            h.create_session(NewSession {
                workspace_root: "/w".into(),
                agent_id: "codex".into(),
                title: Some(format!("任务 {i} 重连")),
                created_at: Some(1_000 + i),
                ..Default::default()
            })
            .unwrap()
        })
        .collect();
    let order = || -> Vec<SessionId> {
        h.flush().unwrap();
        h.list(&Scope::All, &Filter::default(), 100)
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect()
    };
    // Unpinned: newest first.
    assert_eq!(order(), vec![ids[4], ids[3], ids[2], ids[1], ids[0]]);
    h.pin(ids[0]);
    h.pin(ids[1]);
    // Newest pin goes on top.
    assert_eq!(order(), vec![ids[1], ids[0], ids[4], ids[3], ids[2]]);
    h.move_pin(ids[0], Some(ids[1]));
    assert_eq!(order()[..2], [ids[0], ids[1]]);
    h.move_pin(ids[2], None); // pins at the end of the group
    assert_eq!(order()[..3], [ids[0], ids[1], ids[2]]);
    // Many moves into the same gap stay ordered (renumbering kicks in).
    for _ in 0..80 {
        h.move_pin(ids[2], Some(ids[1]));
        h.move_pin(ids[1], Some(ids[2]));
    }
    assert_eq!(order()[..3], [ids[0], ids[1], ids[2]]);
    let pinned_only = h
        .list(
            &Scope::All,
            &Filter {
                pinned_only: true,
                ..Default::default()
            },
            100,
        )
        .unwrap();
    assert_eq!(pinned_only.len(), 3);
    // Search: pinned first regardless of relevance.
    let hits = search(&h, "重连", Scope::All);
    assert_eq!(
        hits.iter()
            .map(|h| h.session.id)
            .take(3)
            .collect::<Vec<_>>(),
        vec![ids[0], ids[1], ids[2]]
    );
    h.unpin(ids[1]);
    assert_eq!(order()[..2], [ids[0], ids[2]]);
}

#[test]
fn archive_and_filters() {
    let dir = TempDir::new("archive");
    let h = History::new(dir.db());
    let old = h
        .create_session(NewSession {
            workspace_root: "/w".into(),
            agent_id: "claude-code".into(),
            title: Some("旧的重连讨论".into()),
            repo: Some("zj".into()),
            created_at: Some(1_000),
            ..Default::default()
        })
        .unwrap();
    let new = h
        .create_session(NewSession {
            workspace_root: "/w".into(),
            agent_id: "codex".into(),
            title: Some("新的重连讨论".into()),
            repo: Some("other".into()),
            created_at: Some(5_000),
            ..Default::default()
        })
        .unwrap();
    h.set_archived(old, true);
    h.set_status(new, SessionStatus::Awaiting);
    h.flush().unwrap();
    let list = |filter: Filter| -> Vec<SessionId> {
        h.list(&Scope::All, &filter, 10)
            .unwrap()
            .iter()
            .map(|s| s.id)
            .collect()
    };
    assert_eq!(list(Filter::default()), vec![new]);
    assert_eq!(
        list(Filter {
            archived: Archived::Only,
            ..Default::default()
        }),
        vec![old]
    );
    // count agrees with list for the same scope and filter.
    for filter in [
        Filter::default(),
        Filter {
            archived: Archived::Only,
            ..Default::default()
        },
    ] {
        assert_eq!(
            h.count(&Scope::All, &filter).unwrap(),
            h.list(&Scope::All, &filter, 100).unwrap().len()
        );
    }
    assert_eq!(
        list(Filter {
            archived: Archived::Include,
            ..Default::default()
        }),
        vec![new, old]
    );
    let all = |f: Filter| Filter {
        archived: Archived::Include,
        ..f
    };
    assert_eq!(
        list(all(Filter {
            agent_id: Some("claude-code".into()),
            ..Default::default()
        })),
        vec![old]
    );
    assert_eq!(
        list(all(Filter {
            repo: Some("other".into()),
            ..Default::default()
        })),
        vec![new]
    );
    assert_eq!(
        list(all(Filter {
            updated_before: Some(2_000),
            ..Default::default()
        })),
        vec![old]
    );
    assert_eq!(
        list(all(Filter {
            updated_after: Some(2_000),
            status: Some(SessionStatus::Awaiting),
            ..Default::default()
        })),
        vec![new]
    );
    // Search honours the archive filter too.
    assert_eq!(search(&h, "重连", Scope::All).len(), 1);
    let mut q = SearchQuery::new("重连", Scope::All);
    q.filter.archived = Archived::Include;
    assert_eq!(h.search(&q).unwrap().len(), 2);
}

#[test]
fn auto_title_never_overrides_a_user_title() {
    let dir = TempDir::new("title");
    let h = History::new(dir.db());
    let id = session(
        &h,
        "/w",
        "claude-code",
        "@src/lib.rs 帮我看看这个连接池为什么会泄漏",
    );
    h.flush().unwrap();
    let s = h.session(id).unwrap().unwrap();
    assert_eq!(s.title, "帮我看看这个连接池为什么会泄漏");
    assert_eq!(s.title_source, TitleSource::Placeholder);
    h.set_auto_title(id, "连接池泄漏排查");
    h.flush().unwrap();
    assert_eq!(h.session(id).unwrap().unwrap().title, "连接池泄漏排查");
    assert_eq!(search(&h, "泄漏排查", Scope::All)[0].kind, HitKind::Title);
    h.rename(id, "我的标题");
    h.set_auto_title(id, "不应生效");
    h.flush().unwrap();
    let s = h.session(id).unwrap().unwrap();
    assert_eq!(
        (s.title.as_str(), s.title_source),
        ("我的标题", TitleSource::User)
    );
    assert!(search(&h, "泄漏排查", Scope::All).is_empty());
}

#[test]
fn line_counts_are_stored_per_session() {
    let dir = TempDir::new("lines");
    let h = History::new(dir.db());
    let id = session(&h, "/w", "codex", "改一下");
    h.flush().unwrap();
    let s = h.session(id).unwrap().unwrap();
    assert_eq!((s.lines_added, s.lines_removed), (0, 0));
    h.set_line_counts(id, 76, 4);
    h.flush().unwrap();
    let s = h.session(id).unwrap().unwrap();
    assert_eq!((s.lines_added, s.lines_removed), (76, 4));
}

#[test]
fn messages_can_be_rewritten_in_place() {
    let dir = TempDir::new("put");
    let h = History::new(dir.db());
    let id = session(&h, "/w", "codex", "流式");
    h.put_message(id, 0, Role::Agent, "部分回复", None);
    h.put_message(id, 0, Role::Agent, "完整回复：使用指数退避", None);
    h.append_message(id, Role::User, "继续");
    h.flush().unwrap();
    let messages = h.messages(id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, "完整回复：使用指数退避");
    assert_eq!(messages[1].seq, 1);
    assert!(search(&h, "部分回复", Scope::All).is_empty());
    assert_eq!(search(&h, "指数退避", Scope::All).len(), 1);
}

/// Deterministic generator for synthetic transcripts.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.next() % items.len()]
    }
}

const ZH: &[&str] = &[
    "修复",
    "断线",
    "重新连接",
    "快照",
    "工作区",
    "仓库",
    "分支",
    "提交",
    "差异",
    "编辑器",
    "性能",
    "内存",
    "启动",
    "渲染",
    "缓存",
    "索引",
    "搜索",
    "配置",
    "权限",
    "终端",
    "文件",
    "测试",
    "日志",
    "错误",
    "超时",
    "取消",
    "并发",
    "线程",
    "布局",
    "主题",
    "图标",
    "字体",
];
const EN: &[&str] = &[
    "render",
    "buffer",
    "socket",
    "cache",
    "index",
    "query",
    "thread",
    "commit",
    "branch",
    "diff",
    "layout",
    "theme",
    "settings.json",
    "Cargo.toml",
    "main.rs",
    "timeout",
    "retry",
    "parser",
    "token",
    "stream",
    "channel",
    "panic",
    "unwrap",
    "window",
    "editor",
];

fn sentence(rng: &mut Lcg) -> String {
    let words = 12 + rng.next() % 40;
    let mut s = String::new();
    for _ in 0..words {
        if rng.next().is_multiple_of(3) {
            s.push_str(rng.pick(EN));
            s.push(' ');
        } else {
            s.push_str(rng.pick(ZH));
        }
        if rng.next().is_multiple_of(9) {
            s.push('，');
        }
    }
    s.push('。');
    s
}

static TEXT_BYTES: AtomicUsize = AtomicUsize::new(0);

fn fill(h: &History, sessions: usize, messages: usize, seed: u64) -> Vec<SessionId> {
    let mut rng = Lcg(seed);
    let mut ids = Vec::new();
    for i in 0..sessions {
        let id = h
            .create_session(NewSession {
                workspace_root: PathBuf::from(format!("/w/{}", i % 4)),
                agent_id: ["claude-code", "codex", "gemini"][i % 3].into(),
                title: Some(format!("{}{} {}", rng.pick(ZH), rng.pick(ZH), rng.pick(EN))),
                repo: Some("zj".into()),
                branch: Some(format!("feature/{}", rng.pick(EN))),
                created_at: Some(now_ms() - (i as i64) * 3_600_000),
                ..Default::default()
            })
            .unwrap();
        ids.push(id);
    }
    for m in 0..messages {
        let id = ids[m % sessions];
        let role = if m.is_multiple_of(2) {
            Role::User
        } else {
            Role::Agent
        };
        let mut text = sentence(&mut rng);
        if m.is_multiple_of(97) {
            text.push_str(" WebSocket 断线后重连");
        }
        TEXT_BYTES.fetch_add(text.len(), Ordering::Relaxed);
        h.append_message(id, role, text);
        if m.is_multiple_of(7) {
            h.touch_file(id, format!("crates/app/src/{}", rng.pick(EN)));
        }
    }
    h.flush().unwrap();
    ids
}

fn file_size(db: &Path) -> u64 {
    let mut total = 0;
    for suffix in ["", "-wal"] {
        total += std::fs::metadata(format!("{}{suffix}", db.display()))
            .map(|m| m.len())
            .unwrap_or(0);
    }
    total
}

#[test]
fn ten_thousand_messages_search_fast() {
    let dir = TempDir::new("perf");
    let h = History::new(dir.db());
    let started = Instant::now();
    let before = TEXT_BYTES.load(Ordering::Relaxed);
    fill(&h, 250, 10_000, 7);
    let text_bytes = TEXT_BYTES.load(Ordering::Relaxed) - before;
    let fill_ms = started.elapsed().as_millis();
    let main_size = std::fs::metadata(dir.db()).unwrap().len();
    let limit_ms = if cfg!(debug_assertions) { 250.0 } else { 20.0 };
    let mut report = Vec::new();
    for query in [
        "重连",
        "快照",
        "重新连接",
        "socket",
        "settings.json",
        "缓存 render",
        "修",
        "x",
    ] {
        for scope in [Scope::All, Scope::Workspace("/w/1".into())] {
            // Warm once, then take the median of seven (the CI box runs other jobs too).
            let _ = h.search(&SearchQuery::new(query, scope.clone())).unwrap();
            let mut times = Vec::new();
            let mut count = 0;
            for _ in 0..7 {
                let t = Instant::now();
                count = h
                    .search(&SearchQuery::new(query, scope.clone()))
                    .unwrap()
                    .len();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            let (median, worst) = (times[3], times[6]);
            report.push(format!(
                "{query}@{}={median:.2}ms(max {worst:.2})/{count}",
                if scope == Scope::All { "all" } else { "ws" }
            ));
            assert!(median < limit_ms, "{query}: {median:.2} ms");
        }
    }
    eprintln!(
        "event=history_perf messages=10000 sessions=250 fill_ms={fill_ms} text_bytes={text_bytes} db_bytes={main_size} {}",
        report.join(" ")
    );
}

#[test]
fn hard_delete_reclaims_space_and_index() {
    let dir = TempDir::new("delete");
    let h = History::new(dir.db());
    // Baseline: schema only.
    let probe = session(&h, "/w/probe", "codex", "probe");
    h.delete_session(probe).unwrap();
    h.flush().unwrap();
    let empty = file_size(&dir.db());

    let ids = fill(&h, 250, 10_000, 11);
    h.pin(ids[0]);
    h.pin(ids[1]);
    h.flush().unwrap();
    let full = file_size(&dir.db());
    let (tri, cjk) = h.index_counts().unwrap();
    assert!(tri > 10_000 && cjk > 10_000);
    assert!(!search(&h, "快照", Scope::All).is_empty());

    // Single delete removes the session's rows and pin.
    let removed = h.delete_session(ids[0]).unwrap();
    assert_eq!(removed, 1);
    assert!(h.session(ids[0]).unwrap().is_none());
    assert!(h.messages(ids[0]).unwrap().is_empty());
    assert!(h.files(ids[0]).unwrap().is_empty());
    let pinned = h
        .list(
            &Scope::All,
            &Filter {
                pinned_only: true,
                ..Default::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(
        pinned.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![ids[1]]
    );
    assert!(
        search(&h, "快照", Scope::All)
            .iter()
            .all(|hit| hit.session.id != ids[0])
    );

    // Archived-only bulk delete within a workspace.
    let ws1: Vec<SessionId> = h
        .list(&Scope::Workspace("/w/1".into()), &Filter::default(), 1000)
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    for id in &ws1 {
        h.set_archived(*id, true);
    }
    h.flush().unwrap();
    assert_eq!(
        h.delete_archived(&Scope::Workspace("/w/1".into())).unwrap(),
        ws1.len()
    );
    // Whole workspace.
    let ws2 = h
        .list(&Scope::Workspace("/w/2".into()), &Filter::default(), 1000)
        .unwrap()
        .len();
    assert_eq!(h.delete_workspace(Path::new("/w/2")).unwrap(), ws2);
    // Everything else.
    let rest: Vec<SessionId> = h
        .list(
            &Scope::All,
            &Filter {
                archived: Archived::Include,
                ..Default::default()
            },
            10_000,
        )
        .unwrap()
        .iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(h.delete_sessions(&rest).unwrap(), rest.len());
    h.flush().unwrap();

    assert_eq!(h.index_counts().unwrap(), (0, 0), "orphaned FTS rows");
    assert!(search(&h, "快照", Scope::All).is_empty());
    assert!(search(&h, "socket", Scope::All).is_empty());
    let after = file_size(&dir.db());
    eprintln!(
        "event=history_delete empty_bytes={empty} full_bytes={full} after_delete_bytes={after}"
    );
    assert!(after <= empty + 64 * 1024, "empty={empty} after={after}");
}

#[test]
fn writer_and_reader_run_concurrently() {
    let dir = TempDir::new("concurrent");
    let h = Arc::new(History::new(dir.db()));
    let id = session(&h, "/w", "claude-code", "并发");
    h.flush().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let h = h.clone();
        let done = done.clone();
        thread::spawn(move || {
            let mut searches = 0;
            let mut last = 0;
            while !done.load(Ordering::Relaxed) {
                let n = h
                    .search(&SearchQuery::new("重连", Scope::All))
                    .unwrap()
                    .len();
                assert!(n <= 1);
                let count = h.messages(id).unwrap().len();
                assert!(count >= last, "reader went backwards");
                last = count;
                searches += 1;
            }
            searches
        })
    };
    let writers: Vec<_> = (0..3)
        .map(|w| {
            let h = h.clone();
            thread::spawn(move || {
                for i in 0..500 {
                    h.append_message(id, Role::Agent, format!("写入 {w}-{i} 断线重连"));
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    h.flush().unwrap();
    done.store(true, Ordering::Relaxed);
    let searches = reader.join().unwrap();
    assert!(searches > 0);
    let messages = h.messages(id).unwrap();
    assert_eq!(messages.len(), 1500);
    let seqs: Vec<i64> = messages.iter().map(|m| m.seq).collect();
    assert_eq!(seqs, (0..1500).collect::<Vec<_>>());
}

#[test]
fn write_errors_surface_on_flush() {
    let dir = TempDir::new("errors");
    let h = History::new(dir.db());
    h.append_message(SessionId(424242), Role::User, "没有这个会话");
    let err = h.flush().unwrap_err();
    assert!(err.to_string().contains("会话历史数据库出错"), "{err}");
    // Reported once.
    h.flush().unwrap();
    // Without waiting: the error shows on a call after the writer got to it.
    h.append_message(SessionId(424242), Role::User, "还是没有");
    let started = Instant::now();
    let error = loop {
        if let Some(error) = h.take_error() {
            break error;
        }
        assert!(started.elapsed().as_secs() < 10, "the error never surfaced");
        thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(error.contains("会话历史数据库出错"), "{error}");
    assert_eq!(h.take_error(), None);
}

#[test]
fn bundled_sqlite_has_fts5_without_the_trimmed_extras() {
    // .cargo/config.toml passes LIBSQLITE3_FLAGS; see docs/adr/0004.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let mut stmt = conn.prepare("PRAGMA compile_options").unwrap();
    let options: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(options.iter().any(|o| o == "ENABLE_FTS5"), "{options:?}");
    assert!(
        options.iter().any(|o| o == "OMIT_LOAD_EXTENSION"),
        "{options:?}"
    );
    assert!(!options.iter().any(|o| o == "ENABLE_FTS3"), "{options:?}");
    assert!(!options.iter().any(|o| o == "ENABLE_RTREE"), "{options:?}");
}

#[test]
fn long_threads_page_backwards() {
    let dir = TempDir::new("page");
    let h = History::new(dir.db());
    let id = session(&h, "/w", "codex", "分页");
    for i in 0..25 {
        h.append_message(id, Role::Agent, format!("m{i}"));
    }
    h.flush().unwrap();
    assert_eq!(h.message_count(id).unwrap(), 25);
    let tail = h.messages_page(id, None, 10).unwrap();
    assert_eq!(tail.first().unwrap().seq, 15);
    assert_eq!(tail.last().unwrap().seq, 24);
    let older = h.messages_page(id, Some(15), 10).unwrap();
    assert_eq!((older[0].seq, older[9].seq), (5, 14));
    assert_eq!(h.messages_page(id, Some(5), 10).unwrap().len(), 5);
}

#[test]
fn a_common_word_still_finds_the_newest_session() {
    let dir = TempDir::new("common");
    let history = History::new(dir.db());
    // More matches for "cargo" than one term keeps, all in an old session.
    let old = session(&history, "/w/a", "claude-code", "旧会话");
    for i in 0..20_050 {
        history.append_message(old, Role::Agent, format!("cargo build {i}"));
    }
    let new = session(&history, "/w/a", "claude-code", "新会话");
    history.append_message(new, Role::Agent, "cargo deadlock in the reader");
    history.flush().unwrap();
    let hits = search(&history, "cargo deadlock", Scope::All);
    assert!(hits.iter().any(|hit| hit.session.id == new), "{hits:?}");
}
