//! Git Graph（编辑区页签）：VS Code Git Graph 扩展的核心 —— 提交图带分支车道、引用标签、
//! 作者、相对时间和短哈希；分支筛选、分页“加载更多”；点提交看详情和改动文件，点文件看
//! Diff。车道布局是纯函数 `layout`，移植自 Git Graph 的 `web/graph.ts`：沿第一父提交向下
//! 走成一条分支，每一行取第一条空闲车道，所以车道空出后右边的线会左移；到已放置父提交的
//! 线一直走到父提交才汇合；颜色在分支结束后复用。每行只画上一行下来的下半段和本行出去的
//! 上半段，列表虚拟化后任意长的历史都按可见行绘制。

use super::{DiffSource, DiffTab, Pane, Prototype, SINGLE_LINE};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
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
    Request, WriteOperation,
};

/// Page size, also the step of 加载更多.
const PAGE: usize = 500;
/// A commit's changed files beyond this count are folded into a note.
const FILES_MAX: usize = 500;

/// One row's lane layout, parallel to `GitGraph::commits`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GraphRow {
    /// The commit node's lane and its color.
    lane: usize,
    color: usize,
    /// (from lane, to lane, color) of each line from this row's node height down to the next
    /// row's; the next row draws their lower halves.
    down: Vec<(usize, usize, usize)>,
    /// Rows of the loaded children, for the node tooltip's "包含它的分支".
    children: Vec<usize>,
}

/// A parent outside the loaded commits: its line runs to the bottom of the list.
const UNLOADED: usize = usize::MAX;

#[derive(Default)]
struct Vertex {
    lane: usize,
    branch: Option<usize>,
    parents: Vec<usize>,
    next_parent: usize,
    /// The first lane still free at this row; `taken[lane]` says which line holds a lane
    /// below it: (the vertex the line heads for, the line's branch).
    next_lane: usize,
    taken: Vec<(Option<usize>, usize)>,
}

impl Vertex {
    fn next_parent(&self) -> Option<usize> {
        self.parents.get(self.next_parent).copied()
    }

    fn take(&mut self, lane: usize, toward: Option<usize>, branch: usize) {
        if lane == self.next_lane {
            self.next_lane += 1;
            self.taken.push((toward, branch));
        }
    }

    fn lane_toward(&self, vertex: usize, branch: usize) -> Option<usize> {
        self.taken
            .iter()
            .position(|&taken| taken == (Some(vertex), branch))
    }
}

/// VS Code Git Graph's layout (`web/graph.ts`): a branch follows first parents down from a
/// vertex; at every row a line takes the first free lane, so lines shift left as lanes free
/// up; a line to a parent that is already placed runs down to it. Merge edges into a placed
/// branch join that branch's line where it passes. A color is reused once its branch ended.
struct Layout {
    vertices: Vec<Vertex>,
    branch_colors: Vec<usize>,
    /// The row where the last branch of each color ended.
    color_ends: Vec<usize>,
    /// (row, from lane, to lane, color).
    lines: Vec<(usize, usize, usize, usize)>,
}

impl Layout {
    fn color(&mut self, start: usize) -> usize {
        match self.color_ends.iter().position(|&end| start > end) {
            Some(color) => color,
            None => {
                self.color_ends.push(0);
                self.color_ends.len() - 1
            }
        }
    }

