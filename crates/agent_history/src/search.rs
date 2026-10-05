//! Ranked search over titles, messages, touched files and session metadata.
//!
//! Each whitespace-separated term is looked up on its own (trigram ≥ 3 characters, CJK bigram
//! for two CJK characters, `LIKE` otherwise); a session matches when every term matches
//! somewhere in it. One hit per session, pinned sessions first, then relevance × recency.

use crate::{
    Filter, Result, Role, Scope, SessionSummary, list_sessions, now_ms,
    schema::{DOC_FILES, DOC_META, DOC_TITLE, decode_session_doc},
    text::{Snippet, Term, TermPlan, fts_phrase, like_pattern, parse_query, snippet},
};
use rusqlite::{Connection, OptionalExtension};
use std::collections::HashMap;

/// Most index rows considered per term (in rowid order). A term this common is not useful for
/// finding a session anyway; titles, files and metadata use negative rowids and come first.
const ROWS_PER_TERM: usize = 20_000;
const RECENCY_HALF_LIFE_DAYS: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub title: bool,
    pub message: bool,
    pub file: bool,
    /// Agent id, repository and branch.
    pub meta: bool,
}

impl Kinds {
    pub const ALL: Kinds = Kinds {
        title: true,
        message: true,
        file: true,
        meta: true,
    };
}

