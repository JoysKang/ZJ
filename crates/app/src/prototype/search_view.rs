//! The Search view (⌘⇧F): full-text search across the workspace, laid out like VS Code's —
//! the query with Aa / ab / .* toggles, collapsible "包含的文件" / "排除的文件" globs with the
//! "使用排除设置和忽略文件" switch, then matches grouped by file. File-name search stays on ⌘P.

use super::{Prototype, SINGLE_LINE, navigation::Placement, sidebar::chevron};
use crate::text_search::{self, FileMatches, Matcher, Options, Progress};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Typing settles for this long before a search starts.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// How often a running search hands its results to the view.
const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy)]
enum SearchRow {
    File(usize),
    Line(usize, usize),
}

pub(super) struct SearchState {
    pub query: Entity<InputState>,
    include: Entity<InputState>,
    exclude: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    details: bool,
    results: Vec<FileMatches>,
    collapsed: HashSet<PathBuf>,
    rows: Vec<SearchRow>,
    matches: usize,
    truncated: bool,
    error: Option<String>,
    running: bool,
    /// A search was asked for before the file index existed.
    waiting_for_index: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl SearchState {
    pub fn new(window: &mut Window, cx: &mut Context<Prototype>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let include =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 *.py, ./src, **/tests"));
        let exclude = cx.new(|cx| InputState::new(window, cx).placeholder("例如 .venv, *.min.js"));
        let rerun = |this: &mut Prototype,
                     _: &Entity<InputState>,
                     event: &InputEvent,
                     window: &mut Window,
                     cx: &mut Context<Prototype>| {
            match event {
                InputEvent::Change => this.schedule_search(DEBOUNCE, window, cx),
                InputEvent::PressEnter { .. } => this.schedule_search(Duration::ZERO, window, cx),
                _ => {}
            }
        };
        let subscriptions = vec![
            cx.subscribe_in(&query, window, rerun),
            cx.subscribe_in(&include, window, rerun),
            cx.subscribe_in(&exclude, window, rerun),
        ];
        Self {
            query,
            include,
            exclude,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            details: false,
            results: Vec::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            matches: 0,
            truncated: false,
            error: None,
            running: false,
            waiting_for_index: false,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (f, file) in self.results.iter().enumerate() {
            self.rows.push(SearchRow::File(f));
            if !self.collapsed.contains(&file.path) {
                self.rows
                    .extend((0..file.lines.len()).map(|l| SearchRow::Line(f, l)));
            }
        }
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Prototype {
    /// ⌘⇧F: the Search view with the query focused (and the selection, if any, as the query).
    pub(super) fn find_in_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = super::Sidebar::Search;
        self.sidebar_visible = true;
        let selected = self
            .active_editor()
            .map(|editor| editor.read(cx).selected_text().to_string());
        if let Some(text) = selected.filter(|t| !t.is_empty() && !t.contains('\n')) {
            self.search
                .query
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
        self.search.query.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        cx.notify();
    }

    fn search_options(&self, cx: &App) -> Options {
        let search = &self.search;
        Options {
            pattern: search.query.read(cx).value().to_string(),
            case_sensitive: search.case_sensitive,
            whole_word: search.whole_word,
            regex: search.regex,
            include: search.include.read(cx).value().to_string(),
            exclude: search.exclude.read(cx).value().to_string(),
            use_excludes: cx.global::<crate::settings::Settings>().search_use_excludes,
            show_hidden: self.show_hidden,
        }
    }

    /// Starts a new search after `delay`, cancelling the running one.
    pub(super) fn schedule_search(
        &mut self,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let search = &mut self.search;
        search.cancel.store(true, Ordering::Relaxed);
        search.cancel = Arc::new(AtomicBool::new(false));
        search.generation += 1;
        search.task = None;
        search.error = None;
        search.waiting_for_index = false;
        let options = self.search_options(cx);
        let Some(root) = self.root.clone() else {
            return;
        };
        if options.pattern.is_empty() {
            let search = &mut self.search;
            search.results.clear();
            search.rows.clear();
            search.matches = 0;
            search.truncated = false;
            search.running = false;
            cx.notify();
            return;
        }
        let matcher = match Matcher::new(&options) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.search.error = Some(error);
                self.search.running = false;
                cx.notify();
                return;
            }
        };
        let index = self.index.clone();
        if options.use_excludes && index.is_none() {
            // build_index starts the search once the file list exists.
            self.search.waiting_for_index = true;
            self.search.running = true;
            cx.notify();
            return;
        }
        let generation = self.search.generation;
        let cancel = self.search.cancel.clone();
        let progress = Arc::new(Progress::default());
        self.search.running = true;
        self.search.task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let worker = progress.clone();
            let stop = cancel.clone();
            std::thread::spawn(move || {
                let files = match &index {
                    Some(index) if options.use_excludes => {
                        let relative: Vec<PathBuf> = index
                            .paths()
                            .into_iter()
                            .filter_map(|p| p.strip_prefix(&root).ok().map(PathBuf::from))
                            .collect();
                        text_search::filter(relative, &matcher, true)
                    }
                    _ => text_search::filter(text_search::walk_all(&root, &stop), &matcher, false),
                };
                text_search::run(&root, files, &matcher, &worker, &stop);
            });
            let mut first = true;
            loop {
                cx.background_executor().timer(POLL).await;
                let done = progress.done.load(Ordering::Relaxed);
                let fresh: Vec<FileMatches> =
                    std::mem::take(&mut *progress.results.lock().unwrap());
                let alive = this
                    .update(cx, |this, cx| {
                        let search = &mut this.search;
                        if search.generation != generation {
                            return false;
                        }
                        if first {
                            search.results.clear();
                            search.collapsed.clear();
                            search.scroll = UniformListScrollHandle::new();
                            first = false;
                        }
                        search.results.extend(fresh);
                        search.results.sort_by(|a, b| a.relative.cmp(&b.relative));
                        search.matches = progress.matches.load(Ordering::Relaxed);
                        search.truncated = progress.truncated.load(Ordering::Relaxed);
                        search.running = !done;
                        search.rebuild_rows();
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if done || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// The file index arrived after a search was asked for.
    pub(super) fn resume_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.waiting_for_index {
            self.schedule_search(Duration::ZERO, window, cx);
        }
    }

    fn open_match(
        &mut self,
        file: usize,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self.search.results.get(file) else {
            return;
        };
        let path = found.path.clone();
        let Some(line) = found.lines.get(line) else {
            return;
        };
        let place = Placement::Point {
            line: line.line,
            column: line.column,
            len: line.len,
        };
        self.remember(cx);
        self.go(path, place, window, cx);
    }

    fn toggle_option(
        &mut self,
        which: fn(&mut SearchState) -> &mut bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let flag = which(&mut self.search);
        *flag = !*flag;
        self.schedule_search(Duration::ZERO, window, cx);
    }

    fn option_button(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        on: bool,
        which: fn(&mut SearchState) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .small()
            .ghost()
            .icon(icon)
            .selected(on)
            .tooltip(label)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_option(which, window, cx)))
    }

    pub(super) fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let search = &self.search;
        let use_excludes = cx.global::<crate::settings::Settings>().search_use_excludes;
        let query_options = h_flex()
            .gap_0p5()
            .child(self.option_button(
                "search-case",
                IconName::CaseSensitive,
                "区分大小写",
                search.case_sensitive,
                |s| &mut s.case_sensitive,
                cx,
            ))
            .child(self.option_button(
                "search-word",
                IconName::WholeWord,
                "全字匹配",
                search.whole_word,
                |s| &mut s.whole_word,
                cx,
            ))
            .child(self.option_button(
                "search-regex",
                IconName::Regex,
                "使用正则表达式",
                search.regex,
                |s| &mut s.regex,
                cx,
            ));
        let label = |text: &'static str| {
            div()
                .pt_1()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(text)
        };
        let summary = if let Some(error) = &search.error {
            Some((error.clone(), colors.deleted))
        } else if search.query.read(cx).value().is_empty() {
            None
        } else if search.running && search.results.is_empty() {
            Some(("正在搜索…".into(), colors.muted))
        } else if search.results.is_empty() {
            Some(("未找到结果。请检查排除设置和忽略文件".into(), colors.muted))
        } else {
            Some((
                format!(
                    "{} 个文件中有 {} 个结果{}",
                    search.results.len(),
                    search.matches,
                    if search.truncated {
                        "（结果过多，只显示前 10,000 个）"
                    } else if search.running {
                        "，仍在搜索…"
                    } else {
                        ""
                    }
                ),
                colors.muted,
            ))
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .px(theme::TREE_BASE)
                    .pb_1()
                    .gap_1()
                    .flex_shrink_0()
                    .child(Input::new(&search.query).small().suffix(query_options))
                    .child(
                        h_flex().justify_end().child(
                            Button::new("search-details")
                                .xsmall()
                                .ghost()
                                .icon(IconName::Ellipsis)
                                .selected(search.details)
                                .tooltip("切换搜索详细信息")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.search.details = !this.search.details;
                                    cx.notify();
                                })),
                        ),
                    )
                    .when(search.details, |panel| {
                        panel
                            .child(label("包含的文件"))
                            .child(Input::new(&search.include).small())
                            .child(label("排除的文件"))
                            .child(
                                Input::new(&search.exclude).small().suffix(
                                    Button::new("search-use-excludes")
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Settings)
                                        .selected(use_excludes)
                                        .tooltip(if use_excludes {
                                            "使用排除设置和忽略文件（已开启：跳过 .gitignore、.venv、node_modules 等）"
                                        } else {
                                            "使用排除设置和忽略文件（已关闭：搜索所有文件）"
                                        })
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let on = !cx.global::<crate::settings::Settings>().search_use_excludes;
                                            this.change_settings(window, cx, |s| s.search_use_excludes = on);
                                            this.schedule_search(Duration::ZERO, window, cx);
                                        })),
                                ),
                            )
                    })
                    .when_some(summary, |panel, (text, color)| {
                        panel.child(
                            div()
                                .pt_1()
                                .text_size(theme::TEXT_CAPTION)
                                .text_color(color)
                                .child(text),
                        )
                    }),
            )
            .child(
                uniform_list(
                    "search-results",
                    search.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|index| this.search_row(index, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&search.scroll)
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }

    fn search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let base = h_flex()
            .id(("search-row", index))
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pr_2()
            .gap_1()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(theme::TEXT_BODY)
            .cursor_pointer()
            .hover(|row| row.bg(colors.hover));
        let row = match self.search.rows[index] {
            SearchRow::File(f) => {
                let file = &self.search.results[f];
                let name = file
                    .relative
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎");
                let folder = file
                    .relative
                    .parent()
                    .map(|p| p.to_string_lossy().replace(SINGLE_LINE, "⏎"))
                    .unwrap_or_default();
                let expanded = !self.search.collapsed.contains(&file.path);
                let path = file.path.clone();
                base.pl(theme::ROW_INSET)
                    .role(Role::TreeItem)
                    .aria_label(format!(
                        "{} · {} 个结果",
                        file.relative.display(),
                        file.count
                    ))
                    .child(chevron(expanded, colors.muted))
                    .child(file_icons::icon(file_icons::for_file(&name)))
                    .child(div().flex_shrink_0().child(name))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(theme::TEXT_CAPTION)
                            .text_color(colors.muted)
                            .child(folder),
                    )
                    .child(super::scm::count_badge(file.count, colors))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.search.collapsed.remove(&path) {
                            this.search.collapsed.insert(path.clone());
                        }
                        this.search.rebuild_rows();
                        cx.notify();
                    }))
            }
            SearchRow::Line(f, l) => {
                let line = &self.search.results[f].lines[l];
                let highlight = HighlightStyle {
                    background_color: Some(colors.find_match),
                    ..Default::default()
                };
                let text = StyledText::new(line.preview.clone())
                    .with_highlights(line.ranges.iter().map(|r| (r.clone(), highlight)));
                base.pl(theme::TREE_BASE + theme::TREE_STEP * 3.)
                    .role(Role::Button)
                    .aria_label(format!("第 {} 行：{}", line.line + 1, line.preview))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(text),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.open_match(f, l, window, cx)),
                    )
            }
        };
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}
