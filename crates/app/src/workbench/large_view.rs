//! The restricted viewer tab (A18; logic in `crate::large_file`): a file the editor does not
//! open is indexed in the background and shown read-only, one tab at a time like the Git
//! graph. Only the lines near the viewport are held; scrolling past them reads the next ones
//! in the background, and a change on disk (seen by the watcher or by a read) re-indexes.
//!
//! Lines are selected with the mouse (⇧ extends) and copied with ⌘C; ⌘F searches the file
//! line by line in the background (smart case), ⌃G goes to a line.

use super::*;
use crate::files::Restricted;
use crate::large_file::{self, Line, LineIndex, ReadError};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable, Theme,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Escape, Input, InputEvent, InputState},
        scroll::Scrollbar,
    },
    prelude::FluentBuilder,
};

gpui_kit::actions!(large_file, [CopyLargeLines]);

/// The viewer's key context (⌘C).
pub(crate) const CONTEXT: &str = "LargeView";

/// ⌘F in the viewer: the query, where the last search ended, and the search in flight.
struct LargeFind {
    input: Entity<InputState>,
    status: String,
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
    _subscription: Subscription,
}
use std::collections::BTreeSet;

/// Lines read around the viewport, so small scrolls need no read.
const MARGIN: usize = 512;

pub(super) struct LargeView {
    pub(super) path: PathBuf,
    reason: Restricted,
    index: Option<Arc<LineIndex>>,
    error: Option<String>,
    /// The first line held and the held lines.
    held: (usize, Arc<Vec<Line>>),
    /// The range being read, so a render does not ask twice.
    reading: Option<std::ops::Range<usize>>,
    /// Widest held line, in characters: the rows' width for horizontal scrolling.
    columns: usize,
    scroll: UniformListScrollHandle,
    cancel: Arc<AtomicBool>,
    generation: u64,
    _index_task: Option<Task<()>>,
    _read_task: Option<Task<()>>,
    focus: FocusHandle,
    /// Selected lines: where the selection started and the line clicked last.
    pub(super) selection: Option<(usize, usize)>,
    find: Option<LargeFind>,
}

impl Drop for LargeView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Workbench {
    /// Shows `path` in the viewer tab (replacing a file shown there).
    pub(super) fn open_large(
        &mut self,
        path: PathBuf,
        reason: Restricted,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if self.large.as_ref().is_none_or(|large| large.path != path) {
            self.large = Some(LargeView {
                path,
                reason,
                index: None,
                error: None,
                held: (0, Arc::default()),
                reading: None,
                columns: 0,
                scroll: UniformListScrollHandle::new(),
                cancel: Arc::default(),
                generation: 0,
                _index_task: None,
                _read_task: None,
                focus: cx.focus_handle(),
                selection: None,
                find: None,
            });
            self.large_index(cx);
        }
        self.message = format!("{reason}，以只读方式查看");
        self.select_pane(Pane::Large, window, cx);
    }

    pub(super) fn close_large(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.large = None;
        if self.active == Pane::Large {
            self.active = self
                .documents
                .last()
                .map(|doc| Pane::Document(doc.id))
                .unwrap_or(Pane::Welcome);
            self.focus_active_editor(window, cx);
        }
        cx.notify();
    }

    /// The watcher saw these paths change (`None`: anything may have).
    pub(super) fn large_check_disk(
        &mut self,
        paths: Option<&BTreeSet<PathBuf>>,
        cx: &mut Context<Self>,
    ) {
        if self
            .large
            .as_ref()
            .is_some_and(|large| paths.is_none_or(|paths| paths.contains(&large.path)))
        {
            self.large_index(cx);
        }
    }

