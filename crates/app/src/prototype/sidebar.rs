//! The sidebar: VS Code-style view switcher on top, then Explorer / Search / Source Control.

use super::SINGLE_LINE;
use super::{Decoration, DecorationKind, Prototype, Sidebar};
use crate::{file_icons, theme};
use gpui_kit::{
    assets::IconName,
    component::{
        Icon, Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::Input,
        v_flex,
    },
    prelude::FluentBuilder,
    *,
};

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
                    this.search_input
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
        let Some(root) = self.tree.first() else {
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
        let expanded = !self.explorer_collapsed;
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
                    ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.explorer_collapsed = !this.explorer_collapsed;
                cx.notify();
            }));
        v_flex()
            .size_full()
            .min_h_0()
            .child(header)
            .when(!self.tree_message.is_empty(), |list| {
                list.child(
                    div()
                        .px(theme::TREE_BASE)
                        .text_size(theme::TEXT_CAPTION)
                        .text_color(colors.muted)
                        .child(self.tree_message.clone()),
                )
            })
            .when(expanded, |list| {
                list.child(
                    uniform_list(
                        "file-tree",
                        self.tree.len().saturating_sub(1),
                        cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| this.tree_row(index + 1, cx))
                                .collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(&self.tree_scroll)
                    .flex_1()
                    .w_full(),
                )
            })
            .into_any_element()
    }

    fn tree_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let row = &self.tree[index];
        let entry = &row.entry;
        let path = entry.path.clone();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let directory = entry.directory;
        let expanded = self.expanded.contains(&path);
        let selected = self
            .documents
            .iter()
            .any(|doc| self.active == super::Pane::Document(doc.id) && doc.path == path);
        let level = row.depth.saturating_sub(1);
        let decoration: Option<Decoration> = self.decorations.get(&path).copied();
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(name_color)
                    .child(format!("{name}{}", if entry.symlink { " ↗" } else { "" })),
            )
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
            .on_click(cx.listener(move |this, _, window, cx| {
                if directory {
                    this.toggle_directory(index, window, cx);
                } else {
                    this.open_file(path.clone(), this.root.clone(), window, cx);
                }
            }));
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let query_empty = self.search_input.read(cx).value().trim().is_empty();
        let summary = if self.searching && self.index.is_none() {
            "正在建立文件索引…".to_string()
        } else if self.searching {
            "搜索中…".into()
        } else if query_empty {
            "按文件名或路径片段模糊匹配".into()
        } else {
            format!(
                "{} 个文件{}{}",
                self.search_results.paths.len(),
                if self.search_results.incomplete {
                    "（部分结果）"
                } else {
                    ""
                },
                if self.search_results.errors > 0 {
                    format!(" · {} 项读取错误", self.search_results.errors)
                } else {
                    String::new()
                }
            )
        };
        v_flex()
            .size_full()
            .min_h_0()
            .gap_1()
            .child(
                div()
                    .px(theme::TREE_BASE)
                    .child(Input::new(&self.search_input).small()),
            )
            .child(
                div()
                    .px(theme::TREE_BASE)
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(summary),
            )
            .child(
                uniform_list(
                    "file-search",
                    self.search_results.paths.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| this.search_row(index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .w_full(),
            )
            .into_any_element()
    }

    fn search_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let colors = theme::colors(cx);
        let path = self.search_results.paths[index].clone();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(SINGLE_LINE, "⏎");
        let directory = path
            .parent()
            .and_then(|parent| {
                self.root
                    .as_ref()
                    .and_then(|root| parent.strip_prefix(root).ok())
            })
            .map(|parent| parent.to_string_lossy().replace(SINGLE_LINE, "⏎"))
            .unwrap_or_default();
        let row = h_flex()
            .id(("search-row", index))
            .w_full()
            .h(theme::ROW_HEIGHT)
            .pl(theme::TREE_BASE - theme::ROW_INSET)
            .pr_2()
            .gap_2()
            .rounded(theme::RADIUS)
            .overflow_hidden()
            .text_size(theme::TEXT_BODY)
            .role(Role::Button)
            .aria_label(format!("打开文件 {}", path.display()))
            .hover(|row| row.bg(colors.hover))
            .child(file_icons::icon(file_icons::for_file(&name)))
            .child(div().flex_shrink_0().child(name))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(theme::TEXT_CAPTION)
                    .text_color(colors.muted)
                    .child(directory),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_file(path.clone(), this.root.clone(), window, cx)
            }));
        div()
            .w_full()
            .px(theme::ROW_INSET)
            .child(row)
            .into_any_element()
    }
}
