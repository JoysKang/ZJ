//! Agent panel logic that does not need GPUI: time buckets and labels for the session list,
//! the composer's `@` mention and attachments, restoring threads from history, and how a
//! tool call is summarized on its card. Rendering lives in `prototype/agent_*.rs`.

use std::{ops::Range, path::PathBuf};
use workspace_editor_agent::{
    Glyph, PromptPart, ToolCall, ToolContent, ToolKind, ToolStatus,
    thread::{Item, Status, ToolCard},
};
use workspace_editor_agent_history::{Message, Role, SessionStatus, SessionSummary};

const DAY_MS: i64 = 86_400_000;

/// Seconds east of UTC at `ms` (local time zone, daylight saving included).
pub fn local_offset(ms: i64) -> i64 {
    let secs = (ms / 1000) as libc::time_t;
    // SAFETY: `localtime_r` writes only into the `tm` we pass and reads `secs`; both live on
    // this stack frame for the whole call. A zeroed `tm` is a valid value of the C struct.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return 0;
        }
        tm.tm_gmtoff
    }
}

/// Local calendar day number of `ms` (days since 1970-01-01 in local time).
fn local_day(ms: i64, offset: i64) -> i64 {
    (ms + offset * 1000).div_euclid(DAY_MS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    Pinned,
    Today,
    Yesterday,
    Week,
    Older,
}

impl Bucket {
    pub fn label(self) -> &'static str {
        match self {
            Bucket::Pinned => "已钉住",
            Bucket::Today => "今天",
            Bucket::Yesterday => "昨天",
            Bucket::Week => "本周",
            Bucket::Older => "更早",
        }
    }
}

pub fn bucket(session: &SessionSummary, now: i64, offset: i64) -> Bucket {
    if session.pinned() {
        return Bucket::Pinned;
    }
    match local_day(now, offset) - local_day(session.updated_at, offset) {
        i64::MIN..=0 => Bucket::Today,
        1 => Bucket::Yesterday,
        2..=6 => Bucket::Week,
        _ => Bucket::Older,
    }
}

/// (year, month, day, weekday 0 = Monday, hour, minute) of a local day / time.
fn civil(ms: i64, offset: i64) -> (i64, u32, u32, u32, u32, u32) {
    let local = ms / 1000 + offset;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    // Howard Hinnant's days_from_civil inverse.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    // 1970-01-01 was a Thursday (weekday 3 counting from Monday).
    let weekday = (days + 3).rem_euclid(7) as u32;
    (
        year,
        month,
        day,
        weekday,
        (secs / 3600) as u32,
        (secs % 3600 / 60) as u32,
    )
}

const WEEKDAYS: [&str; 7] = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];

/// The time shown at the end of a session row: a clock time today and yesterday (the group
/// header says which day), the weekday this week, a date before.
pub fn row_time(ms: i64, now: i64, offset: i64) -> String {
    let (year, month, day, weekday, hour, minute) = civil(ms, offset);
    let days = local_day(now, offset) - local_day(ms, offset);
    match days {
        i64::MIN..=1 => format!("{hour:02}:{minute:02}"),
        2..=6 => WEEKDAYS[weekday as usize].to_string(),
        _ if civil(now, offset).0 == year => format!("{month}月{day}日"),
        _ => format!("{year}年{month}月{day}日"),
    }
}

/// Relative age for pinned rows and search results ("3 天前", "昨天", "10:24").
pub fn relative_time(ms: i64, now: i64, offset: i64) -> String {
    let days = local_day(now, offset) - local_day(ms, offset);
    let (_, month, day, _, hour, minute) = civil(ms, offset);
    match days {
        i64::MIN..=0 => format!("今天 {hour:02}:{minute:02}"),
        1 => "昨天".into(),
        2..=6 => format!("{days} 天前"),
        _ => format!("{month}月{day}日"),
    }
}

/// Status of a row: a live session wins over what the database last recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    None,
    Running,
    Awaiting,
    Unread,
    Error,
}

impl RowStatus {
    pub fn of_thread(status: Status, unread: bool) -> Self {
        match status {
            Status::Awaiting => RowStatus::Awaiting,
            Status::Running => RowStatus::Running,
            Status::Error => RowStatus::Error,
            Status::Idle if unread => RowStatus::Unread,
            Status::Idle => RowStatus::None,
        }
    }

    /// A session that is not live: running / awaiting in the database means ZJ quit
    /// mid-turn, which is shown as nothing rather than a spinner that never stops.
    pub fn of_stored(status: SessionStatus) -> Self {
        match status {
            SessionStatus::Failed => RowStatus::Error,
            _ => RowStatus::None,
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            RowStatus::Running => Some("运行中"),
            RowStatus::Awaiting => Some("待批准"),
            RowStatus::Unread => Some("完成，未读"),
            RowStatus::Error => Some("出错"),
            RowStatus::None => None,
        }
    }
}

