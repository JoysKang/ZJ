//! The editor's find / replace widget, laid out like VS Code's: it floats at the top right
//! of the editor with a shadow.
//! - Left: a chevron that shows or hides the replace row.
//! - Find row: the find input with Aa / ab / .* inside it, "第 N 项，共 M 项", ↑ ↓,
//!   在选区中查找 and ×.
//! - Replace row: the replace input with AB (保留大小写), then 替换 and 全部替换.
//!
//! ⌘F / ⌥⌘F open it, Enter / ⇧Enter step through the matches, ⌘⇧1 and ⌘⌥Enter replace,
//! and Esc closes it. All matches are highlighted in the editor, and the current one more
//! strongly. Matching and replacement rules are in [`crate::replace`].

use super::{Pane, Prototype};
use crate::replace::{self, Finder, Query, Replacement};
use crate::theme;
use gpui_kit::{
    assets::IconName,
    component::{
        Disableable, Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{
            EditorState, Enter, Escape, Input, InputEvent, InputState, RangeDecoration,
            RangeDecorationCollection, RangeDecorationStyle,
        },
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::ops::Range;
use workspace_editor_core::DocumentId;

gpui_kit::actions!(
    find,
    [
        FindInFile,
        FindReplace,
        FindNext,
        FindPrevious,
        ReplaceOne,
        ReplaceAll,
        ToggleFindCase,
        ToggleFindWord,
        ToggleFindRegex,
        TogglePreserveCase,
        ToggleFindInSelection
    ]
);

/// VS Code stops counting here and shows "19999+".
const MAX_MATCHES: usize = 19_999;

pub(super) struct FindState {
    pub open: bool,
    replace_open: bool,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    preserve_case: bool,
    /// 在选区中查找: matches are limited to this byte range of the document.
    scope: Option<Range<usize>>,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
    error: Option<String>,
    /// After a replacement, the next match at or after this offset becomes current.
    resume_at: Option<usize>,
    /// The document the matches belong to, and its highlight collection.
    target: Option<(DocumentId, RangeDecorationCollection)>,
    _subscriptions: Vec<Subscription>,
}

impl FindState {
    pub fn new(window: &mut Window, cx: &mut Context<Prototype>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("查找"));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("替换"));
        let subscriptions =
            vec![
                cx.subscribe(&query, |this: &mut Prototype, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.find.current = None;
                        this.find_update(true, cx);
                    }
                }),
            ];
        Self {
            open: false,
            replace_open: false,
            query,
            replacement,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            preserve_case: false,
            scope: None,
            matches: Vec::new(),
            current: None,
            error: None,
            resume_at: None,
            target: None,
            _subscriptions: subscriptions,
        }
    }

    fn query(&self, cx: &App) -> Query {
        Query {
            pattern: self.query.read(cx).value().to_string(),
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    fn label(&self) -> String {
        if self.error.is_some() {
            return "正则无效".into();
        }
        if self.matches.is_empty() {
            return "无结果".into();
        }
        let total = if self.matches.len() >= MAX_MATCHES {
            format!("{MAX_MATCHES}+")
        } else {
            self.matches.len().to_string()
        };
        match self.current {
            Some(index) => format!("第 {} 项，共 {total} 项", index + 1),
            None => format!("第 ? 项，共 {total} 项"),
        }
    }
}

impl Prototype {
    fn find_document(&self) -> Option<(DocumentId, Entity<EditorState>, bool)> {
        let Pane::Document(id) = self.active else {
            return None;
        };
        self.documents
            .iter()
            .find(|doc| doc.id == id)
            .map(|doc| (doc.id, doc.editor.clone(), doc.readonly))
    }

    /// ⌘F (`replace` false) / ⌥⌘F: open the widget, seeded with the selection if it is one
    /// line, and focus the find input.
    pub(super) fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, editor, readonly)) = self.find_document() else {
            return;
        };
        let selected = editor.read(cx).selected_text().to_string();
        if !selected.is_empty() && !selected.contains('\n') {
            self.find
                .query
                .update(cx, |input, cx| input.set_value(selected, window, cx));
            self.find.current = None;
        }
        if replace && !readonly {
            self.find.replace_open = true;
        }
        self.find.open = true;
        let focus = if replace && !self.find.query.read(cx).value().is_empty() {
            &self.find.replacement
        } else {
            &self.find.query
        };
        focus.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.find_update(true, cx);
    }

    pub(super) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.scope = None;
        self.clear_find_highlights(cx);
        self.focus_active_editor(window, cx);
        cx.notify();
    }

    fn clear_find_highlights(&mut self, cx: &mut App) {
        if let Some((_, collection)) = self.find.target.take() {
            collection.dispose(cx);
        }
    }

    /// Recomputes the matches in the active document (after typing in either the query or
    /// the document, a toggle, or switching tabs) and repaints the highlights. With `reveal`,
    /// a new current match is selected in the editor.
    pub(super) fn find_update(&mut self, reveal: bool, cx: &mut Context<Self>) {
        if !self.find.open {
            return;
        }
        let Some((id, editor, _)) = self.find_document() else {
            self.clear_find_highlights(cx);
            self.find.matches.clear();
            self.find.current = None;
            cx.notify();
            return;
        };
        if self
            .find
            .target
            .as_ref()
            .is_some_and(|(target, _)| *target != id)
        {
            self.clear_find_highlights(cx);
            self.find.scope = None;
            self.find.current = None;
        }
        let text = editor.read(cx).text().to_string();
        let query = self.find.query(cx);
        self.find.error = None;
        let matches = match Finder::new(&query) {
            Ok(finder) => match &self.find.scope {
                Some(scope) => finder.find_in(&text, scope, MAX_MATCHES),
                None => finder.find_all(&text, MAX_MATCHES),
            },
            Err(error) => {
                if !error.is_empty() {
                    self.find.error = Some(error);
                }
                Vec::new()
            }
        };
        let cursor = editor.read(cx).selected_range();
        let previous = self
            .find
            .current
            .and_then(|i| self.find.matches.get(i).cloned());
        self.find.matches = matches;
        // Keep the current match where it was; otherwise take the first one at the cursor.
        let resume = self.find.resume_at.take();
        let mut moved = false;
        self.find.current = match (resume, previous) {
            (Some(offset), _) => {
                moved = true;
                self.find
                    .matches
                    .iter()
                    .position(|m| m.start >= offset)
                    .or((!self.find.matches.is_empty()).then_some(0))
            }
            (None, Some(previous)) => self.find.matches.iter().position(|m| *m == previous),
            (None, None) => {
                moved = true;
                self.find
                    .matches
                    .iter()
                    .position(|m| m.end > cursor.start)
                    .or((!self.find.matches.is_empty()).then_some(0))
            }
        };
        if reveal
            && moved
            && let Some(range) = self.find.current.and_then(|i| self.find.matches.get(i))
        {
            let range = range.clone();
            editor.update(cx, |state, cx| state.set_selected_range(range, cx));
        }
        self.paint_find_highlights(id, &editor, cx);
        cx.notify();
    }

    fn paint_find_highlights(
        &mut self,
        id: DocumentId,
        editor: &Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        let colors = theme::colors(cx);
        let mut decorations = Vec::with_capacity(self.find.matches.len() + 3);
        if let Some(scope) = &self.find.scope {
            decorations.push(
                RangeDecoration::new(scope.clone())
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(colors.selection),
            );
        }
        for (index, range) in self.find.matches.iter().enumerate() {
            let color = if Some(index) == self.find.current {
                colors.find_current
            } else {
                colors.find_match
            };
            decorations.push(
                RangeDecoration::new(range.clone())
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color),
            );
        }
        match &self.find.target {
            Some((_, collection)) => collection.set(decorations, cx),
            None => {
                let collection = editor.update(cx, |state, cx| {
                    state.create_range_decorations_collection(decorations, cx)
                });
                self.find.target = Some((id, collection));
            }
        }
    }

    /// Enter / ⇧Enter, ↑ ↓, ⌘G / ⌘⇧G.
    pub(super) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some((id, editor, _)) = self.find_document() else {
            return;
        };
        let count = self.find.matches.len();
        if count == 0 {
            return;
        }
        let cursor = editor.read(cx).selected_range();
        let next = match self.find.current {
            Some(i) if forward => (i + 1) % count,
            Some(i) => (i + count - 1) % count,
            None if forward => self
                .find
                .matches
                .iter()
                .position(|m| m.start >= cursor.end)
                .unwrap_or(0),
            None => self
                .find
                .matches
                .iter()
                .rposition(|m| m.end <= cursor.start)
                .unwrap_or(count - 1),
        };
        self.find.current = Some(next);
        let range = self.find.matches[next].clone();
        editor.update(cx, |state, cx| state.set_selected_range(range, cx));
        self.paint_find_highlights(id, &editor, cx);
        cx.notify();
    }

    fn find_replacement(&self, cx: &App) -> Replacement {
        Replacement::new(
            self.find.replacement.read(cx).value().to_string(),
            self.find.regex,
            self.find.preserve_case,
        )
    }

    /// 替换 (⌘⇧1, or Enter in the replace input): replaces the current match and moves on.
    pub(super) fn find_replace_one(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, editor, readonly)) = self.find_document() else {
            return;
        };
        if readonly {
            return;
        }
        let Some(range) = self
            .find
            .current
            .and_then(|i| self.find.matches.get(i))
            .cloned()
        else {
            self.find_step(true, cx);
            return;
        };
        let Ok(finder) = Finder::new(&self.find.query(cx)) else {
            return;
        };
        let replacement = self.find_replacement(cx);
        editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let replacement = replacement.with_eol(replace::eol_of(&text));
            let Some(new) = finder.replacement_for(&text, &range, &replacement) else {
                return;
            };
            let utf16 =
                replace::utf16_offset(&text, range.start)..replace::utf16_offset(&text, range.end);
            self.find.resume_at = Some(range.start + new.len());
            state.replace_text_in_range(Some(utf16), &new, window, cx);
        });
        // The edit's change event recomputes the matches; make sure it happens even when
        // the replacement equals the match.
        self.find.current = None;
        self.find_update(true, cx);
    }

    /// 全部替换 (⌘⌥Enter): one edit, so a single undo restores everything.
    pub(super) fn find_replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, editor, readonly)) = self.find_document() else {
            return;
        };
        if readonly || self.find.matches.is_empty() {
            return;
        }
        let Ok(finder) = Finder::new(&self.find.query(cx)) else {
            return;
        };
        let replacement = self.find_replacement(cx);
        let only = self.find.matches.clone();
        let mut replaced = 0;
        editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let replacement = replacement.with_eol(replace::eol_of(&text));
            let (start, end) = (only[0].start, only[only.len() - 1].end);
            let (new, count) = finder.replace(&text, Some(&only), &replacement);
            replaced = count;
            if count == 0 {
                return;
            }
            // Only the span from the first to the last match changes.
            let middle = &new[start..new.len() - (text.len() - end)];
            let utf16 = replace::utf16_offset(&text, start)..replace::utf16_offset(&text, end);
            state.replace_text_in_range(Some(utf16), middle, window, cx);
        });
        self.find.current = None;
        self.find_update(false, cx);
        if replaced > 0 {
            self.message = format!("已替换 {replaced} 处");
        }
        cx.notify();
    }

    fn toggle_find(&mut self, which: fn(&mut FindState) -> &mut bool, cx: &mut Context<Self>) {
        let flag = which(&mut self.find);
        *flag = !*flag;
        self.find.current = None;
        self.find_update(true, cx);
    }

    /// 在选区中查找: limit the search to the current selection (or turn it off).
    pub(super) fn toggle_find_in_selection(&mut self, cx: &mut Context<Self>) {
        if self.find.scope.take().is_none()
            && let Some((_, editor, _)) = self.find_document()
        {
            let selection = editor.read(cx).selected_range();
            if selection.start < selection.end {
                self.find.scope = Some(selection);
            }
        }
        self.find.current = None;
        self.find_update(false, cx);
    }

    fn find_toggle(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        on: bool,
        which: fn(&mut FindState) -> &mut bool,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .with_size(theme::FIND_TOGGLE)
            .ghost()
            .icon(icon)
            .selected(on)
            .tooltip(label)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_find(which, cx)))
    }

    fn find_button(&self, id: &'static str, icon: IconName, label: &'static str) -> Button {
        Button::new(id)
            .with_size(theme::FIND_BUTTON)
            .ghost()
            .icon(icon)
            .tooltip(label)
            .accessibility_label(label)
    }

    pub(super) fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = &self.find;
        let (_, _, readonly) = self.find_document()?;
        if !find.open {
            return None;
        }
        let colors = theme::colors(cx);
        let has_matches = !find.matches.is_empty();
        let replace_open = find.replace_open && !readonly;
        let toggles = h_flex()
            .gap_0p5()
            .child(self.find_toggle(
                "find-case",
                IconName::CaseSensitive,
                "区分大小写 (⌥⌘C)",
                find.case_sensitive,
                |f| &mut f.case_sensitive,
                cx,
            ))
            .child(self.find_toggle(
                "find-word",
                IconName::WholeWord,
                "全字匹配 (⌥⌘W)",
                find.whole_word,
                |f| &mut f.whole_word,
                cx,
            ))
            .child(self.find_toggle(
                "find-regex",
                IconName::Regex,
                "使用正则表达式 (⌥⌘R)",
                find.regex,
                |f| &mut f.regex,
                cx,
            ));
        let find_row = h_flex()
            .gap_1()
            .child(
                div().w(theme::FIND_INPUT_WIDTH).child(
                    Input::new(&find.query)
                        .small()
                        .suffix(toggles)
                        .when(find.error.is_some(), |input| {
                            input.border_color(colors.deleted)
                        }),
                ),
            )
            .child(
                div()
                    .id("find-count")
                    .min_w(theme::FIND_COUNT_WIDTH)
                    .flex_shrink_0()
                    .pl_1()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(if find.error.is_some() {
                        colors.deleted
                    } else if has_matches {
                        colors.foreground
                    } else {
                        colors.muted
                    })
                    .when_some(find.error.clone(), |label, error| {
                        label.tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(error.clone())
                                .build(window, cx)
                        })
                    })
                    .child(find.label()),
            )
            .child(
                self.find_button("find-previous", IconName::ArrowUp, "上一个匹配项 (⇧Enter)")
                    .disabled(!has_matches)
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(false, cx))),
            )
            .child(
                self.find_button("find-next", IconName::ArrowDown, "下一个匹配项 (Enter)")
                    .disabled(!has_matches)
                    .on_click(cx.listener(|this, _, _, cx| this.find_step(true, cx))),
            )
            .child(
                self.find_button(
                    "find-selection",
                    IconName::TextAlignStart,
                    "在选区中查找 (⌥⌘L)",
                )
                .selected(find.scope.is_some())
                .on_click(cx.listener(|this, _, _, cx| this.toggle_find_in_selection(cx))),
            )
            .child(
                self.find_button("find-close", IconName::Close, "关闭 (Esc)")
                    .on_click(cx.listener(|this, _, window, cx| this.close_find(window, cx))),
            );
        let replace_row = h_flex()
            .gap_1()
            .child(
                div().w(theme::FIND_INPUT_WIDTH).child(
                    Input::new(&find.replacement)
                        .small()
                        .suffix(self.find_toggle(
                            "find-preserve-case",
                            IconName::CaseUpper,
                            "保留大小写 (⌥⌘P)",
                            find.preserve_case,
                            |f| &mut f.preserve_case,
                            cx,
                        )),
                ),
            )
            .child(
                self.find_button("replace-one", IconName::Replace, "替换 (⌘⇧1)")
                    .disabled(!has_matches)
                    .on_click(cx.listener(|this, _, window, cx| this.find_replace_one(window, cx))),
            )
            .child(
                self.find_button("replace-all", IconName::ReplaceAll, "全部替换 (⌘⌥Enter)")
                    .disabled(!has_matches)
                    .on_click(cx.listener(|this, _, window, cx| this.find_replace_all(window, cx))),
            );
        let chevron = Button::new("find-toggle-replace")
            .with_size(theme::FIND_BUTTON)
            .ghost()
            .icon(if replace_open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .tooltip("切换替换")
            .disabled(readonly)
            .on_click(cx.listener(|this, _, window, cx| {
                this.find.replace_open = !this.find.replace_open;
                let input = if this.find.replace_open {
                    &this.find.replacement
                } else {
                    &this.find.query
                };
                input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            }));
        Some(
            h_flex()
                .id("find-widget")
                .key_context("FindWidget")
                .occlude()
                .absolute()
                .top_0()
                .right(theme::FIND_RIGHT)
                .min_w(theme::FIND_WIDTH)
                .p_1()
                .gap_1()
                .items_start()
                .bg(colors.panel)
                .border_1()
                .border_t_0()
                .border_color(colors.border)
                .rounded_b(theme::RADIUS)
                .shadow(vec![BoxShadow {
                    color: colors.shadow,
                    offset: point(px(0.), theme::FIND_SHADOW_Y),
                    blur_radius: theme::FIND_SHADOW_BLUR,
                    spread_radius: px(0.),
                    inset: false,
                }])
                .text_color(colors.foreground)
                .on_action(cx.listener(|this, action: &Enter, window, cx| {
                    let in_replace = this
                        .find
                        .replacement
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window);
                    if in_replace {
                        this.find_replace_one(window, cx);
                    } else {
                        this.find_step(!action.shift, cx);
                    }
                }))
                .on_action(cx.listener(|this, _: &Escape, window, cx| this.close_find(window, cx)))
                .on_action(
                    cx.listener(|this, _: &ReplaceOne, window, cx| {
                        this.find_replace_one(window, cx)
                    }),
                )
                .on_action(
                    cx.listener(|this, _: &ReplaceAll, window, cx| {
                        this.find_replace_all(window, cx)
                    }),
                )
                .on_action(cx.listener(|this, _: &ToggleFindCase, _, cx| {
                    this.toggle_find(|f| &mut f.case_sensitive, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleFindWord, _, cx| {
                    this.toggle_find(|f| &mut f.whole_word, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleFindRegex, _, cx| {
                    this.toggle_find(|f| &mut f.regex, cx)
                }))
                .on_action(cx.listener(|this, _: &TogglePreserveCase, _, cx| {
                    this.toggle_find(|f| &mut f.preserve_case, cx)
                }))
                .on_action(cx.listener(|this, _: &ToggleFindInSelection, _, cx| {
                    this.toggle_find_in_selection(cx)
                }))
                .child(div().pt_0p5().child(chevron))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(find_row)
                        .when(replace_open, |rows| rows.child(replace_row)),
                )
                .into_any_element(),
        )
    }
}
