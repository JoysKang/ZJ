//! Reviewing an agent's changes in the diff editor: the file as it was before the agent (or
//! the base of a pending proposal) against what is on disk now (or the proposal), top /
//! bottom by default, with 接受 / 拒绝 on every change block and for the whole file.

use super::agent::AgentView;
use super::agent_panel::glyph_tile;
use super::*;
use crate::agent_model;
use gpui_kit::{
    assets::IconName,
    component::{
        Selectable, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
    },
    prelude::FluentBuilder,
};
use workspace_editor_agent::{Glyph, review::full_context_patch};

gpui_kit::actions!(agent_review, [AcceptAgentChange, RejectAgentChange]);

#[derive(Clone, Debug)]
pub(super) struct AgentDiff {
    /// The live session whose client holds the snapshot or proposal.
    pub key: u64,
    pub path: PathBuf,
    pub agent: String,
    pub glyph: Glyph,
    pub title: String,
}

impl Prototype {
    /// Opens (or refreshes) the review of one changed file in the diff tab.
    pub(super) fn agent_open_review(
        &mut self,
        key: u64,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.agent.session(key) else {
            return;
        };
        let diff = AgentDiff {
            key,
            path: path.clone(),
            agent: session.preset.display_name.clone(),
            glyph: session.preset.glyph,
            title: session.title(),
        };
        let tab = DiffTab {
            label: agent_model::file_name(&path),
            tooltip: path.display().to_string(),
            path,
            source: DiffSource::Agent(diff),
        };
        self.close_preview(window, cx);
        self.message.clear();
        self.diff.tab = Some(tab);
        self.active = Pane::Diff;
        // The panel stays where it is; the review is the editor's job.
        self.update_welcome_blink(window, cx);
        self.diff.focus.focus(window, cx);
        self.load_diff(window, cx);
    }