    fn path(&mut self, start: usize) {
        let rows = self.vertices.len();
        let mut parent = self.vertices[start].next_parent();
        let mut last = match self.vertices[start].branch {
            Some(_) => self.vertices[start].lane,
            None => self.vertices[start].next_lane,
        };
        let merge_into = parent.filter(|&p| {
            p != UNLOADED
                && self.vertices[start].parents.len() > 1
                && self.vertices[start].branch.is_some()
                && self.vertices[p].branch.is_some()
        });
        if let Some(target) = merge_into {
            let branch = self.vertices[target].branch.unwrap_or(0);
            let color = self.branch_colors[branch];
            for row in start + 1..rows {
                let joined = self.vertices[row].lane_toward(target, branch);
                let lane = joined.unwrap_or(self.vertices[row].next_lane);
                self.lines.push((row - 1, last, lane, color));
                self.vertices[row].take(lane, Some(target), branch);
                last = lane;
                if joined.is_some() {
                    break;
                }
            }
            self.vertices[start].next_parent += 1;
            return;
        }
        let color = self.color(start);
        let branch = self.branch_colors.len();
        self.branch_colors.push(color);
        let vertex = &mut self.vertices[start];
        if vertex.branch.is_none() {
            vertex.branch = Some(branch);
            vertex.lane = last;
        }
        vertex.take(last, Some(start), branch);
        let mut current = start;
        let mut row = start + 1;
        // A root commit has no line below it.
        if parent.is_some() {
            while row < rows {
                let reached = Some(row) == parent;
                let placed = self.vertices[row].branch.is_some();
                let lane = if reached && placed {
                    self.vertices[row].lane
                } else {
                    self.vertices[row].next_lane
                };
                self.lines.push((row - 1, last, lane, color));
                self.vertices[row].take(lane, parent, branch);
                last = lane;
                if reached {
                    self.vertices[current].next_parent += 1;
                    let vertex = &mut self.vertices[row];
                    if vertex.branch.is_none() {
                        vertex.branch = Some(branch);
                        vertex.lane = lane;
                    }
                    current = row;
                    parent = self.vertices[row].next_parent();
                    if parent.is_none() || placed {
                        break;
                    }
                }
                row += 1;
            }
        }
        if row >= rows && parent.is_some() {
            // The parent is not loaded: the line has run to the last row.
            self.vertices[current].next_parent += 1;
        }
        self.color_ends[color] = row;
    }
}

