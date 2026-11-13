//! Editor area: VS Code-style tabs, breadcrumbs, the editor itself, and the welcome page.

use super::SINGLE_LINE;
use super::{
    NewWindow, OpenFile, OpenFolder, Pane, QuickOpenFile, ShowAllCommands, ToggleSidebar, Workbench,
};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    base::TestSupportExt,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Editor,
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::{cell::Cell, path::Path, rc::Rc};

#[derive(Default)]
pub(super) struct Tabs {
    pub(super) scroll: ScrollHandle,
    /// Reveal on selection, tab count or viewport changes, without undoing manual scrolling.
    revealed: Rc<Cell<Option<(Pane, usize, Pixels)>>>,
    pub(super) preview: Option<Pane>,
    pub(super) preview_request: Option<(u64, std::path::PathBuf)>,
    pub(super) order: Vec<TabKey>,
}

#[derive(Clone, PartialEq, Debug)]
pub(super) enum TabKey {
    File(std::path::PathBuf),
    Pane(Pane),
}

#[derive(Clone)]
pub(super) struct DraggedTab {
    pub(super) origin: EntityId,
    pub(super) key: TabKey,
    pub(super) order: std::sync::Arc<Vec<TabKey>>,
    label: String,
}
impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        div()
            .px_2()
            .py_1()
            .rounded(theme::RADIUS)
            .bg(colors.selected)
            .text_color(colors.selected_fg)
            .child(self.label.clone())
    }
}

impl Tabs {
    pub(super) fn reveal_selection(&self) {
        self.revealed.set(None);
    }
}

pub(super) struct TabSpec {
    key: usize,
    pub(super) pane: Pane,
    icon: &'static str,
    /// A Lucide icon instead of a file-type icon (the Git Graph tab).
    lucide: Option<IconName>,
    label: String,
    tooltip: String,
    /// The actual path, separate from the tab's descriptive tooltip.
    path: std::path::PathBuf,
    dirty: bool,
    /// Muted text after the label.
    note: Option<&'static str>,
    /// The file was deleted on disk (VS Code strikes the name through).
    deleted: bool,
    /// A tab restored from the last session, not read yet: choosing it opens the file.
    pending: Option<std::path::PathBuf>,
}

impl TabSpec {
    fn identity(&self) -> TabKey {
        if self.pending.is_some() || matches!(self.pane, Pane::Document(_) | Pane::Image(_)) {
            TabKey::File(self.path.clone())
        } else {
            TabKey::Pane(self.pane)
        }
    }
}

