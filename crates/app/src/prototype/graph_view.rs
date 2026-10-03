//! Git Graph（编辑区页签）：VS Code Git Graph 扩展的核心 —— 提交图带分支车道、引用标签、
//! 作者、相对时间和短哈希；分支筛选、分页“加载更多”；点提交看详情和改动文件，点文件看
//! Diff。车道布局是纯函数 `layout`：每条车道记住它期待的下一个哈希，提交落到第一个期待
//! 它的车道；第一父提交继承车道，其余父提交占用第一条空闲车道。绘制只画本行与相邻行的
//! 连接，列表虚拟化后任意长的历史都按可见行绘制。

use super::{DiffSource, DiffTab, Pane, Prototype, SINGLE_LINE};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use workspace_editor_core::RepoId;
use workspace_editor_git::{
    Branch, CommitDetails, CommitFile, GraphCommit, GraphRef, GraphScope, Operation, RefKind,
    Request,
};

/// Page size, also the step of 加载更多.
const PAGE: usize = 500;
/// A commit's changed files beyond this count are folded into a note.
const FILES_MAX: usize = 500;

/// One row's lane layout, parallel to `GitGraph::commits`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GraphRow {
    /// The commit node's lane and its lane color.
    lane: usize,
    color: usize,
    /// (lane, color) verticals crossing the row untouched.
    pass: Vec<(usize, usize)>,
    /// A lane was already waiting for this commit: a line comes in from the top.
    enter: bool,
    /// The first parent stays on this lane: a line continues below the node.
    exit: bool,
    /// (lane, color) of extra parents the node curves down to.
    links: Vec<(usize, usize)>,
}

fn free_lane(waiting: &mut Vec<Option<(String, usize)>>) -> usize {
    match waiting.iter().position(Option::is_none) {
        Some(lane) => lane,
        None => {
            waiting.push(None);
            waiting.len() - 1
        }
    }
}

/// Assigns lanes to the loaded commits, newest first (`git log --date-order`). Freed lanes are
/// reused; a lane's color is fixed when the lane is created.
fn layout(commits: &[GraphCommit]) -> (Vec<GraphRow>, usize) {
    // The hash each lane waits for next, with the lane's color.
    let mut waiting: Vec<Option<(String, usize)>> = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::with_capacity(commits.len());
    let mut width = 1usize;
    for commit in commits {
        let found = waiting
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|(hash, _)| *hash == commit.hash));
        let (lane, color, enter) = match found {
            Some(lane) => {
                let color = waiting[lane].as_ref().map(|(_, color)| *color).unwrap_or(0);
                waiting[lane] = None;
                (lane, color, true)
            }
            None => {
                let lane = free_lane(&mut waiting);
                let color = next_color;
                next_color += 1;
                (lane, color, false)
            }
        };
        let mut exit = false;
        let mut links = Vec::new();
        for (n, parent) in commit.parents.iter().enumerate() {
            let expected = waiting
                .iter()
                .position(|slot| slot.as_ref().is_some_and(|(hash, _)| hash == parent));
            match (n, expected) {
                // The first parent inherits the lane, unless another lane already waits for it
                // (date order listed a sibling first): then this lane ends in a merge edge.
                (0, None) => {
                    waiting[lane] = Some((parent.clone(), color));
                    exit = true;
                }
                (_, Some(other)) => {
                    let color = waiting[other]
                        .as_ref()
                        .map(|(_, color)| *color)
                        .unwrap_or(0);
                    links.push((other, color));
                }
                (_, None) => {
                    let other = free_lane(&mut waiting);
                    let color = next_color;
                    next_color += 1;
                    waiting[other] = Some((parent.clone(), color));
                    links.push((other, color));
                }
            }
        }
        let pass = waiting
            .iter()
            .enumerate()
            .filter(|(other, slot)| {
                *other != lane && slot.is_some() && !links.iter().any(|(target, _)| target == other)
            })
            .map(|(other, slot)| (other, slot.as_ref().map(|(_, color)| *color).unwrap_or(0)))
            .collect();
        width = width.max(waiting.len()).max(lane + 1);
        rows.push(GraphRow {
            lane,
            color,
            pass,
            enter,
            exit,
            links,
        });
    }
    (rows, width)
}

