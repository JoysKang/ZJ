//! ⌘P "转到文件": a VS Code-style quick open over the workspace path index. A query starting
//! with `:` goes to a line of the active file (⌃G). The same panel lists symbols (⌘⇧O) and
//! definition / reference locations.

use super::SINGLE_LINE;
use super::Workbench;
use super::navigation::Target;
use crate::symbols::Kind;
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable, h_flex,
        input::{Input, InputEvent, InputState, RopeExt},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::{path::PathBuf, time::Duration};
use workspace_editor_core::RepoId;

/// A row of a symbol, location, command or branch list. `keys` are keycap labels of the
/// command's shortcut (empty when it has none or the row is not a command).
pub(super) struct PickItem {
    pub label: String,
    pub detail: String,
    pub icon: PickIcon,
    pub keys: Vec<String>,
    pub pick: Pick,
}

pub(super) enum PickIcon {
    File(&'static str),
    Symbol(Kind),
    Lucide(IconName),
    None,
}

/// What choosing a row does.
#[derive(Clone)]
pub(super) enum Pick {
    Jump(Target),
    /// Switches the repository to a branch (`remote`: creates the tracking local branch).
    Checkout {
        repo: RepoId,
        branch: String,
        remote: bool,
    },
    /// A new branch named by the query. Listed whatever is typed, first.
    CreateBranch {
        repo: RepoId,
        start: Option<String>,
    },
    /// A new tag at `commit` named by the query; its message is asked for next.
    TagName {
        repo: RepoId,
        commit: String,
    },
    /// Creates tag `name` with the query as its message (annotated; lightweight when empty).
    CreateTag {
        repo: RepoId,
        commit: String,
        name: String,
        push: bool,
    },
    /// The checked-out branch: nothing to do.
    Close,
    /// `:N:C` in the file search: 0-based line and column; `None` (no file, or the line is
    /// out of range) keeps the panel open, as in VS Code.
    Line(Option<(u32, u32)>),
    /// A `>` command palette row: the index into [`super::commands::COMMANDS`].
    Command(usize),
}

/// The ⌘P panel's state: file search, command palette (`>`) or go to line (`:`).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum LauncherMode {
    Files,
    Commands,
    Line,
}

pub(super) struct QuickOpen {
    pub(super) input: Entity<InputState>,
    /// Opened as the file search (⌘P), where a `>` / `:` prefix switches to commands / lines.
    launcher: bool,
    /// The launcher's prefix state (always `Files` for pickers over a fixed list).
    mode: LauncherMode,
    /// Keycap labels of each command's shortcut, resolved when the panel opened.
    command_keys: Vec<Vec<String>>,
    /// What had focus when the panel opened; a command is dispatched with it restored.
    previous_focus: Option<FocusHandle>,
    results: Vec<PathBuf>,
    /// `Some` for a symbol / location / command list: the items and the matching indexes.
    pub(super) items: Option<(Vec<PickItem>, Vec<usize>)>,
    empty_note: &'static str,
    selected: usize,
    generation: u64,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl Workbench {
    pub(super) fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_open.as_ref().is_some_and(|quick| quick.launcher) {
            return;
        }
        self.open_quick_open_with("", window, cx);
    }