/// What the history database records for a live thread.
pub fn stored_status(status: Status, had_turns: bool) -> SessionStatus {
    match status {
        Status::Running => SessionStatus::Running,
        Status::Awaiting => SessionStatus::Awaiting,
        Status::Error => SessionStatus::Failed,
        Status::Idle if had_turns => SessionStatus::Completed,
        Status::Idle => SessionStatus::Idle,
    }
}

/// One row of the session list: a group header or a session (index into the rows given).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListRow {
    Header { bucket: Bucket, count: usize },
    Session(usize),
}

/// Groups sessions (already pinned-first, then newest-first) under 已钉住 / 今天 / 昨天 /
/// 本周 / 更早. Pinned sessions keep their pin order.
pub fn group_rows(sessions: &[SessionSummary], now: i64, offset: i64) -> Vec<ListRow> {
    let order = [
        Bucket::Pinned,
        Bucket::Today,
        Bucket::Yesterday,
        Bucket::Week,
        Bucket::Older,
    ];
    let buckets: Vec<Bucket> = sessions.iter().map(|s| bucket(s, now, offset)).collect();
    let mut rows = Vec::with_capacity(sessions.len() + order.len());
    for group in order {
        let members: Vec<usize> = (0..sessions.len())
            .filter(|&i| buckets[i] == group)
            .collect();
        if members.is_empty() {
            continue;
        }
        rows.push(ListRow::Header {
            bucket: group,
            count: members.len(),
        });
        rows.extend(members.into_iter().map(ListRow::Session));
    }
    rows
}

/// Where a dragged pinned row lands: `move_pin(session, before)`. `None` when the drop keeps
/// the order.
pub fn pin_drop(pinned: &[i64], dragged: i64, target: i64) -> Option<Option<i64>> {
    if dragged == target {
        return None;
    }
    let from = pinned.iter().position(|&id| id == dragged);
    let to = pinned.iter().position(|&id| id == target)?;
    match from {
        // Dropping onto a row below moves after it; onto a row above moves before it.
        Some(from) if from < to => Some(pinned.get(to + 1).copied()),
        _ => Some(Some(target)),
    }
}

/// Byte ranges of `query`'s terms in `text`, case-insensitive (title highlights).
pub fn mark_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let lower = text.to_lowercase();
    // Lowercasing can change byte lengths; only highlight when it did not.
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for term in query.split_whitespace() {
        let term = term.trim_matches('"').to_lowercase();
        if term.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(at) = lower[from..].find(&term) {
            let start = from + at;
            ranges.push(start..start + term.len());
            from = start + term.len();
        }
    }
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Context attached to the next prompt (chips above the composer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attachment {
    File(PathBuf),
    /// 1-based inclusive lines with the selected text.
    Selection {
        path: PathBuf,
        start: u32,
        end: u32,
        text: String,
    },
}

impl Attachment {
    pub fn label(&self) -> String {
        match self {
            Attachment::File(path) => file_name(path),
            Attachment::Selection {
                path, start, end, ..
            } => {
                if start == end {
                    format!("{} 第 {start} 行", file_name(path))
                } else {
                    format!("{} {start}–{end}", file_name(path))
                }
            }
        }
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            Attachment::File(path) | Attachment::Selection { path, .. } => path,
        }
    }
}

pub fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The prompt: attachments first (as resource links / embedded selections), then the text.
pub fn prompt_parts(text: &str, attachments: &[Attachment]) -> Vec<PromptPart> {
    let mut parts: Vec<PromptPart> = attachments
        .iter()
        .map(|a| match a {
            Attachment::File(path) => PromptPart::File(path.clone()),
            Attachment::Selection {
                path,
                start,
                end,
                text,
            } => PromptPart::Selection {
                path: path.clone(),
                start_line: *start,
                end_line: *end,
                text: Some(text.clone()),
            },
        })
        .collect();
    parts.push(PromptPart::Text(text.to_string()));
    parts
}

/// An `@` mention being typed: the byte range from `@` to the cursor and the query after it.
pub fn mention_at(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    // `a@b` inside a word (an e-mail address) is not a mention.
    if before[..at]
        .chars()
        .next_back()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    Some((at..cursor, query.to_string()))
}