    /// Opens the first changed file of the current session (the card's 审阅).
    pub(super) fn agent_review_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.agent.current else {
            return;
        };
        let path = self
            .agent
            .session(key)
            .and_then(|s| s.thread.changed_files.keys().next().cloned());
        if let Some(path) = path {
            self.agent_open_review(key, path, window, cx);
        }
    }

    /// Rebuilds the open review if it belongs to session `key`.
    pub(super) fn agent_reload_review(
        &mut self,
        key: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .diff
            .tab
            .as_ref()
            .and_then(|tab| tab.agent())
            .is_some_and(|diff| diff.key == key)
        {
            self.load_diff(window, cx);
        }
    }

    pub(super) fn load_agent_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(diff) = self.diff.tab.as_ref().and_then(|tab| tab.agent()).cloned() else {
            return;
        };
        let client = self.agent.session(diff.key).and_then(|s| s.client.clone());
        self.diff.cancel.store(true, Ordering::Relaxed);
        self.diff.generation += 1;
        self.diff.stale = false;
        let version = self.diff.generation;
        let highlight = gpui_kit::component::Theme::global(cx)
            .highlight_theme
            .clone();
        let dark = gpui_kit::component::Theme::global(cx).is_dark();
        let colors = theme::colors(cx);
        let change_colors = crate::diff_doc::ChangeColors {
            inserted_text: colors.diff_added_text,
            removed_text: colors.diff_deleted_text,
        };
        let language = match language_for(&diff.path).0 {
            "plain" => None,
            language => Some(language),
        };
        let path = diff.path.clone();
        let job = cx.background_spawn(async move {
            let client = client?;
            let (before, after) = workspace_editor_agent::review::review_texts(&client, &path)?;
            let (patch, hunks) = full_context_patch(before.as_deref().unwrap_or(""), &after);
            if hunks.is_empty() {
                return None;
            }
            let text: Arc<str> = patch.into();
            let doc = crate::diff_doc::DiffDoc::parse(&text, language, &highlight, change_colors)
                .map(Arc::new);
            Some((text, doc))
        });
        self.diff.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.diff.generation != version {
                    return;
                }
                this.diff.raw = None;
                this.diff.selection = None;
                this.diff.fallback = None;
                match result {
                    Some((text, Some(doc))) => {
                        let same = this
                            .diff
                            .source
                            .as_ref()
                            .is_some_and(|(old, was_dark)| *old == text && *was_dark == dark);
                        if !same {
                            let fresh = this.diff.source.is_none();
                            this.diff.source = Some((text, dark));
                            this.diff.doc = Some(doc);
                            // Block indexes changed; keep the position near the old change.
                            let keep = this.diff.change;
                            if fresh {
                                this.reveal_first_change();
                            } else if let Some(index) = keep {
                                let count = this.diff_change_starts().len();
                                this.diff.change = (count > 0).then(|| index.min(count - 1));
                            }
                        }
                        this.diff.title = this
                            .diff
                            .tab
                            .as_ref()
                            .map(|tab| tab.label.clone())
                            .unwrap_or_default();
                    }
                    Some((_, None)) => {
                        this.diff.doc = None;
                        this.diff.source = None;
                        this.diff.title = "这个文件太大或不是文本，无法逐处审阅".into();
                    }
                    None => {
                        this.diff.doc = None;
                        this.diff.source = None;
                        this.diff.change = None;
                        this.diff.title = "这个文件已没有待审阅的修改".into();
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// 接受 / 拒绝 one change block of the open review.
    pub(super) fn agent_review_hunk(
        &mut self,
        index: usize,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(diff) = self.diff.tab.as_ref().and_then(|tab| tab.agent()).cloned() else {
            return;
        };
        let Some(client) = self.agent.session(diff.key).and_then(|s| s.client.clone()) else {
            self.message = "这个会话的 Agent 已关闭，修改记录不在了".into();
            cx.notify();
            return;
        };
        // The patch on screen: block `index` is only meaningful against it.
        let Some(shown) = self.diff.source.as_ref().map(|(text, _)| text.clone()) else {
            return;
        };
        let path = diff.path.clone();
        let job = cx.background_spawn(async move {
            workspace_editor_agent::review::resolve_hunk(&client, &path, index, accept, &shown)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    this.message = error;
                }
                eprintln!("event=agent_review_hunk accept={accept}");
                this.reload_document_from_disk(&diff.path, window, cx);
                this.agent_resolved(diff.key, window, cx);
            });
        })
        .detach();
    }

    /// ⌘Y / ⌘⌫ in the diff editor: the current change (the first when none is selected).
    pub(super) fn agent_review_current(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.diff_is_agent_review() || self.diff_change_starts().is_empty() {
            return;
        }
        let index = self.diff.change.unwrap_or(0);
        self.agent_review_hunk(index, accept, window, cx);
    }

    /// 接受此文件 / 拒绝此文件.
    pub(super) fn agent_review_file(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(diff) = self.diff.tab.as_ref().and_then(|tab| tab.agent()).cloned() else {
            return;
        };
        self.agent_resolve_files(diff.key, vec![diff.path], accept, window, cx);
    }

    fn agent_resolved(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_reload_review(key, window, cx);
        self.agent_recount(key, window, cx);
        cx.notify();
    }

    /// The toolbar above an agent review (design 01-A: "Claude Code 建议的修改 … 接受此文件").
    pub(super) fn render_agent_review_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let diff = self.diff.tab.as_ref()?.agent()?;
        let colors = theme::colors(cx);
        let (added, removed) = self
            .diff
            .doc
            .as_ref()
            .map_or((0, 0), |doc| (doc.added, doc.removed));
        let count = self.diff_change_starts().len();
        let position = match self.diff.change {
            Some(index) if count > 0 => format!("第 {} / {count} 处", index + 1),
            _ => format!("共 {count} 处"),
        };
        let inline = self.diff_is_inline();
        let one_sided = self
            .diff
            .doc
            .as_ref()
            .is_none_or(|doc| doc.old.lines.is_empty() || doc.new.lines.is_empty());
        let heading = format!("{} 已写入的修改", diff.agent);
        let layout = |id: &'static str, icon: IconName, on: bool, label: &'static str| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .selected(on)
                .tooltip(label)
                .accessibility_label(label)
        };
        Some(
            h_flex()
                .h(theme::AGENT_REVIEW_BAR)
                .w_full()
                .flex_shrink_0()
                .px_3()
                .gap_2()
                .border_b_1()
                .border_color(colors.border)
                .bg(colors.editor)
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .child(glyph_tile(diff.glyph, theme::AGENT_GLYPH_SMALL, colors))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(colors.foreground)
                        .child(heading),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(format!("· {}", diff.title)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(colors.added)
                        .child(format!("+{added}")),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(colors.deleted)
                        .child(format!("−{removed}")),
                )
                .child(div().flex_1())
                .child(div().flex_shrink_0().child(position))
                .child(
                    Button::new("agent-review-previous")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronUp)
                        .tooltip("上一处")
                        .on_click(cx.listener(|this, _, _, cx| this.step_change(false, cx))),
                )
                .child(
                    Button::new("agent-review-next")
                        .ghost()
                        .xsmall()
                        .icon(IconName::ChevronDown)
                        .tooltip("下一处")
                        .on_click(cx.listener(|this, _, _, cx| this.step_change(true, cx))),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            layout("agent-review-inline", IconName::Rows2, inline, "上下显示")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.diff.inline {
                                        this.toggle_diff_layout(window, cx);
                                    }
                                })),
                        )
                        .child(
                            layout(
                                "agent-review-split",
                                IconName::Columns2,
                                !inline,
                                "左右显示",
                            )
                            .when(one_sided, |b| b.opacity(0.5))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if this.diff.inline && !one_sided {
                                        this.toggle_diff_layout(window, cx);
                                    }
                                },
                            )),
                        ),
                )
                .child(
                    Button::new("agent-review-reject-file")
                        .ghost()
                        .xsmall()
                        .label("拒绝此文件")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_review_file(false, window, cx)
                        })),
                )
                .child(
                    Button::new("agent-review-accept-file")
                        .primary()
                        .xsmall()
                        .icon(IconName::Check)
                        .label("接受此文件")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.agent_review_file(true, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }

    /// Whether the diff tab reviews an agent's changes (block buttons become 接受 / 拒绝).
    pub(super) fn diff_is_agent_review(&self) -> bool {
        self.diff
            .tab
            .as_ref()
            .is_some_and(|tab| tab.agent().is_some())
    }

    pub(super) fn agent_view_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_show(AgentView::History, window, cx);
    }
}

