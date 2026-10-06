//! The Search view (⌘⇧F): full-text search across the workspace, laid out like VS Code's —
//! the query with Aa / ab / .* toggles, collapsible "包含的文件" / "排除的文件" globs with the
//! "使用排除设置和忽略文件" switch, then matches grouped by file. File-name search stays on ⌘P.

use super::{
    SINGLE_LINE, Workbench, navigation::Placement, search_replace::ReplaceState, sidebar::chevron,
};
use crate::replace::{Finder, Query};
use crate::text_search::{self, FileMatches, Matcher, Options, Progress};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Selectable, Sizable,
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
    pub include: Entity<InputState>,
    exclude: Entity<InputState>,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub details: bool,
    pub results: Vec<FileMatches>,
    pub collapsed: HashSet<PathBuf>,
    rows: Vec<SearchRow>,
    pub matches: usize,
    truncated: bool,
    error: Option<String>,
    running: bool,
    /// A search was asked for before the file index existed.
    waiting_for_index: bool,
    generation: u64,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    pub replace: ReplaceState,
    /// The running search compiled for replacing (`finder`) and for searching a rewritten
    /// file again (`matcher`).
    pub finder: Option<Arc<Finder>>,
    pub matcher: Option<Arc<Matcher>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchState {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("搜索"));
        let include =
            cx.new(|cx| InputState::new(window, cx).placeholder("例如 *.py, ./src, **/tests"));
        let exclude = cx.new(|cx| InputState::new(window, cx).placeholder("例如 .venv, *.min.js"));
        let rerun = |this: &mut Workbench,
                     _: &Entity<InputState>,
                     event: &InputEvent,
                     window: &mut Window,
                     cx: &mut Context<Workbench>| {
            match event {
                InputEvent::Change => {
                    this.search.replace.summary = None;
                    this.schedule_search(DEBOUNCE, window, cx)
                }
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
            replace: ReplaceState::new(window, cx),
            finder: None,
            matcher: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn rebuild_rows(&mut self) {
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

impl Workbench {
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

    /// The Explorer's 「在文件夹中查找」: the Search view limited to `folder` (all files for
    /// the root), its details open, the query focused.
    pub(super) fn find_in_folder(
        &mut self,
        folder: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let include = self
            .root
            .as_deref()
            .and_then(|root| folder.strip_prefix(root).ok())
            .filter(|relative| !relative.as_os_str().is_empty())
            .map(|relative| crate::text_search::folder_pattern(&relative.to_string_lossy()))
            .unwrap_or_default();
        self.search
            .include
            .update(cx, |input, cx| input.set_value(include, window, cx));
        self.search.details = true;
        self.find_in_files(window, cx);
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
            show_hidden: self.explorer.show_hidden,
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
            Ok(matcher) => Arc::new(matcher),
            Err(error) => {
                self.search.error = Some(error);
                self.search.running = false;
                cx.notify();
                return;
            }
        };
        self.search.matcher = Some(matcher.clone());
        self.search.finder = Finder::new(&Query {
            pattern: options.pattern.clone(),
            case_sensitive: options.case_sensitive,
            whole_word: options.whole_word,
            regex: options.regex,
        })
        .ok()
        .map(Arc::new);
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

    pub(super) fn open_match(
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
            .gap_1()
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
        } else if search.results.is_empty() && search.replace.summary.is_some() {
            // Everything was replaced; the replace summary says so.
            None
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
        let replace = &search.replace;
        let replace_toggle = Button::new("search-toggle-replace")
            .xsmall()
            .ghost()
            .icon(if replace.open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .tooltip("切换替换")
            .on_click(cx.listener(|this, _, window, cx| this.toggle_search_replace(window, cx)));
        let preserve_case = Button::new("search-preserve-case")
            .small()
            .ghost()
            .icon(IconName::CaseUpper)
            .selected(replace.preserve_case)
            .tooltip("保留大小写")
            .on_click(cx.listener(|this, _, _, cx| {
                this.search.replace.preserve_case = !this.search.replace.preserve_case;
                cx.notify();
            }));
        let replace_all = Button::new("search-replace-all")
            .small()
            .ghost()
            .icon(IconName::ReplaceAll)
            .tooltip("全部替换")
            .disabled(search.results.is_empty() || replace.running)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_replace_all(window, cx)));
        let inputs = h_flex()
            .ml(theme::SEARCH_CHEVRON_OUTDENT)
            .gap_1()
            .items_start()
            .child(div().pt_1().child(replace_toggle))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(Input::new(&search.query).small().suffix(query_options))
                    .when(replace.open, |inputs| {
                        inputs.child(
                            h_flex()
                                .gap_1()
                                .child(div().flex_1().min_w_0().child(
                                    Input::new(&replace.input).small().suffix(preserve_case),
                                ))
                                .child(replace_all),
                        )
                    }),
            );
        let replace_summary = replace.summary.clone().map(|(text, warn)| {
            h_flex()
                .gap_1()
                .pt_1()
                .text_size(theme::TEXT_CAPTION)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if warn { colors.deleted } else { colors.muted })
                        .child(text),
                )
                .when(replace.has_undo(), |row| {
                    row.child(
                        Button::new("search-undo-replace")
                            .xsmall()
                            .ghost()
                            .label("撤销")
                            .disabled(replace.running)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.undo_replace(window, cx)),
                            ),
                    )
                })
        });
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .px(theme::TREE_BASE)
                    .pb_1()
                    .gap_1()
                    .flex_shrink_0()
                    .child(inputs)
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
                    })
                    .children(replace_summary),
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

    /// Buttons shown while a row is hovered (VS Code's inline actions).
    fn row_actions(&self, group: SharedString, buttons: Vec<Button>) -> AnyElement {
        h_flex()
            .flex_shrink_0()
            .gap_1()
            .invisible()
            .group_hover(group, |actions| actions.visible())
            // The row's own click must not fire under the buttons.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(buttons)
            .into_any_element()
    }

    fn row_button(id: (&'static str, usize), icon: IconName, label: &'static str) -> Button {
        Button::new(id)
            .xsmall()
            .ghost()
            .icon(icon)
            .tooltip(label)
            .accessibility_label(label)
    }

    fn search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let group: SharedString = format!("search-row-{index}").into();
        let replacing = self.search.replace.open;
        let busy = self.search.replace.running;
        let base = h_flex()
            .id(("search-row", index))
            .group(group.clone())
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
                let mut buttons = Vec::new();
                if replacing {
                    buttons.push(
                        Self::row_button(
                            ("search-file-replace", f),
                            IconName::ReplaceAll,
                            "全部替换",
                        )
                        .disabled(busy)
                        .on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.replace_file(f, window, cx)
                            }),
                        ),
                    );
                }
                buttons.push(
                    Self::row_button(("search-file-ignore", f), IconName::Close, "忽略")
                        .on_click(cx.listener(move |this, _, _, cx| this.ignore_file(f, cx))),
                );
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
                    .child(self.row_actions(group, buttons))
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
                let found = HighlightStyle {
                    background_color: Some(colors.find_match),
                    ..Default::default()
                };
                let text = if replacing {
                    // The match struck through in red, then the replacement in green.
                    let (text, removed, added) =
                        self.replace_preview(&line.preview, &line.ranges, cx);
                    let removed_style = HighlightStyle {
                        background_color: Some(colors.diff_deleted_text),
                        strikethrough: Some(StrikethroughStyle {
                            thickness: theme::STRIKE,
                            color: Some(colors.foreground),
                        }),
                        ..Default::default()
                    };
                    let added_style = HighlightStyle {
                        background_color: Some(colors.diff_added_text),
                        ..Default::default()
                    };
                    let mut runs: Vec<(std::ops::Range<usize>, HighlightStyle)> = removed
                        .into_iter()
                        .map(|r| (r, removed_style))
                        .chain(added.into_iter().map(|r| (r, added_style)))
                        .filter(|(r, _)| r.start < r.end)
                        .collect();
                    runs.sort_by_key(|(r, _)| r.start);
                    StyledText::new(text).with_highlights(runs)
                } else {
                    StyledText::new(line.preview.clone()).with_highlights(
                        line.ranges
                            .iter()
                            .filter(|r| r.start < r.end)
                            .map(|r| (r.clone(), found)),
                    )
                };
                let mut buttons = Vec::new();
                if replacing {
                    buttons.push(
                        Self::row_button(("search-line-replace", index), IconName::Replace, "替换")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.replace_line(f, l, window, cx)
                            })),
                    );
                }
                buttons.push(
                    Self::row_button(("search-line-ignore", index), IconName::Close, "忽略")
                        .on_click(cx.listener(move |this, _, _, cx| this.ignore_line(f, l, cx))),
                );
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
                    .child(self.row_actions(group, buttons))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.search.replace.open {
                            this.preview_replace(f, window, cx)
                        } else {
                            this.open_match(f, l, window, cx)
                        }
                    }))
            }
        };
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}
