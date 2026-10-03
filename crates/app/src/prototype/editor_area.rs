//! Editor area: VS Code-style tabs, breadcrumbs, the editor itself, and the welcome page.

use super::SINGLE_LINE;
use super::{NewWindow, OpenFile, OpenFolder, Pane, Prototype, QuickOpenFile, ToggleSidebar};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{Icon, h_flex, input::Editor, v_flex},
    prelude::FluentBuilder,
    *,
};
use std::path::Path;

struct TabSpec {
    key: usize,
    pane: Pane,
    icon: &'static str,
    /// A Lucide icon instead of a file-type icon (the Git Graph tab).
    lucide: Option<IconName>,
    label: String,
    tooltip: String,
    dirty: bool,
    /// Muted text after the label ("Agent 建议").
    note: Option<&'static str>,
    /// The file was deleted on disk (VS Code strikes the name through).
    deleted: bool,
}

impl Prototype {
    pub(super) fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        self.root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
    }

    fn tab(&self, spec: TabSpec, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let active = self.active == spec.pane;
        let pane = spec.pane;
        let group: SharedString = format!("tab-{}", spec.key).into();
        let close = div()
            .id(("tab-close", spec.key))
            .size(theme::TAB_CLOSE)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS)
            .hover(|close| close.bg(colors.hover))
            .map(|close| {
                if spec.dirty {
                    // A dirty dot that turns into × on hover, as in VS Code.
                    close
                        .child(
                            div()
                                .size(theme::DIRTY_DOT)
                                .rounded_full()
                                .bg(colors.foreground)
                                .group_hover(group.clone(), |dot| dot.opacity(0.)),
                        )
                        .child(
                            div()
                                .absolute()
                                .opacity(0.)
                                .group_hover(group.clone(), |icon| icon.opacity(1.))
                                .child(
                                    Icon::new(IconName::Close)
                                        .size(theme::ICON_SIZE)
                                        .text_color(colors.foreground),
                                ),
                        )
                        .relative()
                } else {
                    close.child(
                        div()
                            .when(!active, |icon| {
                                icon.opacity(0.)
                                    .group_hover(group.clone(), |icon| icon.opacity(1.))
                            })
                            .child(
                                Icon::new(IconName::Close)
                                    .size(theme::ICON_SIZE)
                                    .text_color(colors.foreground),
                            ),
                    )
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                match pane {
                    Pane::Document(id) => this.close_document(id, window, cx),
                    Pane::Diff => this.close_preview(window, cx),
                    Pane::Graph => this.close_graph(window, cx),
                    Pane::Welcome => {}
                }
            }));
        h_flex()
            .id(("tab", spec.key))
            .group(group.clone())
            .flex_shrink_0()
            .h_full()
            .pl_3()
            .pr_1()
            .gap_1()
            .relative()
            .border_r_1()
            .border_color(colors.border)
            .text_size(theme::TEXT_BODY)
            .cursor_pointer()
            .role(Role::Tab)
            .aria_label(spec.tooltip.clone())
            .map(|tab| {
                if active {
                    tab.bg(colors.editor).text_color(colors.foreground).child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(theme::INDICATOR)
                            .bg(colors.accent),
                    )
                } else {
                    tab.bg(colors.tabs).text_color(colors.muted)
                }
            })
            .child(match spec.lucide {
                Some(name) => Icon::new(name)
                    .size(theme::FILE_ICON_SIZE)
                    .text_color(if active {
                        colors.foreground
                    } else {
                        colors.muted
                    })
                    .into_any_element(),
                None => file_icons::icon(spec.icon).into_any_element(),
            })
            .child(
                div()
                    .pl_1()
                    .when(spec.deleted, |label| label.line_through())
                    .child(spec.label),
            )
            .when_some(spec.note, |tab, note| {
                tab.child(
                    div()
                        .text_size(theme::TEXT_SECTION)
                        .text_color(colors.muted)
                        .child(note),
                )
            })
            .when(spec.deleted, |tab| {
                tab.child(
                    div()
                        .pl_1()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child("已删除"),
                )
            })
            .child(close)
            .tooltip({
                let tooltip = spec.tooltip.clone();
                move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                }
            })
            .on_click(cx.listener(move |this, _, window, cx| this.select_pane(pane, window, cx)))
            .into_any_element()
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let mut specs: Vec<TabSpec> = self
            .documents
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let name = doc
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎");
                TabSpec {
                    key: index,
                    pane: Pane::Document(doc.id),
                    icon: file_icons::for_file(&name),
                    lucide: None,
                    label: name,
                    tooltip: doc.path.to_string_lossy().into_owned(),
                    dirty: doc.dirty,
                    note: None,
                    deleted: doc.deleted,
                }
            })
            .collect();
        if let Some(diff) = &self.preview_diff {
            specs.push(TabSpec {
                key: usize::MAX,
                pane: Pane::Diff,
                icon: file_icons::for_file(
                    &diff.path.file_name().unwrap_or_default().to_string_lossy(),
                ),
                lucide: None,
                label: diff.label.clone(),
                tooltip: diff.tooltip.clone(),
                dirty: false,
                note: diff.agent().map(|agent| match agent.origin {
                    workspace_editor_agent::thread::ChangeOrigin::Proposed => "Agent 建议",
                    workspace_editor_agent::thread::ChangeOrigin::Written => "Agent 修改",
                }),
                deleted: false,
            });
        }
        if let Some(graph) = &self.graph {
            specs.push(TabSpec {
                key: usize::MAX - 1,
                pane: Pane::Graph,
                icon: "",
                lucide: Some(IconName::GitGraph),
                label: "Git 图".to_string(),
                tooltip: format!("Git 图 · {}", graph.repo.worktree.display()),
                dirty: false,
                note: None,
                deleted: false,
            });
        }
        let tabs: Vec<AnyElement> = specs.into_iter().map(|spec| self.tab(spec, cx)).collect();
        let strip = h_flex()
            .id("tab-strip")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .children(tabs)
            .child(div().flex_1().h_full());
        h_flex()
            .h(theme::TAB_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .bg(colors.tabs)
            .child(strip)
            .when(
                self.active == Pane::Diff && !self.diff_is_agent_review(),
                |bar| bar.child(self.render_diff_actions(cx)),
            )
            .into_any_element()
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let (path, suffix) = match self.active {
            Pane::Document(id) => match self.documents.iter().find(|doc| doc.id == id) {
                Some(doc) => (
                    Some(doc.path.clone()),
                    doc.readonly.then_some("只读文件（保存时需另存为）"),
                ),
                None => (None, None),
            },
            Pane::Diff => (
                self.preview_diff.as_ref().map(|diff| diff.path.clone()),
                Some(if self.diff_is_agent_review() {
                    "Agent 修改审阅"
                } else {
                    "Diff · 只读"
                }),
            ),
            Pane::Welcome | Pane::Graph => (None, None),
        };
        let Some(path) = path else {
            return div().into_any_element();
        };
        let relative = self.relative(&path).to_path_buf();
        let segments: Vec<String> = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎")
            })
            .collect();
        let last = segments.len().saturating_sub(1);
        let mut crumbs = h_flex()
            .h(theme::BREADCRUMB_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .px_3()
            .gap_1()
            .overflow_hidden()
            .bg(colors.editor)
            .text_size(theme::TEXT_CAPTION)
            .text_color(colors.muted);
        for (index, segment) in segments.into_iter().enumerate() {
            if index > 0 {
                crumbs = crumbs.child(
                    Icon::new(IconName::ChevronRight)
                        .size(theme::SMALL_ICON_SIZE)
                        .text_color(colors.muted),
                );
            }
            if index == last {
                crumbs = crumbs
                    .child(file_icons::icon(file_icons::for_file(&segment)))
                    .child(div().text_color(colors.foreground).child(segment));
            } else {
                crumbs = crumbs.child(div().flex_shrink_0().child(segment));
            }
        }
        crumbs
            .when_some(suffix, |crumbs, suffix| {
                crumbs.child(div().pl_2().child(suffix))
            })
            .into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let keycaps = |keys: &[&'static str]| {
            h_flex().gap_1().children(keys.iter().map(|key| {
                div()
                    .min_w(theme::KEYCAP)
                    .h(theme::KEYCAP)
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS)
                    .bg(colors.keycap)
                    .text_color(colors.foreground)
                    .text_size(theme::TEXT_SECTION)
                    .child(*key)
            }))
        };
        let item = |id: &'static str,
                    label: &'static str,
                    keys: &[&'static str],
                    action: Box<dyn Action>| {
            h_flex()
                .id(id)
                .w_full()
                .h(theme::TAB_HEIGHT)
                .px_2()
                .justify_between()
                .rounded(theme::RADIUS)
                .cursor_pointer()
                .hover(|row| row.bg(colors.hover))
                .text_size(theme::TEXT_BODY)
                .text_color(colors.muted)
                .child(label)
                .child(keycaps(keys))
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(colors.editor)
            .child(
                div()
                    .relative()
                    .w(theme::LOGO_WIDTH)
                    .h(theme::LOGO_HEIGHT)
                    .child(
                        img(if gpui_kit::component::Theme::global(cx).is_dark() {
                            "logo/zj-sprig-dark.png"
                        } else {
                            "logo/zj-sprig.png"
                        })
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(theme::LOGO_CURSOR_LEFT)
                            .top(theme::LOGO_CURSOR_TOP)
                            .w(theme::LOGO_CURSOR_WIDTH)
                            .h(theme::LOGO_CURSOR_HEIGHT)
                            .child(self.welcome_cursor.clone()),
                    ),
            )
            .child(
                v_flex()
                    .w(theme::WELCOME_WIDTH)
                    .child(item(
                        "welcome-quick-open",
                        "转到文件",
                        &["⌘", "P"],
                        Box::new(QuickOpenFile),
                    ))
                    .child(item(
                        "welcome-open-file",
                        "打开文件",
                        &["⌘", "O"],
                        Box::new(OpenFile),
                    ))
                    .child(item(
                        "welcome-open-folder",
                        "打开文件夹",
                        &["⌘K", "⌘O"],
                        Box::new(OpenFolder),
                    ))
                    .child(item(
                        "welcome-new-window",
                        "新建窗口",
                        &["⇧", "⌘", "N"],
                        Box::new(NewWindow),
                    ))
                    .child(item(
                        "welcome-toggle-sidebar",
                        "切换侧栏",
                        &["⌘", "B"],
                        Box::new(ToggleSidebar),
                    )),
            )
            .into_any_element()
    }

    pub(super) fn render_editor_area(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let content = match self.active {
            Pane::Welcome => return self.render_welcome(cx),
            Pane::Diff => self.render_diff(cx),
            Pane::Graph => self.render_graph(cx),
            Pane::Document(id) => match self.documents.iter().find(|doc| doc.id == id) {
                Some(doc) => Editor::new(&doc.editor)
                    .bordered(false)
                    .size_full()
                    .into_any_element(),
                None => div().into_any_element(),
            },
        };
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(colors.editor)
            .child(self.render_tabs(cx))
            .child(self.render_breadcrumbs(cx))
            .when(self.active == Pane::Diff, |area| {
                area.children(self.render_agent_review_bar(cx))
            })
            .children(self.render_disk_banner(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .children(self.render_find(cx)),
            )
            .into_any_element()
    }
}
