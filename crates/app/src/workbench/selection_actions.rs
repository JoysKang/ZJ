//! Deliberately opened selection tools; the captured document and text never follow tab changes.

use super::*;
use crate::agent_model::Attachment;
use gpui_kit::{
    assets::IconName,
    base::Selectable,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex, input,
        native_menu::NativeMenu,
    },
};
use std::ops::Range;

gpui_kit::actions!(editor, [ShowSelectionActions]);

pub(super) struct SelectionActions {
    document: DocumentId,
    editor: Entity<EditorState>,
    range: Range<usize>,
    attachment: Attachment,
    focus: FocusHandle,
    selected: usize,
}

#[derive(Clone, Copy)]
enum SelectionAction {
    Explain,
    Modify,
    Context,
}

const ACTIONS: [(SelectionAction, &str, IconName); 3] = [
    (SelectionAction::Explain, "解释", IconName::Code),
    (SelectionAction::Modify, "修改", IconName::Pencil),
    (
        SelectionAction::Context,
        "加入上下文",
        IconName::SquareDashedText,
    ),
];

/// Both ⌘L and the toolbar use the primary UTF-8 selection and the same line boundaries.
pub(super) fn attachment(doc: &Document, cx: &App) -> Attachment {
    let editor = doc.editor.read(cx);
    let range = editor.selected_range();
    if range.is_empty() {
        return Attachment::File(doc.path.clone());
    }
    let (start, end) = selection_lines(editor, &range);
    Attachment::Selection {
        path: doc.path.clone(),
        start,
        end,
        text: editor.text().slice(range).to_string(),
    }
}

fn selection_lines(editor: &EditorState, range: &Range<usize>) -> (u32, u32) {
    use gpui_kit::component::input::RopeExt;
    let rope = editor.text();
    let start = rope.offset_to_position(range.start);
    let mut end = rope.offset_to_position(range.end);
    // A selection ending at the start of a line does not include that line.
    if end.character == 0 && end.line > start.line {
        end.line -= 1;
    }
    (start.line + 1, end.line + 1)
}

/// Kit lets the app replace the native menu. Keep editing and navigation alongside the new entry.
pub(super) fn editor_menu(editor: &EditorState, menu: NativeMenu) -> NativeMenu {
    let caps = editor.context_menu_capabilities();
    let enabled = !caps.is_disabled();
    let editable = enabled && !caps.is_readonly();
    menu.menu_with_disabled(
        "选区操作…",
        !enabled || !caps.is_copyable(),
        Box::new(ShowSelectionActions),
    )
    .separator()
    .menu_with_disabled(
        "转到定义",
        !enabled || !caps.has_definition(),
        Box::new(input::GoToDefinition),
    )
    .menu_with_disabled(
        "代码操作",
        !editable || !caps.has_code_actions(),
        Box::new(input::ToggleCodeActions),
    )
    .separator()
    .menu_with_disabled(
        "剪切",
        !editable || !caps.is_copyable(),
        Box::new(input::Cut),
    )
    .menu_with_disabled("复制", !caps.is_copyable(), Box::new(input::Copy))
    .menu_with_disabled("粘贴", !editable, Box::new(input::Paste))
    .separator()
    .menu("全选", Box::new(input::SelectAll))
}

impl Workbench {
    pub(super) fn show_selection_actions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Pane::Document(id) = self.active else {
            return;
        };
        let Some(doc) = self.document(id) else {
            return;
        };
        if doc.markdown.as_ref().is_some_and(|md| !md.source) {
            return;
        }
        let range = doc.editor.read(cx).selected_range();
        if range.is_empty() {
            return;
        }
        let editor = doc.editor.clone();
        let attachment = attachment(doc, cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.selection_actions = Some(SelectionActions {
            document: id,
            editor,
            range,
            attachment,
            focus,
            selected: 0,
        });
        cx.notify();
    }