/// The graph tab's state: one repository's paged history plus the details view.
pub(super) struct GitGraph {
    pub repo: workspace_editor_core::Repository,
    scope: GraphScope,
    /// The branch filter's choices, loaded with the first page.
    branches: Vec<Branch>,
    commits: Vec<GraphCommit>,
    rows: Vec<GraphRow>,
    /// Lanes the widest row needs (the graph column's width).
    lanes: usize,
    has_more: bool,
    loading: bool,
    error: Option<String>,
    /// The row whose details show; clicking it again closes the view.
    selected: Option<usize>,
    details: Option<Result<CommitDetails, String>>,
    details_loading: bool,
    list: UniformListScrollHandle,
    /// Page loads check this to discard stale results.
    generation: u64,
    cancel: Arc<AtomicBool>,
}

impl GitGraph {
    fn new(repo: workspace_editor_core::Repository) -> Self {
        Self {
            repo,
            scope: GraphScope::All,
            branches: Vec::new(),
            commits: Vec::new(),
            rows: Vec::new(),
            lanes: 1,
            has_more: false,
            loading: false,
            error: None,
            selected: None,
            details: None,
            details_loading: false,
            list: UniformListScrollHandle::new(),
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn scope_label(&self) -> String {
        match &self.scope {
            GraphScope::All => "全部分支".to_string(),
            GraphScope::Local => "本地分支".to_string(),
            GraphScope::Branch(reference) => reference
                .strip_prefix("refs/heads/")
                .or_else(|| reference.strip_prefix("refs/remotes/"))
                .unwrap_or(reference)
                .to_string(),
        }
    }
}

impl Prototype {
    /// The SCM repository header's graph button: one graph tab per window, focused if open.
    pub(super) fn open_git_graph(&mut self, g: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let repo = group.repo.clone();
        if self
            .graph
            .as_ref()
            .is_some_and(|graph| graph.repo.id == repo.id)
        {
            self.select_pane(Pane::Graph, window, cx);
            return;
        }
        self.graph = Some(GitGraph::new(repo));
        self.select_pane(Pane::Graph, window, cx);
        self.graph_load(true, window, cx);
    }

    pub(super) fn close_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(graph) = self.graph.take() {
            graph.cancel.store(true, Ordering::Relaxed);
        }
        if self.active == Pane::Graph {
            self.active = self
                .preview_diff
                .as_ref()
                .map(|_| Pane::Diff)
                .or_else(|| self.documents.last().map(|doc| Pane::Document(doc.id)))
                .unwrap_or(Pane::Welcome);
            self.focus_active_editor(window, cx);
        }
        self.update_welcome_blink(window, cx);
        cx.notify();
    }

    /// (Re)loads the graph. `reset` starts from the first page and refreshes the branch
    /// filter; otherwise the next page is appended.
    fn graph_load(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(graph) = &mut self.graph else {
            return;
        };
        if graph.loading {
            return;
        }
        let repo = graph.repo.clone();
        let scope = graph.scope.clone();
        let skip = if reset { 0 } else { graph.commits.len() };
        graph.cancel.store(true, Ordering::Relaxed);
        graph.cancel = Arc::new(AtomicBool::new(false));
        let cancel = graph.cancel.clone();
        graph.generation += 1;
        let generation = graph.generation;
        graph.loading = true;
        graph.error = None;
        if reset {
            graph.commits.clear();
            graph.rows.clear();
            graph.selected = None;
            graph.details = None;
            graph.list = UniformListScrollHandle::new();
        }
        cx.notify();
        let service = self.service.clone();
        let job = cx.background_spawn(async move {
            let branches = if reset {
                service.branches(&repo, &cancel).ok()
            } else {
                None
            };
            let page = service.graph(&repo, &scope, skip, PAGE, &cancel);
            (branches, page)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (branches, page) = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(graph) = &mut this.graph else {
                    return;
                };
                if graph.generation != generation {
                    return;
                }
                graph.loading = false;
                match page {
                    Ok(page) => {
                        if let Some(branches) = branches {
                            graph.branches = branches;
                        }
                        graph.has_more = page.len() == PAGE;
                        graph.commits.extend(page);
                        let (rows, lanes) = layout(&graph.commits);
                        graph.rows = rows;
                        graph.lanes = lanes;
                    }
                    Err(error) => {
                        graph.error = Some(error.to_string());
                        graph.has_more = false;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A write (checkout, pull, commit …) touched the graph's repository: reload it.
    pub(super) fn graph_reload(
        &mut self,
        id: &RepoId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .graph
            .as_ref()
            .is_some_and(|graph| graph.repo.id == *id)
        {
            self.graph_load(true, window, cx);
        }
    }

    fn graph_set_scope(&mut self, scope: GraphScope, window: &mut Window, cx: &mut Context<Self>) {
        let changed = self
            .graph
            .as_ref()
            .is_some_and(|graph| graph.scope != scope);
        if changed {
            if let Some(graph) = &mut self.graph {
                graph.scope = scope;
            }
            self.graph_load(true, window, cx);
        }
    }

    fn graph_select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(graph) = &mut self.graph else {
            return;
        };
        if graph.selected == Some(index) {
            graph.selected = None;
            graph.details = None;
            cx.notify();
            return;
        }
        let Some(commit) = graph.commits.get(index) else {
            return;
        };
        let hash = commit.hash.clone();
        graph.selected = Some(index);
        graph.details = None;
        graph.details_loading = true;
        cx.notify();
        let repo = graph.repo.clone();
        let service = self.service.clone();
        let wanted = hash.clone();
        let job = cx.background_spawn(async move {
            service.commit_details(&repo, &wanted, &AtomicBool::new(false))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(graph) = &mut this.graph else {
                    return;
                };
                graph.details_loading = false;
                // The selection may have moved on while the details loaded.
                let still_selected = graph
                    .selected
                    .and_then(|i| graph.commits.get(i))
                    .is_some_and(|commit| commit.hash == hash);
                if still_selected {
                    graph.details = Some(result.map_err(|error| error.to_string()));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A changed file in the details view opens the commit's diff for it, like clicking a
    /// file in VS Code Git Graph's details.
    fn graph_open_diff(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(graph) = &self.graph else {
            return;
        };
        let Some(Ok(details)) = graph.details.as_ref() else {
            return;
        };
        let Some(file) = details.files.get(index) else {
            return;
        };
        let short: String = details.hash.chars().take(7).collect();
        let name = file
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let request = Request {
            repo: graph.repo.clone(),
            generation: self.generation,
            operation: Operation::CommitDiff {
                commit: details.hash.clone(),
                parent: details.parents.first().cloned(),
                path: file.path.clone(),
                original_path: file.original_path.clone(),
            },
        };
        let tab = DiffTab {
            label: format!("{name}（{short}）"),
            tooltip: format!(
                "{} · 提交 {short}",
                graph.repo.worktree.join(&file.path).display()
            ),
            path: graph.repo.worktree.join(&file.path),
            source: DiffSource::Git(request),
        };
        self.show_diff_tab(tab, window, cx);
    }

    fn graph_header(&self, graph: &GitGraph, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let name = graph
            .repo
            .worktree
            .file_name()
            .unwrap_or(graph.repo.worktree.as_os_str())
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let scope = graph.scope_label();
        let weak = cx.weak_entity();
        let filter = Button::new("graph-scope")
            .xsmall()
            .ghost()
            .icon(IconName::GitBranch)
            .label(scope)
            .dropdown_menu(move |menu, _, cx| {
                let Some(this) = weak.upgrade() else {
                    return menu;
                };
                let read = this.read(cx);
                let Some(graph) = read.graph.as_ref() else {
                    return menu;
                };
                let mut menu = menu
                    .item(scope_item("全部分支", GraphScope::All, &graph.scope, &this))
                    .item(scope_item(
                        "本地分支",
                        GraphScope::Local,
                        &graph.scope,
                        &this,
                    ))
                    .separator();
                for branch in &graph.branches {
                    let reference = if branch.remote {
                        format!("refs/remotes/{}", branch.name)
                    } else {
                        format!("refs/heads/{}", branch.name)
                    };
                    menu = menu.item(scope_item(
                        &branch.name,
                        GraphScope::Branch(reference),
                        &graph.scope,
                        &this,
                    ));
                }
                menu
            });
        h_flex()
            .h(theme::SECTION_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION)
            .child(
                Icon::new(IconName::GitGraph)
                    .size(theme::ICON_SIZE)
                    .text_color(colors.muted),
            )
            .child(div().font_weight(FontWeight::MEDIUM).child(name))
            .child(filter)
            .child(
                div()
                    .flex_1()
                    .text_color(colors.muted)
                    .child(format!("{} 个提交", graph.commits.len())),
            )
            .child(
                Button::new("graph-refresh")
                    .xsmall()
                    .ghost()
                    .icon(IconName::RotateCw)
                    .tooltip("刷新")
                    .accessibility_label("刷新")
                    .disabled(graph.loading)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.graph_load(true, window, cx);
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_graph(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(graph) = &self.graph else {
            return div().into_any_element();
        };
        let rows = graph.commits.len()
            + usize::from(
                graph.loading
                    || graph.error.is_some()
                    || graph.commits.is_empty()
                    || graph.has_more,
            );
        let list = uniform_list(
            "git-graph-rows",
            rows,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range.map(|index| this.graph_row(index, cx)).collect()
            }),
        )
        .size_full()
        .track_scroll(&graph.list);
        v_flex()
            .size_full()
            .bg(colors.editor)
            .child(self.graph_header(graph, cx))
            .child(div().flex_1().min_h_0().child(list))
            .children(
                graph
                    .selected
                    .is_some()
                    .then(|| self.graph_details(graph, cx)),
            )
            .into_any_element()
    }

    fn graph_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(graph) = &self.graph else {
            return div().into_any_element();
        };
        if index >= graph.commits.len() {
            return self.graph_status_row(graph, colors, cx);
        }
        let commit = &graph.commits[index];
        let row = &graph.rows[index];
        let selected = graph.selected == Some(index);
        let subject = commit.subject.replace(SINGLE_LINE, "⏎");
        let short: String = commit.hash.chars().take(7).collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let ms = commit.time.saturating_mul(1000);
        let time = crate::agent_model::relative_time(ms, now, crate::agent_model::local_offset(ms));
        let tooltip = format!(
            "{}\n{} · {} · {}",
            commit.subject, commit.author, commit.hash, time
        );
        h_flex()
            .id(("graph-row", index))
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pr_3()
            .gap_2()
            .cursor_pointer()
            .text_size(theme::TEXT_CAPTION)
            .when(selected, |row| row.bg(colors.selected))
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .child(self.graph_lanes(
                row,
                commit.refs.iter().any(|reference| reference.head),
                graph.lanes,
                colors,
            ))
            .children(
                commit
                    .refs
                    .iter()
                    .map(|reference| ref_chip(reference, colors)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if selected {
                        colors.selected_fg
                    } else {
                        colors.foreground
                    })
                    .child(subject.clone()),
            )
            .child(
                div()
                    .w(theme::GRAPH_AUTHOR)
                    .flex_shrink_0()
                    .truncate()
                    .text_color(colors.muted)
                    .child(commit.author.clone()),
            )
            .child(div().flex_shrink_0().text_color(colors.muted).child(short))
            .child(
                div()
                    .w(theme::GRAPH_TIME)
                    .flex_shrink_0()
                    .text_right()
                    .text_color(colors.muted)
                    .child(time),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .on_click(cx.listener(move |this, _, window, cx| this.graph_select(index, window, cx)))
            .into_any_element()
    }

    /// The trailing row: loading, error, empty state or 加载更多.
    fn graph_status_row(
        &self,
        graph: &GitGraph,
        colors: theme::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base = || {
            h_flex()
                .w_full()
                .h(theme::ROW_HEIGHT)
                .px_3()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
        };
        if graph.loading {
            return base().child("正在加载…").into_any_element();
        }
        if let Some(error) = &graph.error {
            return base()
                .text_color(colors.deleted)
                .child(format!("错误：{error}"))
                .into_any_element();
        }
        if graph.commits.is_empty() {
            return base().child("该范围没有提交").into_any_element();
        }
        base()
            .id("graph-load-more")
            .cursor_pointer()
            .hover(|row| row.bg(colors.hover).text_color(colors.foreground))
            .child(format!(
                "加载更多…（已加载 {} 个提交）",
                graph.commits.len()
            ))
            .on_click(cx.listener(|this, _, window, cx| this.graph_load(false, window, cx)))
            .into_any_element()
    }

    /// The lane painting of one row: verticals through it, the node's connections, the node.
    /// HEAD's commit gets a ring around its node, as in VS Code Git Graph.
    fn graph_lanes(
        &self,
        row: &GraphRow,
        head: bool,
        lanes: usize,
        colors: theme::Colors,
    ) -> AnyElement {
        let row = row.clone();
        let lanes_canvas = canvas(
            move |_, _, _| row,
            move |frame, row, window, _| {
                let height = frame.size.height;
                let lane_x = |lane: usize| {
                    frame.origin.x + theme::GRAPH_LANE * lane as f32 + theme::GRAPH_LANE / 2.
                };
                let top = frame.origin.y;
                let middle = top + height / 2.;
                let bottom = top + height;
                let mut lines: Vec<(usize, PathBuilder)> = Vec::new();
                for (lane, color) in &row.pass {
                    let x = lane_x(*lane);
                    let builder = graph_line(&mut lines, *color);
                    builder.move_to(point(x, top));
                    builder.line_to(point(x, bottom));
                }
                let node_x = lane_x(row.lane);
                if row.enter {
                    let builder = graph_line(&mut lines, row.color);
                    builder.move_to(point(node_x, top));
                    builder.line_to(point(node_x, middle));
                }
                if row.exit {
                    let builder = graph_line(&mut lines, row.color);
                    builder.move_to(point(node_x, middle));
                    builder.line_to(point(node_x, bottom));
                }
                for (target, color) in &row.links {
                    let to = point(lane_x(*target), bottom);
                    let builder = graph_line(&mut lines, *color);
                    builder.move_to(point(node_x, middle));
                    builder.curve_to(to, point(to.x, middle));
                }
                for (color, builder) in lines {
                    if let Ok(path) = builder.build() {
                        window
                            .paint_path(path, colors.graph_lanes[color % colors.graph_lanes.len()]);
                    }
                }
                let radius = theme::GRAPH_NODE / 2.;
                let lane_color = colors.graph_lanes[row.color % colors.graph_lanes.len()];
                if head {
                    // A ring: lane-colored disc, background gap, then the node.
                    let ring = radius + theme::GRAPH_STROKE * 2.;
                    window.paint_quad(
                        fill(
                            bounds(
                                point(node_x - ring, middle - ring),
                                size(ring * 2., ring * 2.),
                            ),
                            lane_color,
                        )
                        .corner_radii(Corners::all(ring)),
                    );
                    let gap = radius + theme::GRAPH_STROKE;
                    window.paint_quad(
                        fill(
                            bounds(point(node_x - gap, middle - gap), size(gap * 2., gap * 2.)),
                            colors.editor,
                        )
                        .corner_radii(Corners::all(gap)),
                    );
                }
                window.paint_quad(
                    fill(
                        bounds(
                            point(node_x - radius, middle - radius),
                            size(theme::GRAPH_NODE, theme::GRAPH_NODE),
                        ),
                        lane_color,
                    )
                    .corner_radii(Corners::all(radius)),
                );
            },
        );
        div()
            .w(theme::GRAPH_LANE * lanes.max(1) as f32)
            .flex_shrink_0()
            .h_full()
            .child(lanes_canvas.size_full())
            .into_any_element()
    }

    /// The details view under the graph, as in VS Code Git Graph: ids and dates, the full
    /// message and the changed files.
    fn graph_details(&self, graph: &GitGraph, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(index) = graph.selected else {
            return div().into_any_element();
        };
        let Some(commit) = graph.commits.get(index) else {
            return div().into_any_element();
        };
        let short: String = commit.hash.chars().take(7).collect();
        let full = commit.hash.clone();
        let body: AnyElement = if graph.details_loading {
            div()
                .p_3()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child("正在加载提交详情…")
                .into_any_element()
        } else {
            match &graph.details {
                Some(Ok(details)) => self.graph_details_body(details, colors, cx),
                Some(Err(error)) => div()
                    .p_3()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.deleted)
                    .child(format!("错误：{error}"))
                    .into_any_element(),
                None => div().into_any_element(),
            }
        };
        let repo = graph.repo.id.clone();
        let copy_hash = full.clone();
        v_flex()
            .h(theme::GRAPH_DETAILS_MAX)
            .flex_shrink_0()
            .w_full()
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.panel)
            .child(
                h_flex()
                    .h(theme::SECTION_HEIGHT)
                    .w_full()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_CAPTION)
                    .child(
                        Icon::new(IconName::GitCommitHorizontal)
                            .size(theme::ICON_SIZE)
                            .text_color(colors.muted),
                    )
                    .child(div().font_weight(FontWeight::MEDIUM).child(short))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(commit.subject.replace(SINGLE_LINE, "⏎")),
                    )
                    .child(
                        Button::new("graph-branch-at")
                            .xsmall()
                            .ghost()
                            .icon(IconName::GitBranch)
                            .label("创建分支…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let Some(g) =
                                    this.groups.iter().position(|group| group.repo.id == repo)
                                else {
                                    return;
                                };
                                this.open_create_branch_at(g, Some(full.clone()), window, cx);
                            })),
                    )
                    .child(
                        Button::new("graph-copy-hash")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Copy)
                            .tooltip("复制提交哈希")
                            .accessibility_label("复制提交哈希")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_hash.clone()));
                                this.message = "已复制提交哈希".into();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("graph-details-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .tooltip("关闭")
                            .accessibility_label("关闭")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(graph) = &mut this.graph {
                                    graph.selected = None;
                                    graph.details = None;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    fn graph_details_body(
        &self,
        details: &CommitDetails,
        colors: theme::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let at = details.author_time.saturating_mul(1000);
        let authored = format!(
            "{} <{}> · {}",
            details.author,
            details.email,
            crate::agent_model::relative_time(at, now, crate::agent_model::local_offset(at))
        );
        let committed = (details.committer != details.author
            || details.committer_email != details.email)
            .then(|| {
                let ct = details.commit_time.saturating_mul(1000);
                format!(
                    "提交者 {} <{}> · {}",
                    details.committer,
                    details.committer_email,
                    crate::agent_model::relative_time(
                        ct,
                        now,
                        crate::agent_model::local_offset(ct)
                    )
                )
            });
        // The message repeats the subject; show the body only.
        let body_message = details
            .message
            .split_once('\n')
            .map(|(_, rest)| rest.trim())
            .filter(|rest| !rest.is_empty());
        let files = details.files.len();
        let shown = details.files.iter().take(FILES_MAX).count();
        v_flex()
            .id("graph-details-body")
            .size_full()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .text_size(theme::TEXT_CAPTION)
            .child(div().text_color(colors.muted).child(authored))
            .children(committed.map(|line| div().text_color(colors.muted).child(line)))
            .children(body_message.map(|message| {
                div()
                    .w_full()
                    .rounded(theme::RADIUS)
                    .bg(colors.keycap)
                    .p_2()
                    .text_color(colors.foreground)
                    .child(message.to_string())
            }))
            .child(
                div()
                    .pt_1()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("更改的文件（{files}）")),
            )
            .children(
                details
                    .files
                    .iter()
                    .enumerate()
                    .take(FILES_MAX)
                    .map(|(index, file)| self.graph_file_row(index, file, colors, cx)),
            )
            .when(files > shown, |list| {
                list.child(
                    div()
                        .text_color(colors.muted)
                        .child(format!("… 还有 {} 个文件未显示", files - shown)),
                )
            })
            .into_any_element()
    }

    fn graph_file_row(
        &self,
        index: usize,
        file: &CommitFile,
        colors: theme::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status_color = match file.status {
            'A' => colors.added,
            'D' => colors.deleted,
            'M' | 'T' => colors.modified,
            _ => colors.muted,
        };
        let path = file.path.display().to_string().replace(SINGLE_LINE, "⏎");
        let tooltip = file.original_path.as_ref().map_or_else(
            || file.path.display().to_string(),
            |original| format!("{} ← {}", file.path.display(), original.display()),
        );
        h_flex()
            .id(("graph-file", index))
            .w_full()
            .h(theme::ROW_HEIGHT)
            .gap_1()
            .cursor_pointer()
            .rounded(theme::RADIUS)
            .hover(|row| row.bg(colors.hover))
            .child(
                div()
                    .w(theme::BADGE_SIZE)
                    .flex_shrink_0()
                    .text_right()
                    .text_color(status_color)
                    .font_weight(FontWeight::MEDIUM)
                    .child(file.status.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(colors.foreground)
                    .child(path),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.graph_open_diff(index, window, cx)),
            )
            .into_any_element()
    }
}

/// A branch-filter menu item; the current scope is marked.
fn scope_item(
    label: &str,
    scope: GraphScope,
    current: &GraphScope,
    view: &gpui_kit::Entity<Prototype>,
) -> PopupMenuItem {
    let selected = *current == scope;
    let view = view.clone();
    PopupMenuItem::new(label.to_string())
        .checked(selected)
        .on_click(move |_, window, cx| {
            let scope = scope.clone();
            view.update(cx, |this, cx| this.graph_set_scope(scope, window, cx));
        })
}

/// All segments of one lane color share a path, so a row paints each lane color once.
fn graph_line(lines: &mut Vec<(usize, PathBuilder)>, color: usize) -> &mut PathBuilder {
    let index = match lines.iter().position(|(c, _)| *c == color) {
        Some(index) => index,
        None => {
            lines.push((color, PathBuilder::stroke(theme::GRAPH_STROKE)));
            lines.len() - 1
        }
    };
    &mut lines[index].1
}

/// A branch / remote / tag / HEAD chip in front of the subject, as in VS Code Git Graph.
fn ref_chip(reference: &GraphRef, colors: theme::Colors) -> AnyElement {
    let (icon, label) = match reference.kind {
        RefKind::Head => (None, "HEAD".to_string()),
        RefKind::Branch => (Some(IconName::GitBranch), reference.name.clone()),
        RefKind::Remote => (Some(IconName::Cloud), reference.name.clone()),
        RefKind::Tag => (Some(IconName::Tag), reference.name.clone()),
    };
    h_flex()
        .flex_shrink_0()
        .h(theme::BADGE_SIZE)
        .px_1()
        .mr_1()
        .gap_0p5()
        .items_center()
        .rounded(theme::RADIUS)
        .text_size(theme::TEXT_BADGE)
        .map(|chip| {
            if reference.head {
                chip.bg(colors.accent).text_color(colors.badge_fg)
            } else {
                chip.bg(colors.keycap).text_color(colors.foreground)
            }
        })
        .children(icon.map(|icon| Icon::new(icon).size(theme::AGENT_GLYPH_ICON)))
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // `gpui_kit::*` also exports a `test` macro; `#[test]` must be the built-in one.
    #[allow(unused_imports)]
    use core::prelude::v1::test;

    fn commit(hash: &str, parents: &[&str]) -> GraphCommit {
        GraphCommit {
            hash: hash.into(),
            parents: parents.iter().map(|parent| parent.to_string()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            refs: Vec::new(),
            subject: String::new(),
        }
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        let (rows, lanes) = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(lanes, 1);
        assert!(rows.iter().all(|row| row.lane == 0));
        assert!(!rows[0].enter && rows[0].exit);
        assert!(rows[1].enter && rows[1].exit);
        assert!(rows[2].enter && !rows[2].exit);
        assert!(rows.iter().all(|row| row.links.is_empty()));
    }

    #[test]
    fn a_merged_branch_gets_its_own_lane() {
        // m merges f into main; main's tip b shares the base e.
        let (rows, lanes) = layout(&[
            commit("m", &["b", "f"]),
            commit("f", &["e"]),
            commit("b", &["e"]),
            commit("e", &[]),
        ]);
        assert_eq!(lanes, 2);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].links.len(), 1);
        assert_eq!(rows[0].links[0].0, 1);
        assert!(rows[0].exit);
        assert_eq!(rows[1].lane, 1);
        assert!(rows[1].enter && rows[1].exit);
        // b's parent e is already waited for by the branch lane: the edge merges into it.
        assert_eq!(rows[2].lane, 0);
        assert!(rows[2].enter && !rows[2].exit);
        assert_eq!(rows[2].links.len(), 1);
        assert_eq!(rows[2].links[0].0, 1);
        assert_eq!(rows[3].lane, 1);
        assert!(rows[3].enter && !rows[3].exit);
    }

    #[test]
    fn siblings_waiting_for_one_parent_share_a_lane() {
        // x and y both point at root; y's edge joins the lane that already waits for root.
        let (rows, lanes) = layout(&[
            commit("x", &["root"]),
            commit("y", &["root"]),
            commit("root", &[]),
        ]);
        assert_eq!(lanes, 2);
        assert_eq!(rows[0].lane, 0);
        assert!(rows[0].exit);
        assert_eq!(rows[1].lane, 1);
        assert!(!rows[1].enter && !rows[1].exit);
        assert_eq!(rows[1].links.len(), 1);
        assert_eq!(rows[1].links[0].0, 0);
        assert_eq!(rows[2].lane, 0);
        assert!(rows[2].enter && !rows[2].exit);
    }

    #[test]
    fn lanes_are_reused_after_they_free() {
        // Two independent roots: the second chain reuses the freed first lane.
        let (rows, lanes) = layout(&[
            commit("b", &["a"]),
            commit("a", &[]),
            commit("d", &["c"]),
            commit("c", &[]),
        ]);
        assert_eq!(lanes, 1);
        assert!(rows.iter().all(|row| row.lane == 0));
    }
}

#[cfg(test)]
#[path = "graph_ui_tests.rs"]
mod ui_tests;
