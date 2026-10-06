//! ⌘J: search every stored session (design 04): FTS5 over titles, messages, touched files
//! and metadata, Chinese included. Pinned hits first, then by relevance and recency. Runs in
//! the background with a generation check; nothing is searched while the overlay is closed.

use super::agent::{AgentStore, agent_name, glyph_for, workspace_label};
use super::agent_history::filter_chip;
use super::agent_panel::{glyph_tile, status_mark};
use super::*;
use crate::agent_model::{self, RowStatus};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputState},
        menu::{DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
};
use workspace_editor_agent_history::{
    Archived, Filter, HitKind, Kinds, Scope, SearchHit, SearchQuery, Snippet,
};

/// Typing settles for this long before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KindFilter {
    All,
    Title,
    Message,
    File,
}

enum OverlayRow {
    Header(&'static str, String),
    Hit(usize),
}

pub(super) struct SessionSearch {
    input: Entity<InputState>,
    kind: KindFilter,
    all_workspaces: bool,
    agent: Option<String>,
    archived: bool,
    hits: Vec<SearchHit>,
    rows: Vec<OverlayRow>,
    selected: usize,
    generation: u64,
    elapsed_ms: u128,
    total: usize,
    error: Option<String>,
    list: ListState,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

fn group_rows(hits: &[SearchHit], query: &str) -> Vec<OverlayRow> {
    let pinned: Vec<usize> = (0..hits.len())
        .filter(|&i| hits[i].session.pinned())
        .collect();
    let rest: Vec<usize> = (0..hits.len())
        .filter(|&i| !hits[i].session.pinned())
        .collect();
    let mut rows = Vec::new();
    if !pinned.is_empty() {
        rows.push(OverlayRow::Header("已钉住", pinned.len().to_string()));
        rows.extend(pinned.into_iter().map(OverlayRow::Hit));
    }
    if !rest.is_empty() {
        rows.push(OverlayRow::Header(
            if query.trim().is_empty() {
                "最近"
            } else {
                "最相关"
            },
            if query.trim().is_empty() {
                String::new()
            } else {
                "按相关度 · 近期优先".into()
            },
        ));
        rows.extend(rest.into_iter().map(OverlayRow::Hit));
    }
    rows
}

/// Text with highlighted ranges (search marks).
fn marked(text: &str, ranges: &[std::ops::Range<usize>], colors: theme::Colors) -> StyledText {
    let highlights: Vec<_> = ranges
        .iter()
        .filter(|r| {
            r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)
        })
        .map(|r| {
            (
                r.clone(),
                HighlightStyle {
                    background_color: Some(colors.mark_bg),
                    color: Some(colors.mark_fg),
                    ..Default::default()
                },
            )
        })
        .collect();
    StyledText::new(SharedString::from(text.to_string())).with_highlights(highlights)
}

fn snippet_text(snippet: &Snippet) -> (String, Vec<std::ops::Range<usize>>) {
    let lead = if snippet.leading_ellipsis { "…" } else { "" };
    let mut text = format!("{lead}{}", snippet.text);
    if snippet.trailing_ellipsis {
        text.push('…');
    }
    let shift = lead.len();
    let ranges = snippet
        .highlights
        .iter()
        .map(|r| r.start + shift..r.end + shift)
        .collect();
    (text, ranges)
}

impl Workbench {
    pub(super) fn agent_open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.search.is_some() {
            if let Some(search) = &self.agent.search {
                search.input.update(cx, |input, cx| input.focus(window, cx));
            }
            return;
        }
        self.quick_open = None;
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索会话：标题、消息、文件、分支（中文可用）")
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.agent_run_search(DEBOUNCE, window, cx);
                }
            },
        );
        self.agent.search = Some(SessionSearch {
            input,
            kind: KindFilter::All,
            all_workspaces: false,
            agent: None,
            archived: false,
            hits: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            generation: 0,
            elapsed_ms: 0,
            total: 0,
            error: None,
            list: ListState::new(0, ListAlignment::Top, theme::AGENT_OVERLAY_ROW),
            task: None,
            _subscription: subscription,
        });
        self.agent_run_search(Duration::ZERO, window, cx);
    }

    pub(super) fn agent_close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent.search.take().is_some() {
            if self.agent.visible && self.agent.view == super::agent::AgentView::Thread {
                self.agent_focus_composer(window, cx);
            } else {
                self.focus_active_editor(window, cx);
            }
            cx.notify();
        }
    }

    fn agent_run_search(&mut self, delay: Duration, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = cx
            .try_global::<AgentStore>()
            .and_then(|s| s.history.clone())
        else {
            if let Some(search) = self.agent.search.as_mut() {
                search.error = Some("没有可用的数据目录，会话历史不会保存".into());
            }
            return;
        };
        let root = self.agent_workspace(cx);
        let Some(search) = self.agent.search.as_mut() else {
            return;
        };
        search.generation += 1;
        let generation = search.generation;
        let scope = match (&root, search.all_workspaces) {
            (Some(root), false) => Scope::Workspace(root.clone()),
            _ => Scope::All,
        };
        let filter = Filter {
            agent_id: search.agent.clone(),
            archived: if search.archived {
                Archived::Include
            } else {
                Archived::Exclude
            },
            ..Default::default()
        };
        let kinds = match search.kind {
            KindFilter::All => Kinds::ALL,
            KindFilter::Title => Kinds {
                title: true,
                message: false,
                file: false,
                meta: false,
            },
            KindFilter::Message => Kinds {
                title: false,
                message: true,
                file: false,
                meta: false,
            },
            KindFilter::File => Kinds {
                title: false,
                message: false,
                file: true,
                meta: false,
            },
        };
        search.task = Some(cx.spawn_in(window, async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            // The query as it is now, after the debounce.
            let Ok(Some(text)) = this.update(cx, |this, cx| {
                this.agent
                    .search
                    .as_ref()
                    .map(|s| s.input.read(cx).value().to_string())
            }) else {
                return;
            };
            let job_text = text.clone();
            let result = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let hits = if job_text.trim().is_empty() {
                        store.list(&scope, &filter, 30).map(|rows| {
                            rows.into_iter()
                                .map(|session| SearchHit {
                                    snippet: Snippet {
                                        text: String::new(),
                                        highlights: Vec::new(),
                                        leading_ellipsis: false,
                                        trailing_ellipsis: false,
                                    },
                                    kind: HitKind::Title,
                                    score: 0.,
                                    session,
                                })
                                .collect::<Vec<_>>()
                        })
                    } else {
                        let mut query = SearchQuery::new(job_text, scope.clone());
                        query.filter = filter.clone();
                        query.kinds = kinds;
                        query.limit = 50;
                        store.search(&query)
                    };
                    let total = store
                        .list(
                            &scope,
                            &Filter {
                                archived: Archived::Include,
                                ..Default::default()
                            },
                            100_000,
                        )
                        .map(|r| r.len())
                        .unwrap_or(0);
                    (hits, total, started.elapsed().as_millis())
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(search) = this.agent.search.as_mut() else {
                    return;
                };
                if search.generation != generation {
                    return;
                }
                let (hits, total, elapsed) = result;
                match hits {
                    Ok(hits) => {
                        search.rows = group_rows(&hits, &text);
                        search.hits = hits;
                        search.error = None;
                    }
                    Err(error) => {
                        search.hits.clear();
                        search.rows.clear();
                        search.error = Some(error.to_string());
                    }
                }
                eprintln!("event=agent_search hits={} ms={elapsed}", search.hits.len());
                search.total = total;
                search.elapsed_ms = elapsed;
                search.selected = 0;
                search.list.reset(search.rows.len());
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn agent_search_set(
        &mut self,
        change: impl FnOnce(&mut SessionSearch),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(search) = self.agent.search.as_mut() {
            change(search);
        }
        self.agent_run_search(Duration::ZERO, window, cx);
    }

    fn agent_search_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(search) = self.agent.search.as_mut() else {
            return;
        };
        if search.hits.is_empty() {
            return;
        }
        search.selected = search
            .selected
            .saturating_add_signed(delta)
            .min(search.hits.len() - 1);
        let selected = search.selected;
        if let Some(row) = search
            .rows
            .iter()
            .position(|r| matches!(r, OverlayRow::Hit(i) if *i == selected))
        {
            search.list.scroll_to_reveal_item(row);
        }
        cx.notify();
    }

    fn agent_search_open(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = self.agent.search.as_ref() else {
            return;
        };
        let Some(hit) = search.hits.get(index.unwrap_or(search.selected)) else {
            return;
        };
        let id = hit.session.id;
        self.agent.search = None;
        if !self.agent.visible {
            self.set_agent_panel(true, window, cx);
        }
        self.agent_open_stored(id, window, cx);
        cx.notify();
    }

    pub(super) fn render_agent_search(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let search = self.agent.search.as_ref()?;
        let colors = theme::colors(cx);
        let kind_chip = |id: &'static str, label: &'static str, kind: KindFilter| {
            filter_chip(id, label.into(), search.kind == kind, colors).on_click(cx.listener(
                move |this, _, window, cx| this.agent_search_set(|s| s.kind = kind, window, cx),
            ))
        };
        let agent_menu = {
            let weak = cx.weak_entity();
            let presets: Vec<(String, String)> = self
                .agent
                .presets
                .iter()
                .map(|p| (p.id.clone(), p.display_name.clone()))
                .collect();
            let current = search.agent.clone();
            Button::new("agent-search-agent")
                .ghost()
                .xsmall()
                .label(match &current {
                    Some(id) => format!("Agent：{}", agent_name(&self.agent.presets, id)),
                    None => "Agent：全部".into(),
                })
                .dropdown_caret(true)
                .dropdown_menu(move |menu, _, _| {
                    let all = weak.clone();
                    let mut menu = menu.item(
                        PopupMenuItem::new("全部 Agent")
                            .checked(current.is_none())
                            .on_click(move |_, window, cx| {
                                let _ = all.update(cx, |this, cx| {
                                    this.agent_search_set(|s| s.agent = None, window, cx)
                                });
                            }),
                    );
                    for (id, name) in &presets {
                        let weak = weak.clone();
                        let id = id.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(current.as_deref() == Some(id.as_str()))
                                .on_click(move |_, window, cx| {
                                    let id = id.clone();
                                    let _ = weak.update(cx, |this, cx| {
                                        this.agent_search_set(|s| s.agent = Some(id), window, cx)
                                    });
                                }),
                        );
                    }
                    menu
                })
        };
        let query = search.input.read(cx).value().to_string();
        let count_label = if query.trim().is_empty() {
            String::new()
        } else {
            format!("{} 个会话", search.hits.len())
        };
        let keycap = |label: &'static str| {
            div()
                .px_1()
                .rounded(theme::RADIUS)
                .bg(colors.keycap)
                .text_color(colors.foreground)
                .child(label)
        };
        let visible_rows = search.rows.len().min(theme::AGENT_OVERLAY_ROWS);
        let body: AnyElement = if let Some(error) = &search.error {
            div()
                .p_3()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.deleted)
                .child(error.clone())
                .into_any_element()
        } else if search.rows.is_empty() {
            div()
                .p_4()
                .text_center()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(if query.trim().is_empty() {
                    "还没有会话"
                } else {
                    "没有匹配的会话；中文至少输入两个字，或换成文件名、分支名"
                })
                .into_any_element()
        } else {
            list(
                search.list.clone(),
                cx.processor(|this, index: usize, _, cx| this.render_search_row(index, cx)),
            )
            .w_full()
            .h(theme::AGENT_OVERLAY_ROW * visible_rows.max(3) as f32)
            .into_any_element()
        };
        let panel = v_flex()
            .id("agent-search")
            .key_context("AgentSearch")
            .w(theme::AGENT_OVERLAY_WIDTH)
            .max_w(relative(0.9))
            .rounded(theme::RADIUS_OVERLAY)
            .border_1()
            .border_color(colors.strong_border)
            .bg(colors.panel)
            .shadow_lg()
            .overflow_hidden()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.agent_search_move(-1, cx),
                    "down" => this.agent_search_move(1, cx),
                    "enter" => this.agent_search_open(None, window, cx),
                    "escape" => this.agent_close_search(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .h(theme::AGENT_OVERLAY_INPUT)
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_OVERLAY_INPUT)
                    .child(
                        Icon::new(IconName::Search)
                            .size(theme::ICON_SIZE)
                            .text_color(colors.muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&search.input).appearance(false).bordered(false)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme::TEXT_SECTION)
                            .text_color(colors.muted)
                            .child(count_label),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .flex_wrap()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(kind_chip("agent-search-all", "全部", KindFilter::All))
                    .child(kind_chip("agent-search-title", "标题", KindFilter::Title))
                    .child(kind_chip(
                        "agent-search-message",
                        "消息",
                        KindFilter::Message,
                    ))
                    .child(kind_chip("agent-search-file", "文件", KindFilter::File))
                    .child(
                        filter_chip(
                            "agent-search-scope",
                            self.agent_scope_label(search.all_workspaces, cx),
                            false,
                            colors,
                        )
                        .child(Icon::new(IconName::ChevronDown).size(theme::SMALL_ICON_SIZE))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_search_set(
                                |s| s.all_workspaces = !s.all_workspaces,
                                window,
                                cx,
                            )
                        })),
                    )
                    .child(agent_menu)
                    .child(div().flex_1())
                    .child(
                        filter_chip(
                            "agent-search-archived",
                            "含已归档".into(),
                            search.archived,
                            colors,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_search_set(|s| s.archived = !s.archived, window, cx)
                        })),
                    ),
            )
            .child(div().py_1().child(body))
            .child(
                h_flex()
                    .h(theme::STATUS_HEIGHT + theme::ROW_INSET * 4.)
                    .px_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(keycap("↑"))
                    .child(keycap("↓"))
                    .child("选择")
                    .child(keycap("⏎"))
                    .child("打开")
                    .child(keycap("Esc"))
                    .child("关闭")
                    .child(div().flex_1())
                    .child(format!(
                        "本地 FTS5 · {} 个会话 · {} ms",
                        search.total, search.elapsed_ms
                    )),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .pt(theme::TITLE_HEIGHT + theme::AGENT_OVERLAY_TOP)
                .flex()
                .justify_center()
                .items_start()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.agent_close_search(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(search) = self.agent.search.as_ref() else {
            return div().into_any_element();
        };
        let Some(row) = search.rows.get(index) else {
            return div().into_any_element();
        };
        let hit = match row {
            OverlayRow::Header(label, right) => {
                return h_flex()
                    .w_full()
                    .h(theme::AGENT_GROUP_HEADER)
                    .px_3()
                    .gap_1()
                    .items_end()
                    .pb_1()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .when(*label == "已钉住", |h| {
                        h.child(Icon::new(IconName::Pin).size(theme::SMALL_ICON_SIZE))
                    })
                    .child(div().flex_1().child(*label))
                    .child(right.clone())
                    .into_any_element();
            }
            OverlayRow::Hit(i) => *i,
        };
        let Some(h) = search.hits.get(hit) else {
            return div().into_any_element();
        };
        let selected = hit == search.selected;
        let now = workspace_editor_agent_history::now_ms();
        let offset = agent_model::local_offset(now);
        let query = search.input.read(cx).value().to_string();
        let s = &h.session;
        let live = self.agent.sessions.iter().find(|l| l.db == Some(s.id));
        let status = live.map_or(RowStatus::of_stored(s.status), |l| l.row_status());
        let muted = if selected {
            colors.selected_muted
        } else {
            colors.muted
        };
        let title_marks = agent_model::mark_ranges(&s.title, &query);
        let snippet = match &h.kind {
            HitKind::Message { .. } | HitKind::File => Some(snippet_text(&h.snippet)),
            _ => None,
        };
        let tag = |label: String| {
            div()
                .px_1()
                .rounded(theme::RADIUS)
                .bg(if selected {
                    colors.selected_chip
                } else {
                    colors.keycap
                })
                .text_color(if selected {
                    colors.selected_fg
                } else {
                    colors.foreground
                })
                .child(label)
        };
        let kind_tag = match &h.kind {
            HitKind::Title if !query.trim().is_empty() => Some("标题".to_string()),
            HitKind::Message { .. } => Some("消息".to_string()),
            HitKind::File => Some("文件".to_string()),
            HitKind::Meta => Some("分支 / 仓库".to_string()),
            HitKind::Title => None,
        };
        let row = h_flex()
            .id(("agent-search-row", hit))
            .w_full()
            .px_3()
            .py_2()
            .gap_3()
            .items_start()
            .rounded(theme::RADIUS_LARGE)
            .cursor_pointer()
            .map(|row| {
                if selected {
                    row.bg(colors.selected).text_color(colors.selected_fg)
                } else {
                    row.hover(|row| row.bg(colors.hover))
                }
            })
            .child(glyph_tile(
                glyph_for(&self.agent.presets, &s.agent_id),
                theme::AGENT_GLYPH,
                colors,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(marked(&s.title, &title_marks, colors)),
                            )
                            .when(status != RowStatus::None, |row| {
                                row.child(status_mark(status, self.agent.spin, colors))
                                    .when_some(status.label(), |row, label| {
                                        row.child(
                                            div()
                                                .text_size(theme::TEXT_SECTION)
                                                .text_color(match status {
                                                    RowStatus::Awaiting if !selected => {
                                                        colors.attention
                                                    }
                                                    _ => muted,
                                                })
                                                .child(label),
                                        )
                                    })
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(theme::TEXT_SECTION)
                                    .text_color(muted)
                                    .child(agent_model::relative_time(s.updated_at, now, offset)),
                            ),
                    )
                    .when_some(snippet, |col, (text, ranges)| {
                        col.child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(theme::TEXT_SECTION)
                                .text_color(muted)
                                .child(marked(&text, &ranges, colors)),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(theme::TEXT_SECTION)
                            .text_color(muted)
                            .child(agent_name(&self.agent.presets, &s.agent_id))
                            .child("·")
                            .child(Icon::new(IconName::GitBranch).size(theme::SMALL_ICON_SIZE))
                            .child(workspace_label(&s.workspace_root, cx))
                            .when_some(s.branch.clone(), |m, branch| m.child("·").child(branch))
                            .children(kind_tag.map(tag))
                            .when(s.archived, |m| m.child(tag("已归档".into()))),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.agent_search_open(Some(hit), window, cx)
            }));
        div().w_full().px_1().pb_1().child(row).into_any_element()
    }
}