impl Default for Kinds {
    fn default() -> Self {
        Kinds::ALL
    }
}

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub text: String,
    pub scope: Scope,
    pub filter: Filter,
    pub kinds: Kinds,
    pub limit: usize,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>, scope: Scope) -> Self {
        Self {
            text: text.into(),
            scope,
            filter: Filter::default(),
            kinds: Kinds::ALL,
            limit: 50,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HitKind {
    Title,
    Message { seq: i64, role: Role },
    File,
    Meta,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub session: SessionSummary,
    pub kind: HitKind,
    pub snippet: Snippet,
    pub score: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Doc {
    Message(i64),
    Session(i64, i64),
}

#[derive(Clone, Copy)]
struct RowHit {
    session: i64,
    doc: Doc,
    weight: f64,
}

fn kind_weight(doc: Doc) -> f64 {
    match doc {
        Doc::Session(_, DOC_TITLE) => 4.0,
        Doc::Session(_, DOC_FILES) => 2.0,
        Doc::Session(_, _) => 1.5,
        Doc::Message(_) => 1.0,
    }
}

fn kind_enabled(kinds: &Kinds, doc: Doc) -> bool {
    match doc {
        Doc::Session(_, DOC_TITLE) => kinds.title,
        Doc::Session(_, DOC_FILES) => kinds.file,
        Doc::Session(_, _) => kinds.meta,
        Doc::Message(_) => kinds.message,
    }
}

pub(crate) fn run(conn: &Connection, query: &SearchQuery) -> Result<Vec<SearchHit>> {
    let terms = parse_query(&query.text);
    if terms.is_empty() || query.limit == 0 {
        return Ok(Vec::new());
    }
    let sessions: HashMap<i64, SessionSummary> =
        list_sessions(conn, &query.scope, &query.filter, None)?
            .into_iter()
            .map(|s| (s.id.0, s))
            .collect();
    if sessions.is_empty() {
        return Ok(Vec::new());
    }

    // Per term: rows that matched, already restricted to visible sessions and enabled kinds.
    let mut per_term: Vec<Vec<RowHit>> = Vec::with_capacity(terms.len());
    for term in &terms {
        let rows = term_rows(conn, term)?
            .into_iter()
            .filter(|r| sessions.contains_key(&r.session) && kind_enabled(&query.kinds, r.doc))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        per_term.push(rows);
    }

    struct Acc {
        /// Per term: strongest kind weight and number of matching documents.
        per_term: Vec<(f64, usize)>,
        /// Per document: how many terms it contains.
        docs: HashMap<Doc, usize>,
    }
    let mut acc: HashMap<i64, Acc> = HashMap::new();
    for (index, rows) in per_term.iter().enumerate() {
        for row in rows {
            let entry = acc.entry(row.session).or_insert_with(|| Acc {
                per_term: vec![(0.0, 0); terms.len()],
                docs: HashMap::new(),
            });
            let (best, count) = &mut entry.per_term[index];
            *best = best.max(row.weight);
            *count += 1;
            *entry.docs.entry(row.doc).or_insert(0) += 1;
        }
    }

    let now = now_ms();
    let mut ranked: Vec<(i64, Doc, f64)> = acc
        .into_iter()
        .filter(|(_, a)| a.per_term.iter().all(|(_, count)| *count > 0))
        .map(|(session, a)| {
            // Where it matched (title > files > metadata > message) plus how often.
            let relevance: f64 = a
                .per_term
                .iter()
                .map(|(best, count)| best + (1.0 + *count as f64).ln())
                .sum();
            let updated = sessions[&session].updated_at;
            let age_days = ((now - updated).max(0) as f64) / 86_400_000.0;
            let recency = 1.0 + 0.5 * 0.5f64.powf(age_days / RECENCY_HALF_LIFE_DAYS);
            // Snippet source: the document with the most terms, then the strongest kind,
            // then the latest message.
            let doc = a
                .docs
                .iter()
                .max_by(|x, y| {
                    x.1.cmp(y.1)
                        .then_with(|| kind_weight(*x.0).total_cmp(&kind_weight(*y.0)))
                        .then_with(|| doc_order(*x.0).cmp(&doc_order(*y.0)))
                })
                .map(|(d, _)| *d)
                .expect("a matching session has at least one document");
            (session, doc, relevance * recency)
        })
        .collect();
    ranked.sort_by(|a, b| {
        let sa = &sessions[&a.0];
        let sb = &sessions[&b.0];
        let pin = |s: &SessionSummary| s.pinned_rank.unwrap_or(f64::INFINITY);
        sa.pinned_rank
            .is_none()
            .cmp(&sb.pinned_rank.is_none())
            .then_with(|| pin(sa).total_cmp(&pin(sb)))
            .then_with(|| b.2.total_cmp(&a.2))
            .then_with(|| sb.updated_at.cmp(&sa.updated_at))
    });
    ranked.truncate(query.limit);

    let mut hits = Vec::with_capacity(ranked.len());
    for (session, doc, score) in ranked {
        let summary = sessions[&session].clone();
        let (kind, text) = doc_text(conn, doc, &summary)?;
        hits.push(SearchHit {
            snippet: snippet(&text, &terms),
            session: summary,
            kind,
            score,
        });
    }
    Ok(hits)
}

/// Final tie-break: later messages win.
fn doc_order(doc: Doc) -> i64 {
    match doc {
        Doc::Session(_, kind) => -kind,
        Doc::Message(id) => id,
    }
}

fn term_rows(conn: &Connection, term: &Term) -> Result<Vec<RowHit>> {
    match term.plan {
        TermPlan::Trigram => fts_rows(conn, "fts_tri", &fts_phrase(&term.text)),
        TermPlan::Bigram => fts_rows(conn, "fts_cjk", &fts_phrase(&term.text)),
        TermPlan::Like => like_rows(conn, &like_pattern(&term.text)),
    }
}

/// Matching rowids only: computing bm25() for thousands of rows costs more than the whole
/// rest of the search, and per-session aggregation ranks well enough without it.
/// Session documents (negative rowids) and messages are capped separately, messages newest
/// first, so a common word cannot crowd out titles or the recent sessions.
fn fts_rows(conn: &Connection, table: &str, phrase: &str) -> Result<Vec<RowHit>> {
    let mut raw: Vec<(i64, Option<i64>)> = Vec::new();
    for filter in [
        format!("{table}.rowid < 0"),
        format!("{table}.rowid > 0 ORDER BY {table}.rowid DESC"),
    ] {
        let mut stmt = conn.prepare(&format!(
            "SELECT {table}.rowid, m.session_id FROM {table} \
             LEFT JOIN messages m ON m.id = {table}.rowid \
             WHERE {table} MATCH ?1 AND {filter} LIMIT {ROWS_PER_TERM}"
        ))?;
        for row in stmt.query_map([phrase], |r| Ok((r.get(0)?, r.get(1)?)))? {
            raw.push(row?);
        }
    }
    let mut rows = Vec::with_capacity(raw.len());
    for (rowid, owner) in raw {
        let (session, doc) = if rowid > 0 {
            match owner {
                Some(session) => (session, Doc::Message(rowid)),
                None => continue,
            }
        } else {
            let (session, kind) = decode_session_doc(rowid);
            (session, Doc::Session(session, kind))
        };
        rows.push(RowHit {
            session,
            doc,
            weight: kind_weight(doc),
        });
    }
    Ok(rows)
}

/// One- and two-character fallbacks: titles, file paths and metadata only (message bodies
/// would turn a single character into noise).
fn like_rows(conn: &Connection, pattern: &str) -> Result<Vec<RowHit>> {
    let mut rows = Vec::new();
    let mut push = |session: i64, kind: i64| {
        let doc = Doc::Session(session, kind);
        rows.push(RowHit {
            session,
            doc,
            weight: kind_weight(doc),
        });
    };
    let mut stmt = conn.prepare("SELECT id FROM sessions WHERE title LIKE ?1 ESCAPE '\\'")?;
    for id in stmt.query_map([pattern], |r| r.get(0))? {
        push(id?, DOC_TITLE);
    }
    let mut stmt = conn
        .prepare("SELECT DISTINCT session_id FROM session_files WHERE path LIKE ?1 ESCAPE '\\'")?;
    for id in stmt.query_map([pattern], |r| r.get(0))? {
        push(id?, DOC_FILES);
    }
    let mut stmt = conn.prepare(
        "SELECT id FROM sessions WHERE agent_id LIKE ?1 ESCAPE '\\' \
         OR repo LIKE ?1 ESCAPE '\\' OR branch LIKE ?1 ESCAPE '\\'",
    )?;
    for id in stmt.query_map([pattern], |r| r.get(0))? {
        push(id?, DOC_META);
    }
    Ok(rows)
}

fn doc_text(conn: &Connection, doc: Doc, summary: &SessionSummary) -> Result<(HitKind, String)> {
    Ok(match doc {
        Doc::Message(id) => {
            let row: Option<(i64, String, String)> = conn
                .query_row(
                    "SELECT seq, role, text FROM messages WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let (seq, role, text) = row.unwrap_or_default();
            (
                HitKind::Message {
                    seq,
                    role: Role::parse(&role),
                },
                text,
            )
        }
        Doc::Session(_, DOC_TITLE) => (HitKind::Title, summary.title.clone()),
        Doc::Session(session, DOC_FILES) => {
            let text: Option<String> = conn.query_row(
                "SELECT group_concat(path, char(10)) FROM session_files WHERE session_id = ?1",
                [session],
                |r| r.get(0),
            )?;
            (HitKind::File, text.unwrap_or_default())
        }
        Doc::Session(_, _) => {
            let meta = [
                Some(summary.agent_id.as_str()),
                summary.repo.as_deref(),
                summary.branch.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
            (HitKind::Meta, meta)
        }
    })
}