    fn close_selection_actions(
        &mut self,
        restore_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tools) = self.selection_actions.take() {
            if restore_focus && self.active == Pane::Document(tools.document) {
                tools
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            }
            cx.notify();
        }
    }

    fn selection_actions_valid(&self, tools: &SelectionActions, cx: &App) -> bool {
        if self.active != Pane::Document(tools.document) {
            return false;
        }
        let Some(doc) = self.document(tools.document) else {
            return false;
        };
        let Attachment::Selection {
            path,
            start,
            end,
            text,
        } = &tools.attachment
        else {
            return false;
        };
        let editor = tools.editor.read(cx);
        doc.path == *path
            && editor.selected_range() == tools.range
            && editor.selected_text() == text.as_str()
            && selection_lines(editor, &tools.range) == (*start, *end)
    }

    fn apply_selection_action(
        &mut self,
        action: SelectionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tools) = self.selection_actions.take() else {
            return;
        };
        if !self.selection_actions_valid(&tools, cx) {
            self.focus_active_editor(window, cx);
            cx.notify();
            return;
        }
        if !self.agent.visible {
            self.set_agent_panel(true, window, cx);
        }
        if self.agent.view != agent::AgentView::Thread {
            self.agent_show(agent::AgentView::Thread, window, cx);
        }
        if !self.agent.attachments.contains(&tools.attachment) {
            self.agent.attachments.push(tools.attachment);
        }
        match action {
            SelectionAction::Context => {
                tools
                    .editor
                    .update(cx, |editor, cx| editor.focus(window, cx));
            }
            SelectionAction::Explain | SelectionAction::Modify => {
                self.agent.composer.update(cx, |input, cx| {
                    let draft = input.value();
                    let prompt = match action {
                        SelectionAction::Explain => "请解释这段代码。",
                        SelectionAction::Modify if draft.trim().is_empty() => "请修改这段代码：",
                        SelectionAction::Modify => "请结合上面的要求修改这段代码。",
                        SelectionAction::Context => unreachable!(),
                    };
                    let addition = if draft.is_empty() {
                        prompt.to_owned()
                    } else {
                        format!("\n\n{prompt}")
                    };
                    // Insert at the end; set_value would discard inline image references and undo history.
                    let end = draft.encode_utf16().count();
                    input.replace_text_in_range(Some(end..end), &addition, window, cx);
                });
                self.agent_focus_composer(window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn render_selection_actions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tools = self.selection_actions.as_ref()?;
        if !self.selection_actions_valid(tools, cx) {
            self.close_selection_actions(true, window, cx);
            return None;
        }
        let editor = tools.editor.read(cx);
        let viewport = editor.input_bounds();
        // Zero-length endpoint bounds also work for selections spanning offscreen or unequal line widths.
        let bounds = editor
            .range_to_bounds(&(tools.range.start..tools.range.start))
            .or_else(|| editor.range_to_bounds(&(tools.range.end..tools.range.end)));
        let (anchor, above) = bounds.map_or((viewport.origin, false), |bounds| {
            (
                bounds.origin,
                bounds.top() - theme::SELECTION_ACTIONS_HEIGHT - theme::ROW_INSET >= viewport.top(),
            )
        });
        let screen = window.viewport_size();
        let width = theme::SELECTION_ACTIONS_WIDTH.min(screen.width - theme::ROW_INSET * 2.);
        let left = anchor
            .x
            .max(theme::ROW_INSET)
            .min(screen.width - width - theme::ROW_INSET);
        let top = if above {
            anchor.y - theme::SELECTION_ACTIONS_HEIGHT - theme::ROW_INSET
        } else {
            anchor.y + editor.line_height().unwrap_or(theme::ROW_HEIGHT) + theme::ROW_INSET
        };
        let top = top
            .max(theme::TITLE_HEIGHT)
            .min(screen.height - theme::STATUS_HEIGHT - theme::SELECTION_ACTIONS_HEIGHT);
        let colors = theme::colors(cx);
        Some(
            h_flex()
                .id("selection-actions")
                .test_support()
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .h(theme::SELECTION_ACTIONS_HEIGHT)
                .px_1()
                .gap_1()
                .justify_between()
                .bg(colors.card)
                .text_color(colors.foreground)
                .border_1()
                .border_color(colors.card_border)
                .rounded(theme::RADIUS_LARGE)
                .shadow(vec![theme::overlay_shadow(colors.shadow)])
                .occlude()
                .track_focus(&tools.focus)
                .key_context("SelectionActions")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    this.close_selection_actions(false, window, cx);
                }))
                .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => this.close_selection_actions(true, window, cx),
                        "enter" => {
                            if let Some(tools) = &this.selection_actions {
                                this.apply_selection_action(ACTIONS[tools.selected].0, window, cx);
                            }
                        }
                        "left" | "right" | "tab" => {
                            if let Some(tools) = &mut this.selection_actions {
                                let backwards = event.keystroke.key == "left"
                                    || (event.keystroke.key == "tab"
                                        && event.keystroke.modifiers.shift);
                                tools.selected = (tools.selected
                                    + if backwards { ACTIONS.len() - 1 } else { 1 })
                                    % ACTIONS.len();
                                cx.notify();
                            }
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }))
                .children(
                    ACTIONS
                        .into_iter()
                        .enumerate()
                        .map(|(index, (action, label, icon))| {
                            Button::new(("selection-action", index))
                                .ghost()
                                .xsmall()
                                .icon(icon)
                                .label(label)
                                .selected(index == tools.selected)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.apply_selection_action(action, window, cx)
                                }))
                        }),
                )
                .child(
                    Button::new("selection-actions-close")
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("关闭（Esc）")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_selection_actions(true, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}