/// Assigns lanes to the loaded commits, newest first (`git log --date-order`, so parents come
/// after their children). Returns the rows and the widest row's lane count.
fn layout(commits: &[GraphCommit]) -> (Vec<GraphRow>, usize) {
    let index: std::collections::HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(row, commit)| (commit.hash.as_str(), row))
        .collect();
    let mut children = vec![Vec::new(); commits.len()];
    let mut vertices: Vec<Vertex> = Vec::with_capacity(commits.len());
    for (row, commit) in commits.iter().enumerate() {
        let parents = commit
            .parents
            .iter()
            .map(|parent| match index.get(parent.as_str()) {
                Some(&parent) => {
                    children[parent].push(row);
                    parent
                }
                None => UNLOADED,
            })
            .collect();
        vertices.push(Vertex {
            parents,
            ..Vertex::default()
        });
    }
    let mut layout = Layout {
        vertices,
        branch_colors: Vec::new(),
        color_ends: Vec::new(),
        lines: Vec::new(),
    };
    let mut row = 0;
    while row < commits.len() {
        let vertex = &layout.vertices[row];
        if vertex.next_parent < vertex.parents.len() || vertex.branch.is_none() {
            layout.path(row);
        } else {
            row += 1;
        }
    }
    let mut rows: Vec<GraphRow> = layout
        .vertices
        .iter()
        .zip(children)
        .map(|(vertex, children)| GraphRow {
            lane: vertex.lane,
            color: layout.branch_colors[vertex.branch.unwrap_or(0)],
            down: Vec::new(),
            children,
        })
        .collect();
    for (row, from, to, color) in layout.lines {
        rows[row].down.push((from, to, color));
    }
    let width = layout
        .vertices
        .iter()
        .map(|vertex| vertex.next_lane.max(vertex.lane + 1))
        .max()
        .unwrap_or(1);
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
                .diff
                .tab
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
        // Another page can wait; a reset (new scope, a write) replaces whatever is loading.
        if graph.loading && !reset {
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
                // The selection may have moved on while the details loaded; an older answer
                // neither shows nor ends the newer request's loading state.
                let still_selected = graph
                    .selected
                    .and_then(|i| graph.commits.get(i))
                    .is_some_and(|commit| commit.hash == hash);
                if still_selected {
                    graph.details_loading = false;
                    graph.details = Some(result.map_err(|error| error.to_string()));
                } else if graph.selected.is_none() {
                    graph.details_loading = false;
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
        let selected = graph.selected == Some(index);
        let subject = commit.subject.replace(SINGLE_LINE, "⏎");
        let short: String = commit.hash.chars().take(7).collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        let ms = commit.time.saturating_mul(1000);
        let time = crate::agent_model::relative_time(ms, now, crate::agent_model::local_offset(ms));
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
            .child(self.graph_lanes(index, graph, colors, cx))
            .children({
                let lane = colors.graph_lanes[graph.rows[index].color % colors.graph_lanes.len()];
                commit
                    .refs
                    .iter()
                    .map(move |reference| ref_chip(reference, lane, colors))
            })
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
            .on_click(cx.listener(move |this, _, window, cx| this.graph_select(index, window, cx)))
            .context_menu({
                let weak = cx.weak_entity();
                move |menu, _, cx| match weak.upgrade() {
                    Some(view) => view.read(cx).graph_menu(index, menu, view.clone()),
                    None => menu,
                }
            })
            .into_any_element()
    }

    /// VS Code Git Graph's commit context menu, with the commit's tags' actions after it.
    fn graph_menu(&self, index: usize, menu: PopupMenu, view: Entity<Self>) -> PopupMenu {
        let Some(graph) = &self.graph else {
            return menu;
        };
        let Some(commit) = graph.commits.get(index) else {
            return menu;
        };
        let repo = graph.repo.id.clone();
        let hash = commit.hash.clone();
        type Action = Box<dyn Fn(&mut Prototype, &mut Window, &mut Context<Prototype>)>;
        let item = |label: String, action: Action| {
            let view = view.clone();
            PopupMenuItem::new(label).on_click(move |_, window, cx| {
                view.update(cx, |this, cx| action(this, window, cx));
            })
        };
        let mut menu = menu
            .item(item("添加标签…".into(), {
                let (repo, hash) = (repo.clone(), hash.clone());
                Box::new(move |this, window, cx| {
                    this.open_create_tag(&repo, hash.clone(), window, cx)
                })
            }))
            .item(item("创建分支…".into(), {
                let hash = hash.clone();
                Box::new(move |this, window, cx| this.graph_create_branch(hash.clone(), window, cx))
            }))
            .item(item(
                "复制提交哈希".into(),
                Box::new(move |this, _, cx| this.graph_copy(hash.clone(), "已复制提交哈希", cx)),
            ));
        for tag in commit
            .refs
            .iter()
            .filter(|reference| matches!(reference.kind, RefKind::Tag))
        {
            let name = tag.name.clone();
            menu = menu
                .separator()
                .item(item(format!("推送标签“{name}”"), {
                    let (repo, name) = (repo.clone(), name.clone());
                    Box::new(move |this, window, cx| this.graph_push_tag(&repo, &name, window, cx))
                }))
                .item(item(format!("删除标签“{name}”…"), {
                    let (repo, name) = (repo.clone(), name.clone());
                    Box::new(move |this, window, cx| {
                        this.scm_delete_tag(&repo, name.clone(), window, cx)
                    })
                }))
                .item(item(
                    format!("复制标签名“{name}”"),
                    Box::new(move |this, _, cx| this.graph_copy(name.clone(), "已复制标签名", cx)),
                ));
        }
        menu
    }

    fn graph_create_branch(&mut self, commit: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.graph.as_ref().map(|graph| graph.repo.id.clone()) else {
            return;
        };
        if let Some(g) = self.groups.iter().position(|group| group.repo.id == repo) {
            self.open_create_branch_at(g, Some(commit), window, cx);
        }
    }

    fn graph_push_tag(
        &mut self,
        repo: &RepoId,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let operation = WriteOperation::PushTag { name: name.into() };
        self.scm_request_for(repo, operation, window, cx);
    }

    fn graph_copy(&mut self, text: String, note: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.message = note.into();
        cx.notify();
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

    /// The lane painting of one row: the lower halves of the lines coming from the row above,
    /// the upper halves of the lines leaving this row, then the node. A lane change is Git
    /// Graph's S curve (a cubic from one row's node height to the next), split at the row
    /// boundary. HEAD's node is hollow, as in VS Code Git Graph; hovering the node tells which
    /// branches and tags contain the commit.
    fn graph_lanes(
        &self,
        index: usize,
        graph: &GitGraph,
        colors: theme::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = graph.rows[index].clone();
        let up = index
            .checked_sub(1)
            .and_then(|above| graph.rows.get(above))
            .map(|above| above.down.clone())
            .unwrap_or_default();
        let head = graph.commits[index].refs.iter().any(is_head);
        let lane = row.lane;
        let weak = cx.weak_entity();
        let lanes_canvas = canvas(
            move |_, _, _| (row, up),
            move |frame, (row, up), window, _| {
                let height = frame.size.height;
                let lane_x = |lane: usize| {
                    frame.origin.x + theme::GRAPH_LANE * lane as f32 + theme::GRAPH_LANE / 2.
                };
                let top = frame.origin.y;
                let middle = top + height / 2.;
                let bottom = top + height;
                let bend = height * 0.8;
                let mut lines: Vec<(usize, PathBuilder)> = Vec::new();
                for (from, to, color) in &up {
                    let (x1, x2) = (lane_x(*from), lane_x(*to));
                    let builder = graph_line(&mut lines, *color);
                    builder.move_to(point((x1 + x2) / 2., top));
                    if from == to {
                        builder.line_to(point(x2, middle));
                    } else {
                        let y1 = middle - height;
                        builder.cubic_bezier_to(
                            point(x2, middle),
                            point((x1 + x2 * 3.) / 4., y1 + (height * 3. - bend) / 4.),
                            point(x2, middle - bend / 2.),
                        );
                    }
                }
                for (from, to, color) in &row.down {
                    let (x1, x2) = (lane_x(*from), lane_x(*to));
                    let builder = graph_line(&mut lines, *color);
                    builder.move_to(point(x1, middle));
                    if from == to {
                        builder.line_to(point(x1, bottom));
                    } else {
                        builder.cubic_bezier_to(
                            point((x1 + x2) / 2., bottom),
                            point(x1, middle + bend / 2.),
                            point((x1 * 3. + x2) / 4., middle + (bend + height) / 4.),
                        );
                    }
                }
                for (color, builder) in lines {
                    if let Ok(path) = builder.build() {
                        window
                            .paint_path(path, colors.graph_lanes[color % colors.graph_lanes.len()]);
                    }
                }
                let radius = theme::GRAPH_NODE / 2.;
                let node_x = lane_x(row.lane);
                let lane_color = colors.graph_lanes[row.color % colors.graph_lanes.len()];
                if head {
                    // A stroked circle, so the row's hover / selection shows through it.
                    let ring = radius - theme::GRAPH_STROKE / 2.;
                    let mut circle = PathBuilder::stroke(theme::GRAPH_STROKE);
                    circle.move_to(point(node_x - ring, middle));
                    let radii = point(ring, ring);
                    circle.arc_to(
                        radii,
                        Pixels::ZERO,
                        false,
                        true,
                        point(node_x + ring, middle),
                    );
                    circle.arc_to(
                        radii,
                        Pixels::ZERO,
                        false,
                        true,
                        point(node_x - ring, middle),
                    );
                    if let Ok(path) = circle.build() {
                        window.paint_path(path, lane_color);
                    }
                } else {
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
                }
            },
        );
        div()
            .relative()
            .w(theme::GRAPH_LANE * graph.lanes.max(1) as f32)
            .flex_shrink_0()
            .h_full()
            .child(lanes_canvas.size_full())
            .child(
                div()
                    .id(("graph-node", index))
                    .absolute()
                    .top_0()
                    .left(theme::GRAPH_LANE * lane as f32)
                    .w(theme::GRAPH_LANE)
                    .h_full()
                    .tooltip(move |window, cx| {
                        let text = weak
                            .upgrade()
                            .and_then(|this| {
                                this.read(cx).graph.as_ref().map(|g| node_tooltip(g, index))
                            })
                            .unwrap_or_default();
                        gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
                    }),
            )
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
        let tag_at = full.clone();
        let tag_repo = repo.clone();
        let tags: Vec<String> = commit
            .refs
            .iter()
            .filter(|reference| matches!(reference.kind, RefKind::Tag))
            .map(|reference| reference.name.clone())
            .collect();
        let tags_row = (!tags.is_empty()).then(|| self.graph_tags_row(&repo, tags, colors, cx));
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
                        Button::new("graph-tag-at")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Tag)
                            .label("添加标签…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_create_tag(&tag_repo, tag_at.clone(), window, cx);
                            })),
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
            .children(tags_row)
            .child(div().flex_1().min_h_0().child(body))
            .into_any_element()
    }

    /// The selected commit's tags, each with 推送 and 删除.
    fn graph_tags_row(
        &self,
        repo: &RepoId,
        tags: Vec<String>,
        colors: theme::Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id("graph-tags")
            .w_full()
            .min_h(theme::ROW_HEIGHT)
            .px_3()
            .gap_3()
            .flex_wrap()
            .border_b_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_CAPTION)
            .children(tags.into_iter().enumerate().map(|(i, name)| {
                let (push_repo, push_name) = (repo.clone(), name.clone());
                let (delete_repo, delete_name) = (repo.clone(), name.clone());
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Tag)
                            .size(theme::ICON_SIZE)
                            .text_color(colors.muted),
                    )
                    .child(name.clone())
                    .child(
                        Button::new(("graph-tag-push", i))
                            .xsmall()
                            .ghost()
                            .icon(IconName::ArrowUp)
                            .tooltip(format!("推送标签“{name}”"))
                            .accessibility_label(format!("推送标签“{name}”"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.graph_push_tag(&push_repo, &push_name, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("graph-tag-delete", i))
                            .xsmall()
                            .ghost()
                            .icon(IconName::Trash)
                            .tooltip(format!("删除标签“{name}”…"))
                            .accessibility_label(format!("删除标签“{name}”"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.scm_delete_tag(&delete_repo, delete_name.clone(), window, cx);
                            })),
                    )
            }))
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