impl Workbench {
    pub(super) fn relative<'a>(&self, path: &'a Path) -> &'a Path {
        self.root
            .as_ref()
            .and_then(|root| path.strip_prefix(root).ok())
            .unwrap_or(path)
    }

    fn tab(
        &self,
        spec: TabSpec,
        first: bool,
        order: std::sync::Arc<Vec<TabKey>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme::colors(cx);
        let active = spec.pending.is_none() && self.active == spec.pane;
        let pane = spec.pane;
        let target = spec.identity();
        let dragged = DraggedTab {
            origin: cx.entity_id(),
            key: target.clone(),
            order,
            label: spec.label.clone(),
        };
        let pending = spec.pending.clone();
        let close_pending = spec.pending.clone();
        let menu_pending = spec.pending.clone();
        let group: SharedString = format!("tab-{}", spec.key).into();
        let close = gpui_kit::base::Button::new(("tab-close", spec.key))
            .accessibility_label(format!("关闭 {}", spec.label))
            .focus_visible(|close| close.bg(colors.hover).text_color(colors.accent))
            .size(theme::TAB_CLOSE)
            .flex_shrink_0()
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
                        Icon::new(IconName::Close)
                            .size(theme::ICON_SIZE)
                            .text_color(if active {
                                colors.foreground
                            } else {
                                colors.muted
                            }),
                    )
                }
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.focus_active_editor(window, cx);
                if let Some(path) = &close_pending {
                    this.drop_pending_tab(path, window, cx);
                    return;
                }
                match pane {
                    Pane::Document(id) => this.close_document(id, window, cx),
                    Pane::Diff => this.close_preview(window, cx),
                    Pane::Graph => this.close_graph(window, cx),
                    Pane::Large => this.close_large(window, cx),
                    Pane::Image(id) => this.close_image(id, window, cx),
                    Pane::SavedDiff(id) => this.close_saved_diff(id, window, cx),
                    Pane::Settings => this.close_settings(window, cx),
                    Pane::Welcome => {}
                }
            }));
        h_flex()
            .id(("tab", spec.key))
            .test_support()
            .group(group.clone())
            .flex_shrink_0()
            .max_w(theme::FILE_TAB_MAX_WIDTH)
            .min_w_0()
            .h(theme::TAB_HEIGHT)
            .pl_2()
            .pr_1()
            .gap_1()
            .relative()
            .rounded_t(theme::RADIUS)
            .when(first, |tab| tab.rounded_t(theme::WORKBENCH_RADIUS))
            .text_size(theme::TEXT_BODY)
            .cursor_pointer()
            .role(Role::Tab)
            .aria_label(spec.tooltip.clone())
            .aria_selected(active)
            .map(|tab| {
                if active {
                    tab.bg(colors.input_bg).text_color(colors.foreground).child(
                        div()
                            .absolute()
                            .top_0()
                            .left(theme::TAB_INDICATOR_INSET)
                            .w(theme::TAB_INDICATOR_WIDTH)
                            .h(theme::TAB_ACTIVE_INDICATOR_HEIGHT)
                            .rounded_full()
                            .bg(colors.accent),
                    )
                } else {
                    tab.bg(colors.tabs)
                        .text_color(colors.muted)
                        .hover(|tab| tab.bg(colors.hover).text_color(colors.foreground))
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
                    .id(("tab-label", spec.key))
                    .min_w_0()
                    .text_ellipsis()
                    .when(active, |label| label.font_weight(FontWeight::SEMIBOLD))
                    .when(self.tabs.preview == Some(pane), |label| label.italic())
                    .when(spec.deleted, |label| label.line_through())
                    .child(spec.label)
                    .test_support(),
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
                let weak = cx.weak_entity();
                move |window, cx| {
                    // Tooltips paint above everything, menus included; one that comes up while
                    // the tab's menu is open would cover the menu.
                    let menu_open = weak.upgrade().is_some_and(|view| {
                        view.read(cx)
                            .tab_menu_focus
                            .as_ref()
                            .is_some_and(|focus| focus.contains_focused(window, cx))
                    });
                    if menu_open {
                        return cx.new(|_| Empty).into();
                    }
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                }
            })
            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(colors.selected))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                this.reorder_tabs(dragged, Some(target.clone()), window, cx)
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 && this.tabs.preview == Some(pane) {
                    this.tabs.preview = None;
                }
                match &pending {
                    Some(path) => this.open_pending_tab(path.clone(), window, cx),
                    None => this.select_pane(pane, window, cx),
                }
            }))
            .context_menu({
                let weak = cx.weak_entity();
                move |menu, _, cx| match weak.upgrade() {
                    Some(view) => {
                        let focus = menu.focus_handle(cx);
                        view.update(cx, |this, _| this.tab_menu_focus = Some(focus));
                        view.read(cx).tab_menu(
                            pane,
                            menu_pending.clone(),
                            super::menu_style::compact_menu(menu),
                            view.clone(),
                        )
                    }
                    None => menu,
                }
            })
            .into_any_element()
    }

    pub(super) fn tab_specs(&self) -> Vec<TabSpec> {
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
                    path: doc.path.clone(),
                    dirty: doc.dirty,
                    note: doc.readonly.then_some("只读"),
                    deleted: doc.deleted,
                    pending: None,
                }
            })
            .collect();
        for image in &self.images {
            specs.push(TabSpec {
                key: usize::MAX / 4 + image.id.0 as usize,
                pane: Pane::Image(image.id),
                icon: file_icons::for_file(
                    &image.path.file_name().unwrap_or_default().to_string_lossy(),
                ),
                lucide: None,
                label: image
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .replace(SINGLE_LINE, "⏎"),
                tooltip: image.path.to_string_lossy().into_owned(),
                path: image.path.clone(),
                dirty: false,
                note: Some("图片"),
                deleted: false,
                pending: None,
            });
        }
        if let Some(diff) = &self.diff.tab {
            specs.push(TabSpec {
                key: usize::MAX,
                pane: Pane::Diff,
                icon: file_icons::for_file(
                    &diff.path.file_name().unwrap_or_default().to_string_lossy(),
                ),
                lucide: None,
                label: diff.label.clone(),
                tooltip: diff.tooltip.clone(),
                path: diff.path.clone(),
                dirty: false,
                note: None,
                deleted: false,
                pending: None,
            });
        }
        for saved in &self.archived_diffs {
            if let Some(diff) = &saved.pane.tab {
                specs.push(TabSpec {
                    key: usize::MAX / 8 + saved.id.0 as usize,
                    pane: Pane::SavedDiff(saved.id),
                    icon: file_icons::for_file(
                        &diff.path.file_name().unwrap_or_default().to_string_lossy(),
                    ),
                    lucide: None,
                    label: diff.label.clone(),
                    tooltip: diff.tooltip.clone(),
                    path: diff.path.clone(),
                    dirty: false,
                    note: Some("固定 Diff"),
                    deleted: false,
                    pending: None,
                });
            }
        }
        if let Some(graph) = &self.graph {
            specs.push(TabSpec {
                key: usize::MAX - 1,
                pane: Pane::Graph,
                icon: "",
                lucide: Some(IconName::GitGraph),
                label: "Git 图".to_string(),
                tooltip: format!("Git 图 · {}", graph.repo.worktree.display()),
                path: graph.repo.worktree.clone(),
                dirty: false,
                note: None,
                deleted: false,
                pending: None,
            });
        }
        if self.settings_pane.is_some() {
            specs.push(TabSpec {
                key: usize::MAX - 3,
                pane: Pane::Settings,
                icon: "",
                lucide: Some(IconName::Settings),
                label: "设置".to_string(),
                tooltip: "设置".to_string(),
                path: std::path::PathBuf::new(),
                dirty: false,
                note: None,
                deleted: false,
                pending: None,
            });
        }
        if let Some(large) = &self.large {
            let name = large
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(SINGLE_LINE, "⏎");
            specs.push(TabSpec {
                key: usize::MAX - 2,
                pane: Pane::Large,
                icon: file_icons::for_file(&name),
                lucide: None,
                label: name,
                tooltip: format!("{}（只读）", large.path.display()),
                path: large.path.clone(),
                dirty: false,
                note: Some("只读"),
                deleted: false,
                pending: None,
            });
        }
        for (index, path) in self.pending_tabs.iter().enumerate() {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(SINGLE_LINE, "⏎");
            specs.push(TabSpec {
                key: usize::MAX / 2 + index,
                pane: Pane::Welcome,
                icon: file_icons::for_file(&name),
                lucide: None,
                label: name,
                tooltip: path.to_string_lossy().into_owned(),
                path: path.clone(),
                dirty: false,
                note: None,
                deleted: false,
                pending: Some(path.clone()),
            });
        }
        // Restored tabs return to their saved places (stable: the rest keep their order).
        let rank = |spec: &TabSpec| match (&spec.pending, spec.pane) {
            (Some(path), _) => self.tab_rank(path),
            (None, Pane::Image(id)) => self
                .images
                .iter()
                .find(|image| image.id == id)
                .map_or(usize::MAX, |image| self.tab_rank(&image.path)),
            (None, Pane::Document(id)) => self
                .document(id)
                .map_or(usize::MAX, |doc| self.tab_rank(&doc.path)),
            _ => usize::MAX,
        };
        specs.sort_by_key(rank);
        specs.sort_by_key(|spec| {
            self.tabs
                .order
                .iter()
                .position(|key| *key == spec.identity())
                .unwrap_or(usize::MAX)
        });
        specs
    }

    pub(super) fn show_remaining_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let specs = self.tab_specs();
        if let Some(tab) = specs.iter().rev().find(|spec| spec.pending.is_none()) {
            if let Pane::SavedDiff(id) = tab.pane {
                self.activate_saved_diff(id, window, cx);
            } else {
                self.show_pane(tab.pane, window, cx);
            }
        } else if let Some(path) = specs.first().and_then(|spec| spec.pending.clone()) {
            self.show_pane(Pane::Welcome, window, cx);
            self.open_pending_tab(path, window, cx);
        } else {
            self.show_pane(Pane::Welcome, window, cx);
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let specs = self.tab_specs();
        let order = std::sync::Arc::new(specs.iter().map(TabSpec::identity).collect::<Vec<_>>());
        let active = self.active;
        let selected = specs
            .iter()
            .position(|spec| spec.pending.is_none() && spec.pane == active);
        let count = specs.len();
        let weak = cx.weak_entity();
        let menu_items: Vec<_> = specs
            .iter()
            .map(|spec| {
                let path = self.relative(&spec.path);
                let label = match spec.pane {
                    // Keep the comparison from the visible tab label, including local previews.
                    Pane::Diff | Pane::SavedDiff(_) => {
                        path.parent()
                            .unwrap_or(Path::new(""))
                            .join(&spec.label)
                            .to_string_lossy()
                            .into_owned()
                            + " · Diff"
                    }
                    Pane::Graph => format!("{} · Git 图", spec.path.display()),
                    Pane::Large => format!("{} · 只读", path.display()),
                    Pane::Settings => "设置".to_string(),
                    _ => path.display().to_string(),
                }
                .replace(SINGLE_LINE, "⏎");
                let label = if spec.dirty {
                    format!("{label} •")
                } else {
                    label
                };
                (label, spec.pane, spec.pending.clone())
            })
            .collect();
        let menu = Button::new("open-tabs")
            .ghost()
            .xsmall()
            .icon(IconName::ChevronDown)
            .tooltip("已打开文件")
            .accessibility_label("已打开文件")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let mut menu = super::menu_style::compact_menu(menu);
                for (label, pane, pending) in &menu_items {
                    let weak = weak.clone();
                    let pane = *pane;
                    let pending = pending.clone();
                    menu = menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(pending.is_none() && pane == active)
                            .on_click(move |_, window, cx| {
                                let _ = weak.update(cx, |this, cx| match &pending {
                                    Some(path) => this.open_pending_tab(path.clone(), window, cx),
                                    None => this.select_pane(pane, window, cx),
                                });
                            }),
                    );
                }
                menu
            });
        let scroll = self.tabs.scroll.clone();
        let revealed = self.tabs.revealed.clone();
        let tabs: Vec<AnyElement> = specs
            .into_iter()
            .enumerate()
            .map(|(index, spec)| self.tab(spec, index == 0, order.clone(), cx))
            .collect();
        let strip = h_flex()
            .on_children_prepainted(move |_, window, _| {
                let key = (active, count, scroll.bounds().size.width);
                if revealed.replace(Some(key)) != Some(key)
                    && let Some(selected) = selected
                {
                    scroll.scroll_to_item(selected);
                    window.request_animation_frame();
                }
            })
            .id("tab-strip")
            .test_support()
            .flex_1()
            .min_w_0()
            .h(theme::TAB_HEIGHT)
            .overflow_x_scroll()
            .track_scroll(&self.tabs.scroll)
            .children(tabs)
            .child(
                div()
                    .flex_1()
                    .h(theme::TAB_HEIGHT)
                    .on_drop(cx.listener(|this, dragged: &DraggedTab, window, cx| {
                        this.reorder_tabs(dragged, None, window, cx)
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::double_click_new_file)),
            );
        h_flex()
            .id("editor-tabs")
            .test_support()
            .h(theme::TAB_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .rounded_t(theme::WORKBENCH_RADIUS)
            .bg(colors.tabs)
            .child(strip)
            .child(
                h_flex()
                    .h(theme::TAB_HEIGHT)
                    .flex_shrink_0()
                    .px_1()
                    .children(self.render_markdown_toggle(cx))
                    .when(
                        matches!(self.active, Pane::Document(_) | Pane::Diff),
                        |toolbar| toolbar.child(self.render_review_actions(cx)),
                    )
                    .when(self.active == Pane::Diff, |toolbar| {
                        toolbar.child(
                            Button::new("pin-diff")
                                .ghost()
                                .xsmall()
                                .label(if self.diff_pinned {
                                    "取消固定"
                                } else {
                                    "固定"
                                })
                                .tooltip("固定当前比较，打开其他比较时保留此标签")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if this.diff_pinned {
                                        this.diff_pinned = false;
                                        this.remember_tabs(true, window, cx);
                                        cx.notify();
                                    } else {
                                        this.pin_current_diff(window, cx);
                                    }
                                })),
                        )
                    })
                    .child(menu),
            )
            .into_any_element()
    }

    /// As in VS Code: a double click on the welcome page or the empty end of the tab bar
    /// opens a new untitled file.
    fn double_click_new_file(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.click_count == 2 {
            self.new_untitled(window, cx);
        }
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
                // A double click on an item is two clicks on it, not a new file.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
        };
        v_flex()
            .id("welcome")
            .size_full()
            .on_mouse_down(MouseButton::Left, cx.listener(Self::double_click_new_file))
            .items_center()
            .justify_center()
            .gap_4()
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
                            // A steady cursor: anything that blinks keeps the window drawing.
                            .rounded(theme::LOGO_CURSOR_RADIUS)
                            .bg(theme::colors(cx).logo_cursor),
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
                        "welcome-commands",
                        "显示所有命令",
                        &["⇧", "⌘", "P"],
                        Box::new(ShowAllCommands),
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

    /// All remaining tab types stay reachable, including when the welcome pane has focus.
    pub(super) fn shows_tab_bar(&self) -> bool {
        self.active != Pane::Welcome || !self.tab_specs().is_empty()
    }

    pub(super) fn render_editor_area(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.shows_tab_bar() {
            return self.render_welcome(cx);
        }
        let content = match self.active {
            Pane::Welcome | Pane::SavedDiff(_) => self.render_welcome(cx),
            Pane::Diff if self.image_diff.is_some() => self.render_image_diff(cx),
            Pane::Diff => self.render_diff(cx),
            Pane::Graph => self.render_graph(cx),
            Pane::Large => self.render_large(cx),
            Pane::Settings => self.render_settings(cx),
            Pane::Image(id) => self.render_image(id, cx),
            Pane::Document(id) => match self.documents.iter().find(|doc| doc.id == id) {
                Some(doc) if doc.markdown.as_ref().is_some_and(|md| !md.source) => {
                    self.render_markdown_preview(id, cx)
                }
                Some(doc) => div()
                    .key_context(super::edit_commands::CONTEXT)
                    .size_full()
                    .child(
                        Editor::new(&doc.editor)
                            .readonly(doc.encoding.is_some())
                            .bordered(false)
                            .size_full(),
                    )
                    .into_any_element(),
                None => div().into_any_element(),
            },
        };
        v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            // The shell paints the surface; rectangular child fills leak past its corners.
            .child(self.render_tabs(cx))
            .children(self.render_disk_banner(cx))
            .children(self.render_conflict_bar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    // `h_flex` centers on the cross axis; the editor fills the row's height.
                    .items_stretch()
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(content)
                            .children(self.render_find(cx)),
                    )
                    // Outside the find overlay's frame, right of the editor's scrollbar.
                    .children(self.render_conflict_ruler(cx)),
            )
            .into_any_element()
    }
}