/// Restores a thread from stored messages (oldest first).
pub fn items_from_messages(messages: &[Message]) -> Vec<Item> {
    messages
        .iter()
        .map(|m| match m.role {
            Role::User => Item::User {
                text: m.text.clone(),
                attachments: Vec::new(),
            },
            Role::Agent => Item::Agent {
                text: m.text.clone(),
                streaming: false,
            },
            Role::Thought => Item::Thought {
                text: m.text.clone(),
                streaming: false,
            },
            Role::Tool => Item::Tool(ToolCard {
                call: ToolCall {
                    id: format!("history-{}", m.seq),
                    title: m.text.clone(),
                    kind: ToolKind::Other,
                    status: ToolStatus::Completed,
                    locations: Vec::new(),
                    content: Vec::new(),
                },
                added: 0,
                removed: 0,
            }),
            Role::System => Item::Notice {
                text: m.text.clone(),
                error: false,
            },
        })
        .collect()
}

/// Index into `theme::Colors::glyphs` and the Lucide icon for an agent glyph.
pub fn glyph_index(glyph: Glyph) -> usize {
    match glyph {
        Glyph::Claude => 0,
        Glyph::Codex => 1,
        Glyph::DeepSeek => 2,
        Glyph::Gemini => 3,
        Glyph::Generic => 4,
    }
}

/// A tool card's verb and target, e.g. ("编辑", "binance.rs").
pub fn tool_summary(call: &ToolCall) -> (&'static str, String) {
    let creates = call.kind == ToolKind::Edit
        && call
            .content
            .iter()
            .any(|c| matches!(c, ToolContent::Diff { old_text: None, .. }));
    let verb = match call.kind {
        ToolKind::Read => "读取",
        ToolKind::Edit if creates => "新建",
        ToolKind::Edit => "编辑",
        ToolKind::Delete => "删除",
        ToolKind::Move => "移动",
        ToolKind::Search => "搜索",
        ToolKind::Execute => "运行",
        ToolKind::Think => "思考",
        ToolKind::Fetch => "获取",
        ToolKind::SwitchMode => "切换模式",
        ToolKind::Other => "",
    };
    let files: Vec<String> = call.locations.iter().map(|l| file_name(&l.path)).collect();
    let detail = match call.kind {
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move
            if !files.is_empty() =>
        {
            if files.len() > 3 {
                format!("{} 等 {} 个文件", files[..3].join(", "), files.len())
            } else {
                files.join(", ")
            }
        }
        _ => call.title.trim_matches('`').to_string(),
    };
    (verb, detail)
}

/// Text output of a tool call (for the expanded card).
pub fn tool_output(call: &ToolCall) -> String {
    let mut out = String::new();
    for content in &call.content {
        match content {
            ToolContent::Text(text) if !text.trim().is_empty() => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text.trim_end());
            }
            ToolContent::Diff { path, .. } => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("修改 {}", path.display()));
            }
            _ => {}
        }
    }
    out
}

/// (fraction used, percent label) for the context ring.
pub fn usage_ring(usage: Option<(u64, u64)>) -> Option<(f32, String)> {
    let (used, size) = usage?;
    if size == 0 {
        return None;
    }
    let fraction = (used as f64 / size as f64).clamp(0.0, 1.0) as f32;
    Some((fraction, format!("{}%", (fraction * 100.0).round() as u32)))
}