fn is_head(reference: &GraphRef) -> bool {
    reference.head || reference.kind == RefKind::Head
}

/// Git Graph's node tooltip: whether HEAD contains the commit, and the loaded branches and
/// tags that do (the refs on the commit and on its loaded descendants).
fn node_tooltip(graph: &GitGraph, index: usize) -> String {
    let Some(commit) = graph.commits.get(index) else {
        return String::new();
    };
    let mut seen = vec![false; graph.commits.len()];
    let mut stack = vec![index];
    let (mut head, mut branches, mut tags) = (false, Vec::new(), Vec::new());
    while let Some(row) = stack.pop() {
        if !std::mem::replace(&mut seen[row], true) {
            stack.extend(graph.rows.get(row).map_or(&[][..], |row| &row.children));
        }
    }
    // Top to bottom, as the refs appear in the graph.
    for row in (0..seen.len()).filter(|&row| seen[row]) {
        for reference in &graph.commits[row].refs {
            head |= is_head(reference);
            match reference.kind {
                RefKind::Branch | RefKind::Remote => branches.push(reference.name.as_str()),
                RefKind::Tag => tags.push(reference.name.as_str()),
                RefKind::Head => {}
            }
        }
    }
    let short: String = commit.hash.chars().take(7).collect();
    let mut lines = vec![format!("提交 {short}")];
    if graph
        .commits
        .iter()
        .any(|commit| commit.refs.iter().any(is_head))
    {
        lines.push(
            if head {
                "包含在 HEAD 中"
            } else {
                "不在 HEAD 中"
            }
            .to_string(),
        );
    }
    for (label, names) in [("分支", branches), ("标签", tags)] {
        if !names.is_empty() {
            lines.push(format!("{label}：{}", limited(&names)));
        }
    }
    lines.join("\n")
}

