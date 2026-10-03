//! Markdown live preview, Typora-style. A Markdown file opens rendered, one Kit `TextView` per
//! top-level block (`markdown_blocks.rs`) in a virtualized list. Clicking a block swaps it for
//! its source in a text area; every keystroke goes straight into the document's buffer, so
//! dirty state, saving and undo in the source view work as for any edit. Esc or clicking
//! elsewhere renders it again. Clicking below the last block starts a new one. The tab bar's
//! button (⇧⌘V, as in VS Code) switches between the preview and the source editor.

use super::{Pane, Prototype};
use crate::{
    markdown_blocks::{self, Kind},
    replace, theme,
};
use gpui_kit::{
    assets::IconName,
    base::TestSupportExt,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{Escape, InputEvent, RopeExt, Textarea, TextareaState},
        scroll::Scrollbar,
        text::TextView,
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::ops::Range;
use workspace_editor_core::DocumentId;

gpui_kit::actions!(markdown, [ToggleMarkdownPreview]);

struct Shown {
    range: Range<usize>,
    kind: Kind,
    /// What the block renders (`markdown_blocks::rendered`).
    text: SharedString,
}

struct Editing {
    /// The block being edited; `None` for a new block after the last one.
    block: Option<usize>,
    start: usize,
    /// Bytes the edit covers in the buffer now.
    len: usize,
    /// Goes before the typed text of a new block, so it does not join the last one.
    separator: &'static str,
    editor: Entity<TextareaState>,
    _subscription: Subscription,
}

pub(super) struct MarkdownPreview {
    /// Showing the source editor instead.
    pub(super) source: bool,
    blocks: Vec<Shown>,
    /// The document version `blocks` were split from.
    parsed: Option<u64>,
    /// One item per block plus the room after the last one.
    list: ListState,
    focus: FocusHandle,
    editing: Option<Editing>,
    task: Option<Task<()>>,
}

impl MarkdownPreview {
    /// A preview for a Markdown document, starting in the preview unless `source`.
    pub(super) fn for_language(language: &str, source: bool, cx: &mut App) -> Option<Self> {
        (language == "markdown").then(|| Self {
            source,
            blocks: Vec::new(),
            parsed: None,
            list: ListState::new(1, ListAlignment::Top, theme::MD_OVERDRAW),
            focus: cx.focus_handle(),
            editing: None,
            task: None,
        })
    }

    /// Takes a fresh split, telling the list only about the blocks that changed so the scroll
    /// position and measured heights of the rest survive.
    fn replace_blocks(&mut self, blocks: Vec<Shown>) {
        let old = &self.blocks;
        if old.is_empty() {
            // A splice in front of the room after the last block would keep the view on it.
            self.list.reset(blocks.len() + 1);
            self.blocks = blocks;
            return;
        }
        let same = |a: &Shown, b: &Shown| a.text == b.text;
        let prefix = old
            .iter()
            .zip(&blocks)
            .take_while(|(a, b)| same(a, b))
            .count();
        let room = old.len().min(blocks.len()) - prefix;
        let suffix = old
            .iter()
            .rev()
            .zip(blocks.iter().rev())
            .take(room)
            .take_while(|(a, b)| same(a, b))
            .count();
        self.list
            .splice(prefix..old.len() - suffix, blocks.len() - prefix - suffix);
        self.blocks = blocks;
    }

    #[cfg(test)]
    pub(super) fn block_texts(&self) -> Vec<String> {
        self.blocks
            .iter()
            .map(|block| block.text.to_string())
            .collect()
    }

    #[cfg(test)]
    pub(super) fn editing_block(&self) -> Option<Option<usize>> {
        self.editing.as_ref().map(|editing| editing.block)
    }

    /// The buffer text an edit puts in: CRLF files keep their line endings.
    fn buffer_text(editing: &Editing, eol: &str, cx: &App) -> String {
        let typed = editing.editor.read(cx).value();
        let typed = if eol == "\n" {
            typed.to_string()
        } else {
            typed.replace('\n', eol)
        };
        if editing.block.is_none() && !typed.is_empty() {
            format!("{}{typed}", editing.separator)
        } else {
            typed
        }
    }
}

impl Prototype {
    fn markdown(&self, id: DocumentId) -> Option<&MarkdownPreview> {
        self.document(id)?.markdown.as_ref()
    }

    fn markdown_mut(&mut self, id: DocumentId) -> Option<&mut MarkdownPreview> {
        self.document_mut(id)?.markdown.as_mut()
    }

    /// The active document when it shows the rendered preview.
    pub(super) fn active_preview(&self) -> Option<(DocumentId, &MarkdownPreview)> {
        let Pane::Document(id) = self.active else {
            return None;
        };
        self.markdown(id).filter(|md| !md.source).map(|md| (id, md))
    }

    /// Focuses the block being edited, else the preview itself.
    pub(super) fn focus_preview(&self, window: &mut Window, cx: &mut App) -> bool {
        let Some((_, md)) = self.active_preview() else {
            return false;
        };
        match &md.editing {
            Some(editing) => editing
                .editor
                .update(cx, |state, cx| state.focus(window, cx)),
            None => md.focus.focus(window, cx),
        }
        true
    }

    /// Splits the document again in the background unless the preview is hidden, a block is
    /// being edited, or the split is current.
    pub(super) fn markdown_refresh(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(md) = &doc.markdown else {
            return;
        };
        if md.source || md.editing.is_some() || md.parsed == Some(doc.version) {
            return;
        }
        let text = doc.editor.read(cx).text().to_string();
        let version = doc.version;
        let work = cx.background_spawn(async move {
            let split = markdown_blocks::split(&text);
            split
                .blocks
                .iter()
                .map(|block| Shown {
                    range: block.range.clone(),
                    kind: block.kind,
                    text: markdown_blocks::rendered(&text, block, &split.definitions).into(),
                })
                .collect::<Vec<_>>()
        });
        let task = cx.spawn(async move |this, cx| {
            let blocks = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(doc) = this.document_mut(id) else {
                    return;
                };
                if doc.version != version {
                    eprintln!("event=markdown_refresh_stale id={id:?}");
                    return;
                }
                let Some(md) = doc.markdown.as_mut() else {
                    return;
                };
                if md.editing.is_some() {
                    return;
                }
                md.replace_blocks(blocks);
                md.parsed = Some(version);
                md.task = None;
                cx.notify();
            });
        });
        if let Some(md) = self.markdown_mut(id) {
            md.task = Some(task);
        }
    }

    /// After any change to the buffer: a block edit's own keystroke is left alone; anything
    /// else (the source view, a reload, an agent) ends the edit, since its range is no longer
    /// known, and splits again.
    pub(super) fn markdown_buffer_changed(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(md) = &doc.markdown else {
            return;
        };
        if let Some(editing) = &md.editing {
            let text = doc.editor.read(cx).text().to_string();
            let expected = MarkdownPreview::buffer_text(editing, replace::eol_of(&text), cx);
            if text.get(editing.start..editing.start + editing.len) == Some(expected.as_str()) {
                return;
            }
            if let Some(md) = self.markdown_mut(id) {
                md.editing = None;
                md.parsed = None;
            }
        }
        self.markdown_refresh(id, cx);
    }

    /// Clicking a block (by where it starts, which survives a split arriving in between) or,
    /// with `None`, the room after the last block.
    fn markdown_edit(
        &mut self,
        id: DocumentId,
        start: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.markdown_finish(id, false, window, cx);
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(md) = &doc.markdown else {
            return;
        };
        let text = doc.editor.read(cx).text().to_string();
        let (block, range, separator) = match start {
            Some(start) => {
                let Some(index) = md.blocks.iter().position(|b| b.range.start == start) else {
                    return;
                };
                let Some(range) = md.blocks.get(index).map(|b| b.range.clone()) else {
                    return;
                };
                (Some(index), range, "")
            }
            None => (
                None,
                text.len()..text.len(),
                markdown_blocks::separator_at_end(&text),
            ),
        };
        let Some(source) = text.get(range.clone()) else {
            return;
        };
        let source = source.replace("\r\n", "\n");
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, theme::MD_EDIT_MAX_ROWS)
                .default_value(source)
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            move |this: &mut Self, editor, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.markdown_typed(id, window, cx),
                // Only the current edit's text area: the previous one blurs after a click
                // already moved on to the next block.
                InputEvent::Blur => {
                    let current = this
                        .markdown(id)
                        .and_then(|md| md.editing.as_ref())
                        .is_some_and(|editing| editing.editor == *editor);
                    if current {
                        this.markdown_finish(id, false, window, cx);
                    }
                }
                _ => {}
            },
        );
        editor.update(cx, |state, cx| {
            state.focus(window, cx);
            let end = state.text().offset_to_position(state.text().len());
            state.set_cursor_position(end, window, cx);
        });
        if let Some(md) = self.markdown_mut(id) {
            md.editing = Some(Editing {
                block,
                start: range.start,
                len: range.len(),
                separator,
                editor,
                _subscription: subscription,
            });
        }
        cx.notify();
    }

    /// Writes the edited block into the buffer.
    fn markdown_typed(&mut self, id: DocumentId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(editing) = doc.markdown.as_ref().and_then(|md| md.editing.as_ref()) else {
            return;
        };
        let buffer = doc.editor.clone();
        let (start, end) = (editing.start, editing.start + editing.len);
        let mut written = None;
        buffer.update(cx, |state, cx| {
            let text = state.text().to_string();
            let new = MarkdownPreview::buffer_text(editing, replace::eol_of(&text), cx);
            if text.get(start..end).is_none_or(|old| old == new) {
                return;
            }
            let utf16 = replace::utf16_offset(&text, start)..replace::utf16_offset(&text, end);
            state.replace_text_in_range(Some(utf16), &new, window, cx);
            written = Some(new.len());
        });
        if let Some(len) = written
            && let Some(editing) = self.markdown_mut(id).and_then(|md| md.editing.as_mut())
        {
            editing.len = len;
        }
    }

    /// Renders the edited block again: its new text shows at once, the other blocks move by
    /// the size change, and a fresh split follows.
    pub(super) fn markdown_finish(
        &mut self,
        id: DocumentId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editing) = self.markdown_mut(id).and_then(|md| md.editing.take()) else {
            return;
        };
        let typed: SharedString = editing.editor.read(cx).value();
        let Some(md) = self.markdown_mut(id) else {
            return;
        };
        let start = editing.start;
        match editing.block {
            Some(index) if index < md.blocks.len() => {
                let old = md.blocks[index].range.len();
                for block in &mut md.blocks[index + 1..] {
                    block.range =
                        block.range.start + editing.len - old..block.range.end + editing.len - old;
                }
                md.blocks[index] = Shown {
                    range: start..start + editing.len,
                    kind: Kind::Markdown,
                    text: typed,
                };
                md.list.remeasure_items(index..index + 1);
            }
            None if editing.len > 0 => {
                let count = md.blocks.len();
                md.blocks.push(Shown {
                    range: start + editing.separator.len()..start + editing.len,
                    kind: Kind::Markdown,
                    text: typed,
                });
                md.list.splice(count..count, 1);
            }
            _ => {}
        }
        md.parsed = None;
        if focus {
            md.focus.focus(window, cx);
        }
        self.markdown_refresh(id, cx);
        cx.notify();
    }

    /// ⇧⌘V and the tab bar button.
    pub(super) fn toggle_markdown_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Document(id) = self.active else {
            return;
        };
        self.markdown_finish(id, false, window, cx);
        let Some(md) = self.markdown_mut(id) else {
            return;
        };
        md.source = !md.source;
        self.markdown_refresh(id, cx);
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    /// Jumps to a line and find work on the source. Returns whether it switched.
    pub(super) fn markdown_show_source(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.markdown(id).is_some_and(|md| !md.source) {
            return false;
        }
        self.markdown_finish(id, false, window, cx);
        if let Some(md) = self.markdown_mut(id) {
            md.source = true;
        }
        cx.notify();
        true
    }

    pub(super) fn markdown_path_changed(&mut self, id: DocumentId, language: &str, cx: &mut App) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        if doc.markdown.is_some() != (language == "markdown") {
            // A buffer that becomes Markdown by Save As keeps showing its source.
            doc.markdown = MarkdownPreview::for_language(language, true, cx);
        }
    }

    pub(super) fn render_markdown_toggle(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Pane::Document(id) = self.active else {
            return None;
        };
        let md = self.markdown(id)?;
        let colors = theme::colors(cx);
        let (icon, label) = if md.source {
            (IconName::BookOpen, "显示预览 (⇧⌘V)")
        } else {
            (IconName::Code, "显示源码 (⇧⌘V)")
        };
        Some(
            h_flex()
                .h_full()
                .flex_shrink_0()
                .px_2()
                .bg(colors.tabs)
                .child(
                    Button::new("markdown-toggle")
                        .ghost()
                        .small()
                        .icon(icon)
                        .tooltip(label)
                        .accessibility_label(label)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_markdown_preview(window, cx)
                        })),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_markdown_preview(
        &self,
        id: DocumentId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(md) = self.markdown(id) else {
            return div().into_any_element();
        };
        let font = px(cx.global::<crate::settings::Settings>().editor_font_size);
        let view = cx.weak_entity();
        let items = list(md.list.clone(), move |index, _, cx| {
            let Some(view) = view.upgrade() else {
                return div().into_any_element();
            };
            let colors = theme::colors(cx);
            let Some(md) = view.read(cx).markdown(id) else {
                return div().into_any_element();
            };
            let editing = md.editing.as_ref();
            let editor = |editing: &Editing| {
                div()
                    .w_full()
                    .p(theme::MD_EDIT_PAD)
                    .rounded(theme::RADIUS)
                    .bg(colors.hover)
                    .font_family(
                        gpui_kit::component::Theme::global(cx)
                            .mono_font_family
                            .clone(),
                    )
                    .on_action({
                        let view = view.clone();
                        move |_: &Escape, window, cx| {
                            view.update(cx, |this, cx| this.markdown_finish(id, true, window, cx))
                        }
                    })
                    .child(
                        Textarea::new(&editing.editor)
                            .appearance(false)
                            .bordered(false),
                    )
            };
            let column = |content: AnyElement| {
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .px(theme::MD_PAD_X)
                    .child(
                        div()
                            .w_full()
                            .max_w(theme::MD_MAX_WIDTH)
                            .py(theme::MD_BLOCK_GAP)
                            .child(content),
                    )
                    .into_any_element()
            };
            let Some(block) = md.blocks.get(index) else {
                // The room after the last block: a click starts a new one.
                let new = editing.filter(|editing| editing.block.is_none());
                let empty = md.blocks.is_empty() && new.is_none();
                return v_flex()
                    .w_full()
                    .children(new.map(|editing| column(editor(editing).into_any_element())))
                    .child(
                        div()
                            .id("md-tail")
                            .w_full()
                            .h(theme::MD_TAIL)
                            .cursor_text()
                            .when(empty, |tail| {
                                tail.flex()
                                    .justify_center()
                                    .pt(theme::MD_BLOCK_GAP)
                                    .text_color(colors.muted)
                                    .child("点击这里开始输入")
                            })
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |this, cx| {
                                        this.markdown_edit(id, None, window, cx)
                                    })
                                }
                            })
                            .test_support(),
                    )
                    .into_any_element();
            };
            if let Some(editing) = editing.filter(|editing| editing.block == Some(index)) {
                return column(editor(editing).into_any_element());
            }
            let content = match block.kind {
                Kind::Definition => div()
                    .text_color(colors.muted)
                    .child(block.text.clone())
                    .into_any_element(),
                Kind::Markdown | Kind::Frontmatter(_) => {
                    TextView::markdown(("md-text", index), block.text.clone())
                        .selectable(false)
                        .on_link_click(|url, _, _, cx| {
                            cx.open_url(url);
                            cx.stop_propagation();
                        })
                        .into_any_element()
                }
            };
            let start = block.range.start;
            column(
                div()
                    .id(("md-block", index))
                    .w_full()
                    .cursor_text()
                    .child(content)
                    .on_click({
                        let view = view.clone();
                        move |_, window, cx| {
                            view.update(cx, |this, cx| {
                                this.markdown_edit(id, Some(start), window, cx)
                            })
                        }
                    })
                    .test_support()
                    .into_any_element(),
            )
        })
        .size_full();
        div()
            .relative()
            .size_full()
            .track_focus(&md.focus)
            .text_size(font)
            .child(items)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::new(&md.list).id("md-scrollbar")),
            )
            .into_any_element()
    }
}