/// Direction B: the live strip appears once two or more sessions need attention or run.
pub fn show_live_strip(active_sessions: usize) -> bool {
    active_sessions >= 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use workspace_editor_agent_history::{SessionId, TitleSource};

    fn summary(id: i64, updated_at: i64, pinned: Option<f64>) -> SessionSummary {
        SessionSummary {
            id: SessionId(id),
            workspace_root: "/w".into(),
            agent_id: "claude-code".into(),
            acp_session_id: None,
            title: format!("t{id}"),
            title_source: TitleSource::Placeholder,
            repo: None,
            branch: None,
            created_at: updated_at,
            updated_at,
            pinned_rank: pinned,
            archived: false,
            status: SessionStatus::Idle,
            lines_added: 0,
            lines_removed: 0,
        }
    }

    // 2026-10-02 10:30 UTC is a Friday.
    const NOW: i64 = 1_790_937_000_000;
    const HOUR: i64 = 3_600_000;

    #[test]
    fn civil_dates_and_buckets() {
        assert_eq!(civil(NOW, 0), (2026, 10, 2, 4, 10, 30));
        assert_eq!(civil(0, 0), (1970, 1, 1, 3, 0, 0));
        // UTC+8: 18:30 local.
        assert_eq!(civil(NOW, 8 * 3600).4, 18);
        let rows = [
            summary(1, NOW - HOUR, None),
            summary(2, NOW - 24 * HOUR, None),
            summary(3, NOW - 4 * 24 * HOUR, None),
            summary(4, NOW - 30 * 24 * HOUR, None),
            summary(5, NOW - 60 * 24 * HOUR, Some(1.0)),
        ];
        let buckets: Vec<Bucket> = rows.iter().map(|s| bucket(s, NOW, 0)).collect();
        assert_eq!(
            buckets,
            [
                Bucket::Today,
                Bucket::Yesterday,
                Bucket::Week,
                Bucket::Older,
                Bucket::Pinned
            ]
        );
        assert_eq!(row_time(NOW - HOUR, NOW, 0), "09:30");
        assert_eq!(row_time(NOW - 4 * 24 * HOUR, NOW, 0), "周一");
        assert_eq!(row_time(NOW - 30 * 24 * HOUR, NOW, 0), "9月2日");
        assert_eq!(relative_time(NOW - 3 * 24 * HOUR, NOW, 0), "3 天前");
        assert_eq!(relative_time(NOW - 24 * HOUR, NOW, 0), "昨天");
        let grouped = group_rows(&rows, NOW, 0);
        assert_eq!(
            grouped[..2],
            [
                ListRow::Header {
                    bucket: Bucket::Pinned,
                    count: 1
                },
                ListRow::Session(4)
            ]
        );
        assert_eq!(grouped.len(), 10);
        // The local offset is whatever the machine says; it must not panic.
        let _ = local_offset(NOW);
    }

    #[test]
    fn pinned_rows_reorder_by_drop_target() {
        let pinned = [10, 20, 30];
        // 30 dropped on 10: before 10.
        assert_eq!(pin_drop(&pinned, 30, 10), Some(Some(10)));
        // 10 dropped on 20: after 20, i.e. before 30.
        assert_eq!(pin_drop(&pinned, 10, 20), Some(Some(30)));
        // 10 dropped on the last row: to the end.
        assert_eq!(pin_drop(&pinned, 10, 30), Some(None));
        // An unpinned session dropped on a pinned row is pinned before it.
        assert_eq!(pin_drop(&pinned, 99, 20), Some(Some(20)));
        assert_eq!(pin_drop(&pinned, 20, 20), None);
    }

    #[test]
    fn mentions_and_marks() {
        assert_eq!(
            mention_at("看看 @src/ma", 14),
            Some((7..14, "src/ma".into()))
        );
        assert_eq!(mention_at("@", 1), Some((0..1, String::new())));
        assert_eq!(mention_at("mail a@b", 8), None);
        assert_eq!(mention_at("@a b", 4), None);
        assert_eq!(
            mark_ranges("修复重连后序列号缺口", "重连 缺口"),
            vec![6..12, 24..30]
        );
        assert_eq!(mark_ranges("WebSocket", "socket"), vec![3..9]);
    }

    #[test]
    fn prompts_and_history_items() {
        let parts = prompt_parts(
            "修一下",
            &[
                Attachment::File("/w/a.rs".into()),
                Attachment::Selection {
                    path: "/w/b.rs".into(),
                    start: 3,
                    end: 5,
                    text: "x".into(),
                },
            ],
        );
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2], PromptPart::Text("修一下".into()));
        assert_eq!(
            Attachment::Selection {
                path: "/w/b.rs".into(),
                start: 137,
                end: 145,
                text: String::new()
            }
            .label(),
            "b.rs 137–145"
        );
        let items = items_from_messages(&[
            Message {
                seq: 1,
                role: Role::User,
                text: "q".into(),
                created_at: 0,
            },
            Message {
                seq: 2,
                role: Role::Tool,
                text: "Read a.rs".into(),
                created_at: 0,
            },
        ]);
        assert!(matches!(&items[0], Item::User { text, .. } if text == "q"));
        assert!(matches!(&items[1], Item::Tool(card) if card.call.title == "Read a.rs"));
    }

    #[test]
    fn tool_cards_and_statuses() {
        let call = ToolCall {
            id: "1".into(),
            title: "Read".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Completed,
            locations: ["a.rs", "b.rs", "c.rs", "d.rs"]
                .iter()
                .map(|p| workspace_editor_agent::Location {
                    path: Path::new("/w").join(p),
                    line: None,
                })
                .collect(),
            content: vec![],
        };
        assert_eq!(
            tool_summary(&call),
            ("读取", "a.rs, b.rs, c.rs 等 4 个文件".to_string())
        );
        assert_eq!(usage_ring(Some((38, 100))).unwrap().1, "38%");
        assert_eq!(usage_ring(Some((1, 0))), None);
        assert_eq!(RowStatus::of_thread(Status::Idle, true), RowStatus::Unread);
        assert_eq!(
            RowStatus::of_stored(SessionStatus::Running),
            RowStatus::None
        );
        assert_eq!(stored_status(Status::Idle, true), SessionStatus::Completed);
        assert!(!show_live_strip(1) && show_live_strip(2));
    }
}
