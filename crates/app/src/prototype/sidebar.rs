//! The sidebar: VS Code-style view switcher on top, then Explorer / Search / Source Control.

use super::SINGLE_LINE;
use super::explorer_ops::{EditKind, FileClipboard};
use super::{Decoration, DecorationKind, Prototype, Sidebar};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub(super) fn chevron(expanded: bool, color: Hsla) -> Icon {
    Icon::new(if expanded {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    })
    .size(theme::ICON_SIZE)
    .text_color(color)
}

impl theme::Colors {
    pub(super) fn decoration(&self, kind: DecorationKind) -> Hsla {
        match kind {
            DecorationKind::Untracked => self.untracked,
            DecorationKind::Added => self.added,
            DecorationKind::Modified => self.modified,
            DecorationKind::Deleted => self.deleted,
            DecorationKind::Conflict => self.conflict,
        }
    }
}

impl Prototype {
    pub(super) fn change_count(&self) -> usize {
        self.groups
            .iter()
            .filter_map(|group| match &group.status {
                Some(Ok(status)) => Some(status.changes.len()),
                _ => None,
            })
            .sum()
    }

    fn activity_item(
        &self,
        view: Sidebar,
        icon: IconName,
        label: &'static str,
        badge: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        let active = self.sidebar == view;
        div()
            .id(label)
            .relative()
            .size(theme::ACTIVITY_ITEM)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS)
            .cursor_pointer()
            .role(Role::Tab)
            .aria_label(label)
            .when(active, |item| item.bg(colors.hover))
            .hover(|item| item.bg(colors.hover))
            .child(
                Icon::new(icon)
                    .size(theme::ICON_SIZE + theme::ROW_INSET)
                    .text_color(if active {
                        colors.foreground
                    } else {
                        colors.muted
                    }),
            )
            .when(badge > 0, |item| {
                item.child(
                    div()
                        .absolute()
                        .right(theme::BADGE_OFFSET)
                        .bottom(theme::BADGE_OFFSET)
                        .h(theme::BADGE_SIZE)
                        .min_w(theme::BADGE_SIZE)
                        .px_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(colors.badge)
                        .text_color(colors.badge_fg)
                        .text_size(theme::TEXT_BADGE)
                        .font_weight(FontWeight::BOLD)
                        .child(if badge > 99 {
                            "99+".to_string()
                        } else {
                            badge.to_string()
                        }),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sidebar = view;
                if view == Sidebar::Search {
                    this.search
                        .query
                        .update(cx, |input, cx| input.focus(window, cx));
                }
                cx.notify();
            }))
    }

    pub(super) fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let changes = self.change_count();
        let title = match self.sidebar {
            Sidebar::Explorer => "资源管理器",
            Sidebar::Search => "搜索",
            Sidebar::SourceControl => "源代码管理",
        };
        let content = match self.sidebar {
            Sidebar::Explorer => self.render_explorer(cx).into_any_element(),
            Sidebar::Search => self.render_search(cx).into_any_element(),
            Sidebar::SourceControl => self.render_scm(cx).into_any_element(),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .bg(colors.panel)
            .border_r_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .h(theme::ACTIVITY_HEIGHT)
                    .flex_shrink_0()
                    .px_1()
                    .gap_1()
                    .child(self.activity_item(
                        Sidebar::Explorer,
                        IconName::Files,
                        "资源管理器",
                        0,
                        cx,
                    ))
                    .child(self.activity_item(Sidebar::Search, IconName::Search, "搜索", 0, cx))
                    .child(self.activity_item(
                        Sidebar::SourceControl,
                        IconName::GitBranch,
                        "源代码管理",
                        changes,
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .id("sidebar-title")
                    .group("sidebar-title")
                    .h(theme::SIDEBAR_TITLE_HEIGHT)
                    .flex_shrink_0()
                    .pl(theme::TREE_BASE)
                    .pr_2()
                    .justify_between()
                    .text_size(theme::TEXT_SECTION)
                    .text_color(colors.muted)
                    .child(title)
                    .child(
                        h_flex()
                            .gap_1()
                            .when(self.sidebar == Sidebar::SourceControl, |actions| {
                                actions.child(
                                    Button::new("refresh-git")
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::RefreshCw)
                                        .tooltip("刷新")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.refresh(window, cx)
                                        })),
                                )
                            })
                            .when(self.sidebar == Sidebar::SourceControl, |actions| {
                                actions.child(self.scm_more(cx))
                            }),
                    ),
            )
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element()
    }

    fn render_explorer(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let Some(root) = self.explorer.rows.first() else {
            return v_flex()
                .size_full()
                .px(theme::TREE_BASE)
                .py_2()
                .gap_2()
                .text_size(theme::TEXT_BODY)
                .text_color(colors.muted)
                .child("尚未打开文件夹。")
                .child(
                    Button::new("explorer-open-folder")
                        .small()
                        .primary()
                        .w_full()
                        .label("打开文件夹")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.choose_path(true, window, cx)),
                        ),
                )
                .into_any_element();
        };
        let name = root
            .entry
            .path
            .file_name()
            .unwrap_or(root.entry.path.as_os_str())
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let expanded = !self.explorer.collapsed;
        let header = h_flex()
            .id("explorer-section")
            .group("explorer-section")
            .h(theme::SECTION_HEIGHT)
            .flex_shrink_0()
            .pl(theme::ROW_INSET)
            .pr_2()
            .gap_1()
            .cursor_pointer()
            .text_size(theme::TEXT_SECTION)
            .font_weight(FontWeight::BOLD)
            .child(chevron(expanded, colors.foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(name.to_uppercase()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .opacity(0.)
                    .group_hover("explorer-section", |actions| actions.opacity(1.))
                    .child(
                        Button::new("explorer-new-file")
                            .xsmall()
                            .ghost()
                            .icon(IconName::FilePlus)
                            .tooltip("新建文件…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(super::NewFile), cx);
                                this.explorer.focus.focus(window, cx);
                            })),
                    )
                    .child(
                        Button::new("explorer-new-folder")
                            .xsmall()
                            .ghost()
                            .icon(IconName::FolderPlus)
                            .tooltip("新建文件夹…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                window.dispatch_action(Box::new(super::NewFolder), cx);
                                this.explorer.focus.focus(window, cx);
                            })),
                    )
                    .child(
                        Button::new("refresh-tree")
                            .xsmall()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .tooltip("刷新资源管理器")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.refresh_tree(window, cx)
                            })),
                    )
                    .child(
                        Button::new("collapse-tree")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronsDownUp)
                            .tooltip("全部折叠")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.collapse_tree(window, cx)
                            })),
                    )
                    .child(self.explorer_more()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.explorer.collapsed = !this.explorer.collapsed;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.explorer.selection = this.root.clone();
                    this.explorer.focus.focus(window, cx);
                    cx.notify();
                }),
            );
        let weak = cx.weak_entity();
        let root_path = root.entry.path.clone();
        let header = header.context_menu(move |menu, _, cx| {
            let can_paste = cx
                .try_global::<FileClipboard>()
                .is_some_and(|c| !c.paths.is_empty());
            match weak.upgrade() {
                Some(this) => this.read(cx).explorer_menu(&root_path, can_paste, menu),
                None => menu,
            }
        });
        let explorer = v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.explorer.focus);
        self.explorer_actions(explorer, cx)
            .child(header)
            .when(!self.explorer.message.is_empty(), |list| {
                list.child(
                    div()
                        .px(theme::TREE_BASE)
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(self.explorer.message.clone()),
                )
            })
            .when(expanded, |list| {
                list.child(
                    uniform_list(
                        "file-tree",
                        self.explorer.rows.len().saturating_sub(1),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.tree_row(index + 1, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.explorer.scroll)
                    .flex_1()
                    .w_full(),
                )
            })
            .into_any_element()
    }

    /// The Explorer header's "···" menu.
    fn explorer_more(&self) -> AnyElement {
        let show_hidden = self.explorer.show_hidden;
        let focus = self.focus_handle.clone();
        Button::new("explorer-more")
            .xsmall()
            .ghost()
            .icon(IconName::Ellipsis)
            .tooltip("更多操作")
            .dropdown_menu(move |menu, _, _| {
                menu.action_context(focus.clone()).item(
                    PopupMenuItem::new("显示隐藏文件")
                        .checked(show_hidden)
                        .action(Box::new(super::ToggleHiddenFiles)),
                )
            })
            .into_any_element()
    }

    /// The inline name field for a new file or folder, indented like its future row.
    fn pending_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let row = &self.explorer.rows[index];
        let level = row.depth.saturating_sub(1);
        let Some(edit) = &self.explorer.edit else {
            return div().h(theme::ROW_HEIGHT).into_any_element();
        };
        let folder = matches!(edit.kind, EditKind::NewFolder(_));
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(
                h_flex()
                    .id(("tree-row", index))
                    .w_full()
                    .h(theme::ROW_HEIGHT)
                    .pl(theme::TREE_BASE - theme::ROW_INSET + theme::TREE_STEP * level as f32)
                    .pr_2()
                    .rounded(theme::RADIUS)
                    .bg(colors.hover)
                    .child(div().w(theme::TWISTY_WIDTH).flex_shrink_0())
                    .child(
                        div()
                            .pl_1()
                            .pr(theme::ROW_INSET * 3.)
                            .child(file_icons::icon(if folder {
                                file_icons::FOLDER
                            } else {
                                file_icons::for_file("")
                            })),
                    )
                    .child(div().flex_1().min_w_0().child(self.edit_field(cx))),
            )
            .into_any_element()
    }

    fn edit_field(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(edit) = &self.explorer.edit else {
            return div().into_any_element();
        };
        div()
            .key_context("ExplorerEdit")
            // The field reports Enter as `PressEnter` and lets the action bubble; stop it here
            // so the Explorer's own Enter (rename) does not start a new edit.
            .on_action(|_: &gpui_kit::component::input::Enter, _, _| {})
            .on_action(cx.listener(
                |this, _: &gpui_kit::component::input::Escape, window, cx| {
                    this.cancel_tree_edit(cx);
                    this.explorer.focus.focus(window, cx);
                },
            ))
            .child(
                Input::new(&edit.input)
                    .xsmall()
                    .text_size(theme::TEXT_BODY)
                    .h(theme::ROW_HEIGHT - theme::ROW_INSET * 2.),
            )
            .into_any_element()
    }

    fn tree_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let row = &self.explorer.rows[index];
        if row.pending {
            return self.pending_row(index, cx);
        }
        let entry = &row.entry;
        let path = entry.path.clone();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let directory = entry.directory;
        let expanded = self.explorer.expanded.contains(&path);
        let selected = match &self.explorer.selection {
            Some(selection) => *selection == path,
            None => self
                .documents
                .iter()
                .any(|doc| self.active == super::Pane::Document(doc.id) && doc.path == path),
        };
        let editing = self
            .explorer
            .edit
            .as_ref()
            .is_some_and(|edit| edit.kind == EditKind::Rename(path.clone()));
        let level = row.depth.saturating_sub(1);
        let decoration: Option<Decoration> = self.explorer.decorations.get(&path).copied();
        let name_color = match decoration {
            _ if selected => colors.selected_fg,
            Some(decoration) => colors.decoration(decoration.kind),
            None => colors.foreground,
        };
        let guides = (0..level).map(|ancestor| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(
                    theme::TREE_BASE
                        + theme::TREE_STEP * ancestor as f32
                        + theme::TWISTY_WIDTH / 2.,
                )
                .w(theme::GUIDE_WIDTH)
                .bg(colors.indent_guide)
        });
        let row = h_flex()
            .id(("tree-row", index))
            .relative()
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pl(theme::TREE_BASE - theme::ROW_INSET + theme::TREE_STEP * level as f32)
            .pr_2()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .role(Role::TreeItem)
            .aria_label(format!(
                "{} {}",
                if directory { "目录" } else { "文件" },
                path.display()
            ))
            .when(selected, |row| row.bg(colors.selected))
            .when(!selected, |row| row.hover(|row| row.bg(colors.hover)))
            .children(guides)
            .child(
                div()
                    .w(theme::TWISTY_WIDTH)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .when(directory, |twisty| {
                        twisty.child(chevron(
                            expanded,
                            if selected {
                                colors.selected_fg
                            } else {
                                colors.muted
                            },
                        ))
                    }),
            )
            .child(
                div()
                    .pl_1()
                    .pr(theme::ROW_INSET * 3.)
                    .child(file_icons::icon(if directory {
                        if expanded {
                            file_icons::FOLDER_OPEN
                        } else {
                            file_icons::FOLDER
                        }
                    } else {
                        file_icons::for_file(&name)
                    })),
            )
            .child(if editing {
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.edit_field(cx))
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(name_color)
                    .child(format!("{name}{}", if entry.symlink { " ↗" } else { "" }))
                    .into_any_element()
            })
            .when_some(decoration, |row, decoration| {
                let color = colors.decoration(decoration.kind);
                row.child(
                    div()
                        .w(theme::DECORATION_WIDTH)
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(if selected { colors.selected_fg } else { color })
                        .map(|slot| {
                            if directory {
                                slot.child(
                                    div()
                                        .size(theme::DECORATION_DOT)
                                        .rounded_full()
                                        .bg(color.opacity(0.7)),
                                )
                            } else {
                                slot.child(decoration.letter.to_string())
                            }
                        }),
                )
            })
            .on_mouse_down(MouseButton::Right, {
                let path = path.clone();
                cx.listener(move |this, _, window, cx| {
                    this.select_tree_path(path.clone(), window, cx)
                })
            })
            .on_click({
                let path = path.clone();
                cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if this.explorer.edit.is_some() {
                        return;
                    }
                    this.select_tree_path(path.clone(), window, cx);
                    if directory {
                        this.toggle_directory(index, window, cx);
                    } else {
                        // One click previews and keeps the tree focused (arrow keys, ⌘C, F2);
                        // a double click moves into the editor, as in VS Code.
                        this.explorer.focus_on_open = event.click_count() < 2;
                        this.open_file(path.clone(), this.root.clone(), window, cx);
                    }
                })
            });
        let weak = cx.weak_entity();
        let row = row.context_menu(move |menu, _, cx| {
            let can_paste = cx
                .try_global::<FileClipboard>()
                .is_some_and(|c| !c.paths.is_empty());
            match weak.upgrade() {
                Some(this) => this.read(cx).explorer_menu(&path, can_paste, menu),
                None => menu,
            }
        });
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}

/// The Explorer: the visible tree, expanded folders, selection and inline rename, and its folder listings.
pub(super) struct Explorer {
    pub(super) rows: Vec<super::TreeRow>,
    pub(super) collapsed: bool,
    pub(super) decorations: HashMap<PathBuf, Decoration>,
    pub(super) scroll: UniformListScrollHandle,
    /// The Explorer and quick open show dot entries hidden by default (settings, ⌘⇧.).
    pub(super) show_hidden: bool,
    /// The Explorer row file operations act on (clicked or right-clicked).
    pub(super) selection: Option<PathBuf>,
    pub(super) focus: FocusHandle,
    pub(super) edit: Option<super::explorer_ops::TreeEdit>,
    /// A single click in the Explorer opens the file but keeps the focus in the tree.
    pub(super) focus_on_open: bool,
    pub(super) reveal_pending: bool,
    pub(super) expanded: HashSet<PathBuf>,
    pub(super) restore_expanded: HashSet<PathBuf>,
    pub(super) tasks: HashMap<PathBuf, Task<()>>,
    pub(super) generation: u64,
    pub(super) message: String,
}