    /// Opens the file search with `prefix` typed (`:` for ⌃G), or retypes the query of the
    /// one already open.
    pub(super) fn open_quick_open_with(
        &mut self,
        prefix: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(quick) = self.quick_open.as_ref().filter(|quick| quick.launcher) {
            let input = quick.input.clone();
            input.update(cx, |input, cx| {
                input.set_value(prefix.to_string(), window, cx);
                input.focus(window, cx);
            });
            self.update_quick_open(window, cx);
            return;
        }
        // Shortcut labels and command dispatch need the focus the panel takes away.
        let previous_focus = window.focused(cx);
        let command_keys = super::commands::shortcut_keys(window, cx);
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder("按名称搜索文件（输入 : 转到行，> 执行命令）");
            input.set_value(prefix.to_string(), window, cx);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_quick_open(window, cx);
                }
            },
        );
        self.quick_open = Some(QuickOpen {
            input,
            launcher: true,
            mode: LauncherMode::Files,
            command_keys,
            previous_focus,
            results: Vec::new(),
            items: None,
            empty_note: "",
            selected: 0,
            generation: 0,
            task: None,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        });
        self.update_quick_open(window, cx);
    }

    /// Opens the panel over a fixed list (symbols or locations), filtered as the user types.
    pub(super) fn open_picker(
        &mut self,
        items: Vec<PickItem>,
        placeholder: String,
        empty_note: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.quick_open = None;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_quick_open(window, cx);
                }
            },
        );
        let all = (0..items.len()).collect();
        self.quick_open = Some(QuickOpen {
            input,
            launcher: false,
            mode: LauncherMode::Files,
            command_keys: Vec::new(),
            previous_focus: None,
            results: Vec::new(),
            items: Some((items, all)),
            empty_note,
            selected: 0,
            generation: 0,
            task: None,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        });
        cx.notify();
    }

    /// The one row of `:N:C`: where Enter goes, or why it cannot go (VS Code's wording).
    fn line_item(&self, query: &str, cx: &App) -> PickItem {
        let row = |label: String, line| PickItem {
            label,
            detail: String::new(),
            icon: PickIcon::None,
            keys: Vec::new(),
            pick: Pick::Line(line),
        };
        let Some((_, editor)) = self.active_document() else {
            return row("请先打开一个文件".into(), None);
        };
        let state = editor.read(cx);
        let lines = state.text().lines_len() as u32;
        match super::navigation::parse_line_query(query) {
            Some((line, column)) if line <= lines => {
                let label = match column {
                    Some(column) => format!("转到第 {line} 行第 {column} 个字符"),
                    None => format!("转到第 {line} 行"),
                };
                row(
                    label,
                    Some((line - 1, column.map_or(0, |column| column - 1))),
                )
            }
            _ => {
                let here = state.cursor_position();
                row(
                    format!(
                        "当前行: {}，字符: {}。请输入 1 到 {lines} 之间的行号。",
                        here.line + 1,
                        here.character + 1
                    ),
                    None,
                )
            }
        }
    }

    fn quick_open_len(&self) -> usize {
        self.quick_open
            .as_ref()
            .map_or(0, |quick| match &quick.items {
                Some((_, filtered)) => filtered.len(),
                None => quick.results.len(),
            })
    }

    /// Empty query lists open files, most recent first; otherwise fuzzy results from the
    /// index. In the ⌘P launcher a `>` prefix lists commands and `:` goes to a line.
    pub(super) fn update_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(quick) = self.quick_open.as_ref().filter(|quick| quick.launcher) {
            let value = quick.input.read(cx).value().to_string();
            let mode = if value.starts_with('>') {
                LauncherMode::Commands
            } else if value.starts_with(':') {
                LauncherMode::Line
            } else {
                LauncherMode::Files
            };
            if mode != quick.mode {
                let input = quick.input.clone();
                input.update(cx, |input, cx| {
                    input.set_placeholder(
                        match mode {
                            LauncherMode::Files => "按名称搜索文件（输入 : 转到行，> 执行命令）",
                            LauncherMode::Commands => "输入命令名称",
                            LauncherMode::Line => "转到行（:行 或 :行:列）",
                        },
                        window,
                        cx,
                    );
                });
                if let Some(quick) = self.quick_open.as_mut() {
                    quick.mode = mode;
                }
            }
            match mode {
                LauncherMode::Line => {
                    let item = self.line_item(value.strip_prefix(':').unwrap_or_default(), cx);
                    if let Some(quick) = self.quick_open.as_mut() {
                        quick.generation += 1;
                        quick.task = None;
                        quick.selected = 0;
                        quick.items = Some((vec![item], vec![0]));
                    }
                    cx.notify();
                    return;
                }
                LauncherMode::Commands => {
                    let items = self
                        .quick_open
                        .as_ref()
                        .map(|quick| {
                            super::commands::COMMANDS
                                .iter()
                                .enumerate()
                                .map(|(i, command)| PickItem {
                                    label: command.name.into(),
                                    detail: String::new(),
                                    icon: PickIcon::None,
                                    keys: quick.command_keys.get(i).cloned().unwrap_or_default(),
                                    pick: Pick::Command(i),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(quick) = self.quick_open.as_mut() {
                        quick.generation += 1;
                        quick.task = None;
                        quick.selected = 0;
                        quick.items = Some((items, Vec::new()));
                    }
                }
                LauncherMode::Files => {
                    if let Some(quick) = self.quick_open.as_mut() {
                        quick.items = None;
                    }
                }
            }
        }
        let history = self.command_history.clone();
        if let Some(quick) = self.quick_open.as_mut()
            && let Some((items, filtered)) = &mut quick.items
        {
            let value = quick.input.read(cx).value();
            let text: &str = &value;
            let text = if quick.launcher && quick.mode == LauncherMode::Commands {
                text.strip_prefix('>').unwrap_or(text)
            } else {
                text
            };
            let query = crate::fuzzy::query_chars(text);
            let mut scored: Vec<(i32, usize)> = items
                .iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    if let Pick::CreateBranch { .. }
                    | Pick::TagName { .. }
                    | Pick::CreateTag { .. } = item.pick
                    {
                        // First with an empty query (nothing is sorted then), after real
                        // matches otherwise, so Enter on a typed branch checks it out. The
                        // query is a name or a message here, never a filter.
                        return Some((i32::MIN, i));
                    }
                    let key = item.label.to_lowercase();
                    crate::fuzzy::score(&query, &key, 0).map(|score| (score, i))
                })
                .collect();
            if !query.is_empty() {
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            }
            *filtered = scored.into_iter().map(|(_, i)| i).collect();
            if query.is_empty() && quick.launcher && quick.mode == LauncherMode::Commands {
                // 空查询时最近用过的命令在前，其余按命令表顺序（稳定排序）。
                let rank = |i: usize| {
                    history
                        .iter()
                        .position(|name| *name == items[i].label)
                        .unwrap_or(usize::MAX)
                };
                filtered.sort_by_key(|i| rank(*i));
            }
            quick.selected = 0;
            cx.notify();
            return;
        }
        let recent: Vec<PathBuf> = self
            .documents
            .iter()
            .rev()
            .map(|doc| doc.path.clone())
            .collect();
        let index = self.index.clone();
        let show_hidden = self.explorer.show_hidden;
        let Some(quick) = self.quick_open.as_mut() else {
            return;
        };
        quick.generation += 1;
        quick.selected = 0;
        let query = quick.input.read(cx).value().to_string();
        if query.trim().is_empty() {
            quick.results = recent;
            quick.task = None;
            cx.notify();
            return;
        }
        let Some(index) = index else {
            // build_index re-runs this when the index is ready.
            quick.results.clear();
            cx.notify();
            return;
        };
        let generation = quick.generation;
        quick.task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let result = cx
                .background_spawn(async move { index.search(&query, show_hidden) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(quick) = this.quick_open.as_mut()
                    && quick.generation == generation
                {
                    quick.results = result.paths;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub(super) fn close_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_open.take().is_some() {
            self.focus_active_editor(window, cx);
            cx.notify();
        }
    }

    fn confirm_quick_open(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(quick) = self.quick_open.as_ref()
            && let Some((items, filtered)) = &quick.items
        {
            let pick = filtered
                .get(index.unwrap_or(quick.selected))
                .map(|i| items[*i].pick.clone());
            let query = quick.input.read(cx).value().trim().to_string();
            if let Some(Pick::CreateBranch { repo, start }) = &pick
                && query.is_empty()
            {
                // As in VS Code, the name is asked for next.
                let item = PickItem {
                    label: String::new(),
                    detail: String::new(),
                    icon: PickIcon::Lucide(IconName::Plus),
                    keys: Vec::new(),
                    pick: Pick::CreateBranch {
                        repo: repo.clone(),
                        start: start.clone(),
                    },
                };
                self.open_picker(vec![item], "新分支名称".into(), "", window, cx);
                return;
            }
            if let Some(Pick::TagName { repo, commit }) = &pick {
                if query.is_empty() {
                    return;
                }
                // As in VS Code's 创建标签: the name, then an optional message.
                let items = [false, true]
                    .into_iter()
                    .map(|push| PickItem {
                        label: String::new(),
                        detail: String::new(),
                        icon: PickIcon::Lucide(if push {
                            IconName::ArrowUp
                        } else {
                            IconName::Tag
                        }),
                        keys: Vec::new(),
                        pick: Pick::CreateTag {
                            repo: repo.clone(),
                            commit: commit.clone(),
                            name: query.clone(),
                            push,
                        },
                    })
                    .collect();
                self.open_picker(
                    items,
                    "标签说明（可选；填写后创建附注标签）".into(),
                    "",
                    window,
                    cx,
                );
                return;
            }
            if let Some(Pick::Line(None)) = pick {
                return;
            }
            let focus = self
                .quick_open
                .as_ref()
                .and_then(|quick| quick.previous_focus.clone());
            self.quick_open = None;
            match pick {
                Some(Pick::Jump(target)) => self.jump_to(target, true, window, cx),
                Some(Pick::Line(Some((line, column)))) => self.go_to_line(line, column, window, cx),
                Some(Pick::Command(index)) => {
                    let command = &super::commands::COMMANDS[index];
                    let action = (command.action)();
                    self.command_history.retain(|name| *name != command.name);
                    self.command_history.insert(0, command.name);
                    self.command_history.truncate(20);
                    // Restore the focus the panel took, so context-sensitive commands
                    // (editor commands, the Agent panel) dispatch where the user was.
                    match focus {
                        Some(focus) => focus.focus(window, cx),
                        None => self.focus_active_editor(window, cx),
                    }
                    window.dispatch_action(action, cx);
                }
                Some(Pick::Checkout {
                    repo,
                    branch,
                    remote,
                }) => {
                    self.focus_active_editor(window, cx);
                    self.scm_checkout(&repo, branch, remote, window, cx);
                }
                Some(Pick::CreateBranch { repo, start }) => {
                    self.focus_active_editor(window, cx);
                    self.scm_create_branch(&repo, query, start, window, cx);
                }
                Some(Pick::CreateTag {
                    repo,
                    commit,
                    name,
                    push,
                }) => {
                    self.focus_active_editor(window, cx);
                    let operation = workspace_editor_git::WriteOperation::CreateTag {
                        name,
                        commit,
                        message: (!query.is_empty()).then_some(query),
                        push,
                    };
                    self.scm_request_for(&repo, operation, window, cx);
                }
                Some(Pick::TagName { .. }) | Some(Pick::Close) | Some(Pick::Line(None)) | None => {
                    self.focus_active_editor(window, cx)
                }
            }
            cx.notify();
            return;
        }
        let path = self
            .quick_open
            .as_ref()
            .and_then(|quick| quick.results.get(index.unwrap_or(quick.selected)).cloned());
        self.quick_open = None;
        match path {
            Some(path) => self.open_file(path, self.root.clone(), window, cx),
            None => self.focus_active_editor(window, cx),
        }
        cx.notify();
    }

    fn move_quick_open(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.quick_open_len();
        if let Some(quick) = self.quick_open.as_mut()
            && len > 0
        {
            let last = len - 1;
            quick.selected = quick.selected.saturating_add_signed(delta).min(last);
            quick
                .scroll
                .scroll_to_item(quick.selected, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn quick_open_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(quick) = self.quick_open.as_ref() else {
            return div().into_any_element();
        };
        if let Some((items, filtered)) = &quick.items {
            return self.pick_row(&items[filtered[index]], index, index == quick.selected, cx);
        }
        let path = &quick.results[index];
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let directory = path
            .parent()
            .map(|parent| {
                self.root
                    .as_ref()
                    .and_then(|root| parent.strip_prefix(root).ok())
                    .unwrap_or(parent)
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎")
            })
            .unwrap_or_default();
        let selected = index == quick.selected;
        h_flex()
            .id(("quick-open-row", index))
            .h(theme::ROW_HEIGHT)
            .w_full()
            .px_2()
            .gap_2()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .when(selected, |row| {
                row.bg(colors.selected).text_color(colors.selected_fg)
            })
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .child(file_icons::icon(file_icons::for_file(&name)))
            .child(div().flex_shrink_0().child(name))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(if selected {
                        colors.selected_fg
                    } else {
                        colors.muted
                    })
                    .child(directory),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_quick_open(Some(index), window, cx)
            }))
            .into_any_element()
    }

    pub(super) fn render_quick_open(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let quick = self.quick_open.as_ref()?;
        let colors = theme::colors(cx);
        let count = self.quick_open_len();
        let visible = count.min(theme::QUICK_OPEN_ROWS);
        let query_empty = quick.input.read(cx).value().trim().is_empty();
        let note = if count > 0 {
            None
        } else if quick.items.is_some() {
            Some(quick.empty_note)
        } else if query_empty {
            Some("输入文件名或路径片段；按 ↑↓ 选择，回车打开，Esc 关闭")
        } else if self.index.is_none() {
            Some("正在建立文件索引…")
        } else {
            Some("没有匹配的文件")
        };
        let panel = v_flex()
            .id("quick-open")
            .key_context("QuickOpen")
            .w(theme::QUICK_OPEN_WIDTH)
            .pb_1()
            .gap_1()
            .rounded(theme::RADIUS_LARGE)
            .border_1()
            .border_color(colors.command_border)
            .bg(colors.panel)
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.move_quick_open(-1, cx),
                    "down" => this.move_quick_open(1, cx),
                    "enter" => this.confirm_quick_open(None, window, cx),
                    "escape" => this.close_quick_open(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(div().p_1().child(Input::new(&quick.input).small()))
            .when_some(note, |panel, note| {
                panel.child(
                    div()
                        .px_3()
                        .h(theme::ROW_HEIGHT)
                        .flex()
                        .items_center()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(note),
                )
            })
            .when(count > 0, |panel| {
                panel.child(
                    uniform_list(
                        "quick-open-results",
                        count,
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.quick_open_row(index, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&quick.scroll)
                    .px_1()
                    .h(theme::ROW_HEIGHT * visible as f32),
                )
            });
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .pt(theme::TITLE_HEIGHT + theme::QUICK_OPEN_TOP)
                .flex()
                .justify_center()
                .items_start()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.close_quick_open(window, cx)),
                )
                .child(panel)
                .into_any_element(),
        )
    }
}

impl Workbench {
    fn pick_row(
        &self,
        item: &PickItem,
        index: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let icon = match item.icon {
            PickIcon::File(icon) => Some(file_icons::icon(icon).into_any_element()),
            PickIcon::Symbol(kind) => {
                let (name, color) = super::navigation::kind_icon(kind, colors);
                Some(
                    gpui_kit::component::Icon::new(name)
                        .size(theme::ICON_SIZE)
                        .text_color(color)
                        .into_any_element(),
                )
            }
            PickIcon::Lucide(name) => Some(
                gpui_kit::component::Icon::new(name)
                    .size(theme::ICON_SIZE)
                    .text_color(if selected {
                        colors.selected_fg
                    } else {
                        colors.muted
                    })
                    .into_any_element(),
            ),
            PickIcon::None => None,
        };
        let query = || {
            self.quick_open
                .as_ref()
                .map(|quick| quick.input.read(cx).value().trim().to_string())
                .unwrap_or_default()
        };
        let label = match &item.pick {
            Pick::CreateBranch { .. } => {
                let query = query();
                if query.is_empty() {
                    "创建新分支…".to_string()
                } else {
                    format!("创建新分支“{query}”")
                }
            }
            Pick::TagName { .. } => {
                let query = query();
                if query.is_empty() {
                    "输入新标签的名称".to_string()
                } else {
                    format!("添加标签“{query}”")
                }
            }
            Pick::CreateTag { name, push, .. } => {
                let kind = if query().is_empty() {
                    "轻量"
                } else {
                    "附注"
                };
                if *push {
                    format!("创建{kind}标签“{name}”并推送到远程")
                } else {
                    format!("创建{kind}标签“{name}”")
                }
            }
            _ => item.label.clone(),
        };
        h_flex()
            .id(("pick-row", index))
            .h(theme::ROW_HEIGHT)
            .w_full()
            .px_2()
            .gap_2()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .when(selected, |row| {
                row.bg(colors.selected).text_color(colors.selected_fg)
            })
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .when_some(icon, |row, icon| {
                row.child(div().flex_shrink_0().child(icon))
            })
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label.replace(SINGLE_LINE, "⏎")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(if selected {
                        colors.selected_fg
                    } else {
                        colors.muted
                    })
                    .child(item.detail.replace(SINGLE_LINE, "⏎")),
            )
            .when(!item.keys.is_empty(), |row| {
                // The keycaps of the command's shortcut, the Welcome page's style.
                row.child(
                    h_flex()
                        .flex_shrink_0()
                        .gap_1()
                        .children(item.keys.iter().map(|key| {
                            div()
                                .min_w(theme::KEYCAP)
                                .h(theme::KEYCAP)
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::RADIUS)
                                .bg(colors.keycap)
                                .text_size(theme::TEXT_SECTION)
                                .text_color(if selected {
                                    colors.selected_fg
                                } else {
                                    colors.muted
                                })
                                .child(key.clone())
                        })),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.confirm_quick_open(Some(index), window, cx)
            }))
            .into_any_element()
    }
}
