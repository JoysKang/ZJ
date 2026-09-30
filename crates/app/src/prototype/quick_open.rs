//! ⌘P "转到文件": a VS Code-style quick open over the workspace path index.

use super::Prototype;
use super::SINGLE_LINE;
use crate::{file_icons, theme};
use gpui_kit::{
    component::{
        Sizable, h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::{path::PathBuf, time::Duration};

pub(super) struct QuickOpen {
    input: Entity<InputState>,
    results: Vec<PathBuf>,
    selected: usize,
    generation: u64,
    task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl Prototype {
    pub(super) fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quick_open.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("按名称搜索文件"));
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
            results: Vec::new(),
            selected: 0,
            generation: 0,
            task: None,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        });
        self.update_quick_open(window, cx);
    }

    /// Empty query lists open files, most recent first; otherwise fuzzy results from the index.
    pub(super) fn update_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let recent: Vec<PathBuf> = self
            .documents
            .iter()
            .rev()
            .map(|doc| doc.path.clone())
            .collect();
        let index = self.index.clone();
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
                .background_spawn(async move { index.search(&query) })
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
        if let Some(quick) = self.quick_open.as_mut()
            && !quick.results.is_empty()
        {
            let last = quick.results.len() - 1;
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
        let count = quick.results.len();
        let visible = count.min(theme::QUICK_OPEN_ROWS);
        let query_empty = quick.input.read(cx).value().trim().is_empty();
        let note = if count > 0 {
            None
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