    /// (Re)builds the line index; the held lines stay on screen until the new ones arrive.
    fn large_index(&mut self, cx: &mut Context<Self>) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        large.cancel.store(true, Ordering::Relaxed);
        large.cancel = Arc::default();
        large.generation += 1;
        large.reading = None;
        large._read_task = None;
        let (generation, cancel, path) =
            (large.generation, large.cancel.clone(), large.path.clone());
        let job = cx.background_spawn(async move { large_file::index(&path, &cancel) });
        large._index_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(large) = this.large.as_mut().filter(|l| l.generation == generation) else {
                    return;
                };
                match result {
                    Ok(index) => {
                        eprintln!(
                            "event=large_file_indexed lines={} bytes={}",
                            index.lines, index.bytes
                        );
                        large.index = Some(Arc::new(index));
                        large.error = None;
                        // Re-read what is on screen from the new index.
                        let first = large.held.0;
                        large.held = (first, Arc::default());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => large.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
    }

    /// Reads the lines around `visible` unless they are held or being read.
    fn large_read(&mut self, visible: std::ops::Range<usize>, cx: &mut Context<Self>) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        let Some(index) = large.index.clone() else {
            return;
        };
        let (first, held) = (large.held.0, large.held.1.len());
        let covered = |range: &std::ops::Range<usize>| {
            range.start <= visible.start && visible.end.min(index.lines) <= range.end
        };
        if covered(&(first..first + held)) || large.reading.as_ref().is_some_and(covered) {
            return;
        }
        let start = visible.start.saturating_sub(MARGIN);
        let range = start
            ..(visible.end + MARGIN)
                .min(index.lines)
                .min(start + large_file::MAX_READ_LINES);
        large.reading = Some(range.clone());
        let (generation, path) = (large.generation, large.path.clone());
        let read = range.clone();
        let job = cx.background_spawn(async move { large_file::read_lines(&path, &index, read) });
        large._read_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(large) = this.large.as_mut().filter(|l| l.generation == generation) else {
                    return;
                };
                large.reading = None;
                match result {
                    Ok(lines) => {
                        large.columns = lines
                            .iter()
                            .map(|l| l.text.chars().count() + 1)
                            .max()
                            .unwrap_or(0);
                        large.held = (range.start, Arc::new(lines));
                    }
                    Err(ReadError::Changed) => this.large_index(cx),
                    Err(ReadError::Io(error)) => large.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn large_selected(&self) -> Option<std::ops::Range<usize>> {
        let (anchor, head) = self.large.as_ref()?.selection?;
        Some(anchor.min(head)..anchor.max(head) + 1)
    }

    /// A click on a line selects it; with ⇧ the selection runs from where it started.
    pub(super) fn large_click(
        &mut self,
        line: usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        large.selection = Some(match (large.selection, extend) {
            (Some((anchor, _)), true) => (anchor, line),
            _ => (line, line),
        });
        large.focus.focus(window, cx);
        cx.notify();
    }

    /// Selects `line` (0-based) and scrolls it to the middle (⌃G, a search result).
    pub(super) fn large_go_to(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        let Some(lines) = large.index.as_ref().map(|index| index.lines) else {
            return;
        };
        let line = line.min(lines.saturating_sub(1));
        large.selection = Some((line, line));
        large.scroll.scroll_to_item(line, ScrollStrategy::Center);
        cx.notify();
    }

    pub(super) fn large_line_count(&self) -> Option<(usize, usize)> {
        let large = self.large.as_ref()?;
        let lines = large.index.as_ref()?.lines;
        Some((lines, large.selection.map_or(0, |(_, head)| head)))
    }

    /// ⌘C: the selected lines, whole, read in the background.
    pub(super) fn large_copy(&mut self, cx: &mut Context<Self>) {
        let Some(range) = self.large_selected() else {
            return;
        };
        let Some(large) = self.large.as_ref() else {
            return;
        };
        let Some(index) = large.index.clone() else {
            return;
        };
        let path = large.path.clone();
        let count = range.len();
        let job = cx.background_spawn(async move { large_file::read_text(&path, &index, range) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(text) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        this.message = format!("已复制 {count} 行");
                    }
                    Err(ReadError::Changed) => this.message = "文件已变化，请重新选择".into(),
                    Err(ReadError::Io(error)) => this.message = format!("复制失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⌘F in the viewer: the find bar, focused.
    pub(super) fn large_open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        if large.find.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder("在文件中查找"));
            let subscription = cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { shift, .. } = event {
                        this.large_find_step(!shift, cx);
                    }
                },
            );
            large.find = Some(LargeFind {
                input,
                status: String::new(),
                cancel: Arc::default(),
                _task: None,
                _subscription: subscription,
            });
        }
        if let Some(find) = &large.find {
            find.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        cx.notify();
    }

    fn large_close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(large) = self.large.as_mut() {
            if let Some(find) = large.find.take() {
                find.cancel.store(true, Ordering::Relaxed);
            }
            large.focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Enter / ⌘G (⇧ backwards): the next line with the query after the selected one.
    pub(super) fn large_find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(large) = self.large.as_mut() else {
            return;
        };
        let (Some(index), Some(find)) = (large.index.clone(), large.find.as_mut()) else {
            return;
        };
        let query = find.input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        find.cancel.store(true, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        find.cancel = cancel.clone();
        find.status = "正在查找…".into();
        let from = large.selection.map_or(0, |(_, head)| head);
        // From the very top, the first line counts too.
        let from = if large.selection.is_none() && forward {
            usize::MAX
        } else {
            from
        };
        let path = large.path.clone();
        let job = cx.background_spawn(async move {
            let from = if from == usize::MAX {
                index.lines.saturating_sub(1)
            } else {
                from
            };
            large_file::find(&path, &index, &query, from, forward, &cancel)
        });
        find._task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                let status = match result {
                    Ok(Some(line)) => {
                        this.large_go_to(line, cx);
                        format!("第 {} 行", line + 1)
                    }
                    Ok(None) => "没有找到".into(),
                    Err(ReadError::Changed) => {
                        this.large_index(cx);
                        "文件已变化，请再试一次".into()
                    }
                    Err(ReadError::Io(error))
                        if error.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        return;
                    }
                    Err(ReadError::Io(error)) => format!("查找失败：{error}"),
                };
                if let Some(find) = this.large.as_mut().and_then(|large| large.find.as_mut()) {
                    find.status = status;
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn large_status(&self) -> Option<String> {
        let index = self.large.as_ref()?.index.as_ref()?;
        Some(format!(
            "{} 行 · {:.1} MB · 只读",
            index.lines,
            index.bytes as f64 / 1_000_000.
        ))
    }

    pub(super) fn render_large(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(large) = &self.large else {
            return div().into_any_element();
        };
        let note = |text: String, color| {
            div()
                .p_4()
                .text_size(theme::TEXT_BODY)
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        let body = match (&large.error, &large.index) {
            (Some(error), _) => note(format!("无法显示：{error}"), colors.deleted),
            (None, None) => note("正在建立行索引…".into(), colors.muted),
            (None, Some(index)) => self.render_large_lines(index.lines, cx),
        };
        let path = large.path.clone();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .w_full()
                    .h(theme::BANNER_HEIGHT)
                    .px_3()
                    .gap_2()
                    .flex_shrink_0()
                    .bg(colors.banner)
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.foreground)
                    .child(Icon::new(IconName::Info).text_color(colors.muted))
                    .child(div().flex_1().min_w_0().child(format!(
                        "{}，以只读方式受限查看：不高亮、不折行、不能编辑，过长的行只显示开头。",
                        large.reason.reason()
                    )))
                    .child(
                        Button::new("large-open-external")
                            .xsmall()
                            .ghost()
                            .label("用默认程序打开")
                            .on_click(
                                cx.listener(move |_, _, _, cx| open_with_system(path.clone(), cx)),
                            ),
                    ),
            )
            .children(large.find.as_ref().map(|find| {
                h_flex()
                    .w_full()
                    .h(theme::BANNER_HEIGHT)
                    .px_3()
                    .gap_1()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(colors.border)
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .on_action(
                        cx.listener(|this, _: &Escape, window, cx| {
                            this.large_close_find(window, cx)
                        }),
                    )
                    .child(
                        div()
                            .w(theme::FIND_INPUT_WIDTH)
                            .child(Input::new(&find.input).small()),
                    )
                    .child(
                        Button::new("large-find-previous")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ArrowUp)
                            .tooltip("上一个 (⇧Enter)")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.large_find_step(false, cx)),
                            ),
                    )
                    .child(
                        Button::new("large-find-next")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ArrowDown)
                            .tooltip("下一个 (Enter)")
                            .on_click(cx.listener(|this, _, _, cx| this.large_find_step(true, cx))),
                    )
                    .child(div().flex_1().min_w_0().child(find.status.clone()))
                    .child(
                        Button::new("large-find-close")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.large_close_find(window, cx)
                                }),
                            ),
                    )
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .key_context(CONTEXT)
                    .track_focus(&large.focus)
                    .on_action(cx.listener(|this, _: &CopyLargeLines, _, cx| this.large_copy(cx)))
                    .child(body),
            )
            .into_any_element()
    }

    fn render_large_lines(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(large) = &self.large else {
            return div().into_any_element();
        };
        let font = Theme::global(cx).mono_font_family.clone();
        let metrics = theme::diff_metrics(Theme::global(cx).mono_font_size);
        let digits = count.to_string().len() as f32;
        let gutter = metrics.column * (digits + 2.);
        let width = gutter + metrics.column * large.columns as f32 + theme::DIFF_TEXT_END;
        let list = uniform_list(
            "large-file-lines",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                // The list first lays out row 0 alone to measure a row; reading for that
                // would replace the lines on screen every frame.
                if range.len() > 1 || range.end == count {
                    this.large_read(range.clone(), cx);
                }
                let Some(large) = &this.large else {
                    return Vec::new();
                };
                let (first, held) = (large.held.0, large.held.1.clone());
                let selected = this.large_selected();
                range
                    .map(|index| {
                        let line = index.checked_sub(first).and_then(|i| held.get(i));
                        h_flex()
                            .id(("large-line", index))
                            .when(
                                selected.as_ref().is_some_and(|s| s.contains(&index)),
                                |row| row.bg(colors.selection),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                    this.large_click(index, event.modifiers.shift, window, cx)
                                }),
                            )
                            .h(metrics.row)
                            .w(width)
                            .min_w_full()
                            .flex_shrink_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family(font.clone())
                            .text_size(metrics.text)
                            .line_height(metrics.row)
                            .child(
                                div()
                                    .w(gutter)
                                    .flex_shrink_0()
                                    .pr(metrics.column)
                                    .text_right()
                                    .text_color(colors.muted)
                                    .child((index + 1).to_string()),
                            )
                            .when_some(line, |row, line| {
                                row.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(colors.code)
                                        .child(line.text.clone()),
                                )
                                .when(line.cut, |row| {
                                    row.child(div().text_color(colors.muted).child("…"))
                                })
                            })
                            .into_any_element()
                    })
                    .collect()
            }),
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&large.scroll)
        .size_full();
        div()
            .relative()
            .size_full()
            .bg(colors.editor)
            .child(list)
            .child(
                div().absolute().inset_0().child(
                    Scrollbar::new(&large.scroll)
                        .id("large-file-scrollbar")
                        .viewport_from_layout(),
                ),
            )
            .into_any_element()
    }
}

/// Opens `path` in the program the system associates with it.
fn open_with_system(path: PathBuf, cx: &mut App) {
    cx.background_spawn(async move {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        if let Err(error) = std::process::Command::new(program)
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            eprintln!("event=open_external_failed error={error}");
        }
    })
    .detach();
}

#[cfg(test)]
#[path = "large_ui_tests.rs"]
mod ui_tests;