/// 接受 / 拒绝 at the top right of a change block, on its first row. `viewport` is the list's
/// horizontal scroll offset and visible width: the buttons stay at the right edge of what is
/// visible while long lines scroll underneath.
pub(super) fn agent_block_actions(
    list: &'static str,
    index: usize,
    block_start: usize,
    current: bool,
    viewport: (Pixels, Pixels),
    colors: theme::Colors,
    cx: &mut Context<Prototype>,
) -> AnyElement {
    let button = |accept: bool| {
        let label = if accept { "接受" } else { "拒绝" };
        Button::new((
            SharedString::from(format!("{list}-agent-{label}")),
            block_start,
        ))
        .xsmall()
        .compact()
        .map(|b| {
            if accept && current {
                b.primary()
            } else {
                b.ghost()
            }
        })
        .icon(if accept {
            IconName::Check
        } else {
            IconName::Undo2
        })
        .label(label)
        .tooltip(if accept {
            "接受这一处修改（当前处：⌘Y）"
        } else {
            "拒绝这一处修改，还原成 Agent 之前的内容（当前处：⌘⌫）"
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.diff.change = Some(index);
            this.agent_review_hunk(index, accept, window, cx);
        }))
    };
    let (scroll_x, width) = viewport;
    h_flex()
        .absolute()
        .top_0()
        .h_full()
        .map(|strip| {
            // Before the list's first layout the width is unknown; the row then ends at the
            // visible edge unless a line is wider than the view.
            if width > Pixels::ZERO {
                strip.left(-scroll_x).w(width)
            } else {
                strip.left_0().right_0()
            }
        })
        .justify_end()
        .pr(theme::AGENT_HUNK_ACTIONS_INSET)
        .child(
            h_flex()
                .h_full()
                .gap_1()
                .px_1()
                .items_center()
                .rounded(theme::RADIUS)
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(button(false))
                .child(button(true)),
        )
        .into_any_element()
}