/// Git Graph keeps the first and last five of a long ref list.
fn limited(names: &[&str]) -> String {
    if names.len() <= 10 {
        return names.join("、");
    }
    let mut kept = names[..5].to_vec();
    kept.push("…");
    kept.extend_from_slice(&names[names.len() - 5..]);
    kept.join("、")
}

/// A branch / remote / tag / HEAD chip in front of the subject, outlined in the commit's lane
/// color as in VS Code Git Graph; the checked-out branch is filled with the accent.
fn ref_chip(reference: &GraphRef, lane: Hsla, colors: theme::Colors) -> AnyElement {
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
        .border_1()
        .border_color(lane)
        .text_size(theme::TEXT_BADGE)
        .map(|chip| {
            if reference.head {
                chip.bg(colors.accent).text_color(colors.badge_fg)
            } else {
                chip.bg(colors.keycap).text_color(colors.foreground)
            }
        })
        .children(icon.map(|icon| {
            Icon::new(icon)
                .size(theme::AGENT_GLYPH_ICON)
                .when(!reference.head, |icon| icon.text_color(lane))
        }))
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

    /// (lane, [(from, to)] of the lines down) per row, colors left out.
    fn shape(rows: &[GraphRow]) -> Vec<(usize, Vec<(usize, usize)>)> {
        rows.iter()
            .map(|row| {
                let down = row.down.iter().map(|&(from, to, _)| (from, to)).collect();
                (row.lane, down)
            })
            .collect()
    }

    #[test]
    fn linear_history_stays_on_one_lane() {
        let (rows, lanes) = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(lanes, 1);
        assert_eq!(
            shape(&rows),
            [(0, vec![(0, 0)]), (0, vec![(0, 0)]), (0, vec![])]
        );
        assert_eq!(rows[2].children, [1]);
    }

    #[test]
    fn a_merged_branch_runs_down_to_the_shared_base() {
        // m merges f into main; main's tip b shares the base e. f's line runs beside b and
        // curves into e's node instead of joining main's line early.
        let (rows, lanes) = layout(&[
            commit("m", &["b", "f"]),
            commit("f", &["e"]),
            commit("b", &["e"]),
            commit("e", &[]),
        ]);
        assert_eq!(lanes, 2);
        assert_eq!(
            shape(&rows),
            [
                (0, vec![(0, 0), (0, 1)]),
                (1, vec![(0, 0), (1, 1)]),
                (0, vec![(0, 0), (1, 0)]),
                (0, vec![]),
            ]
        );
        assert_ne!(rows[0].color, rows[1].color);
        assert_eq!(rows[0].color, rows[2].color);
        assert_eq!(rows[3].children, [1, 2]);
    }

    #[test]
    fn siblings_meet_at_their_parent() {
        let (rows, lanes) = layout(&[
            commit("x", &["root"]),
            commit("y", &["root"]),
            commit("root", &[]),
        ]);
        assert_eq!(lanes, 2);
        assert_eq!(
            shape(&rows),
            [(0, vec![(0, 0)]), (1, vec![(0, 0), (1, 0)]), (0, vec![])]
        );
    }

    #[test]
    fn lines_shift_left_into_freed_lanes() {
        // a's lane ends at root; b's line then moves over to lane 0 for c.
        let (rows, lanes) = layout(&[
            commit("a", &["root"]),
            commit("b", &["c"]),
            commit("root", &[]),
            commit("c", &[]),
        ]);
        assert_eq!(lanes, 2);
        assert_eq!(
            shape(&rows),
            [
                (0, vec![(0, 0)]),
                (1, vec![(0, 0), (1, 1)]),
                (0, vec![(1, 0)]),
                (0, vec![]),
            ]
        );
    }

    #[test]
    fn colors_are_reused_after_a_branch_ends() {
        let (rows, lanes) = layout(&[
            commit("b", &["a"]),
            commit("a", &[]),
            commit("d", &["c"]),
            commit("c", &[]),
        ]);
        assert_eq!(lanes, 1);
        assert!(rows.iter().all(|row| row.lane == 0 && row.color == 0));
        assert!(rows[1].down.is_empty());
    }

    #[test]
    fn a_parent_beyond_the_loaded_page_keeps_its_line() {
        // y's parent is not loaded: its line passes z and stops at the last row.
        let (rows, lanes) = layout(&[commit("y", &["older"]), commit("z", &[])]);
        assert_eq!(lanes, 2);
        assert_eq!(shape(&rows), [(0, vec![(0, 0)]), (1, vec![])]);
    }

    #[test]
    fn random_histories_draw_continuous_lines() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move |bound: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % bound as u64) as usize
        };
        for _ in 0..50 {
            let count = 2 + next(120);
            let hashes: Vec<String> = (0..count).map(|n| format!("c{n}")).collect();
            let commits: Vec<GraphCommit> = (0..count)
                .map(|n| {
                    // Parents come later in date order; some point past the loaded page.
                    let parents: Vec<String> = (0..next(3))
                        .map(|_| match count - n - 1 {
                            0 => "older".to_string(),
                            later => hashes[n + 1 + next(later.min(8))].clone(),
                        })
                        .collect();
                    let parents: Vec<&str> = parents.iter().map(String::as_str).collect();
                    commit(&hashes[n], &parents)
                })
                .collect();
            let (rows, lanes) = layout(&commits);
            for (n, row) in rows.iter().enumerate() {
                assert!(row.lane < lanes);
                let up = n.checked_sub(1).map_or(&[][..], |above| &rows[above].down);
                for &(from, to, _) in &row.down {
                    assert!(from.max(to) < lanes);
                    assert!(from == row.lane || up.iter().any(|&(_, end, _)| end == from));
                }
                for &(_, to, _) in up {
                    assert!(to == row.lane || row.down.iter().any(|&(start, ..)| start == to));
                }
            }
        }
    }

    #[test]
    fn the_node_tooltip_lists_refs_of_descendants() {
        let reference = |kind, name: &str, head| GraphRef {
            kind,
            name: name.into(),
            head,
        };
        let mut commits = vec![
            commit("m", &["b", "f"]),
            commit("f", &["e"]),
            commit("b", &["e"]),
            commit("e", &[]),
        ];
        commits[0].refs = vec![reference(RefKind::Branch, "main", true)];
        commits[1].refs = vec![
            reference(RefKind::Branch, "feature", false),
            reference(RefKind::Tag, "v1", false),
        ];
        let (rows, lanes) = layout(&commits);
        let mut graph = GitGraph::new(workspace_editor_core::Repository {
            id: RepoId("/tmp/repo/.git".into()),
            worktree: "/tmp/repo".into(),
            common_dir: "/tmp/repo/.git".into(),
        });
        graph.commits = commits;
        graph.rows = rows;
        graph.lanes = lanes;
        assert_eq!(
            node_tooltip(&graph, 3),
            "提交 e\n包含在 HEAD 中\n分支：main、feature\n标签：v1"
        );
        assert_eq!(
            node_tooltip(&graph, 1),
            "提交 f\n包含在 HEAD 中\n分支：main、feature\n标签：v1"
        );
        graph.commits[0].refs.clear();
        graph.commits[2].refs = vec![reference(RefKind::Head, "", false)];
        assert_eq!(
            node_tooltip(&graph, 1),
            "提交 f\n不在 HEAD 中\n分支：feature\n标签：v1"
        );
        let names: Vec<String> = (0..12).map(|n| format!("t{n}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(
            limited(&names),
            "t0、t1、t2、t3、t4、…、t7、t8、t9、t10、t11"
        );
    }
}

#[cfg(test)]
#[path = "graph_ui_tests.rs"]
mod ui_tests;
