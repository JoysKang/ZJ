//! The editor tab's right-click menu, VS Code's trimmed to what this editor needs: close,
//! close others, close all, copy (relative) path, reveal in Finder and in the Explorer. Menu
//! items act on the right-clicked tab; their shortcuts act on the active tab.

use super::{Pane, Prototype, Sidebar, explorer_ops::REVEAL_LABEL};
use gpui_kit::{
    component::menu::{PopupMenu, PopupMenuItem},
    *,
};
use std::path::PathBuf;

gpui_kit::actions!(
    tab,
    [
        CloseOtherEditors,
        CloseAllEditors,
        CopyActivePath,
        CopyActiveRelativePath,
        RevealActiveInFinder,
        RevealActiveInExplorer
    ]
);

type PaneAction = fn(&mut Prototype, Pane, &mut Window, &mut Context<Prototype>);

impl Prototype {
    /// The file a tab shows: the document, the diffed file, the graph's repository.
    fn pane_path(&self, pane: Pane) -> Option<PathBuf> {
        match pane {
            Pane::Document(id) => self
                .documents
                .iter()
                .find(|doc| doc.id == id && !doc.untitled)
                .map(|doc| doc.path.clone()),
            Pane::Diff => self.preview_diff.as_ref().map(|diff| diff.path.clone()),
            Pane::Graph => self.graph.as_ref().map(|graph| graph.repo.worktree.clone()),
            Pane::Welcome => None,
        }
    }

    pub(super) fn tab_menu(&self, pane: Pane, menu: PopupMenu, view: Entity<Self>) -> PopupMenu {
        let path = self.pane_path(pane);
        let tabs = self.documents.len()
            + usize::from(self.preview_diff.is_some())
            + usize::from(self.graph.is_some());
        let in_tree = matches!(pane, Pane::Document(_))
            && path
                .as_ref()
                .zip(self.root.as_ref())
                .is_some_and(|(path, root)| path.starts_with(root));
        let item = |label: &'static str, action: Box<dyn Action>, run: PaneAction| {
            let view = view.clone();
            PopupMenuItem::new(label)
                .action(action)
                .on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| run(this, pane, window, cx));
                })
        };
        menu.action_context(self.focus_handle.clone())
            .item(item(
                "关闭",
                Box::new(super::CloseEditor),
                |this, pane, window, cx| this.close_pane(pane, window, cx),
            ))
            .item(
                item(
                    "关闭其他",
                    Box::new(CloseOtherEditors),
                    Prototype::close_other_panes,
                )
                .disabled(tabs < 2),
            )
            .item(item(
                "全部关闭",
                Box::new(CloseAllEditors),
                |this, _, window, cx| this.close_all_panes(window, cx),
            ))
            .separator()
            .item(
                item(
                    "复制路径",
                    Box::new(CopyActivePath),
                    |this, pane, _, cx| this.copy_pane_path(pane, false, cx),
                )
                .disabled(path.is_none()),
            )
            .item(
                item(
                    "复制相对路径",
                    Box::new(CopyActiveRelativePath),
                    |this, pane, _, cx| this.copy_pane_path(pane, true, cx),
                )
                .disabled(path.is_none()),
            )
            .separator()
            .item(
                item(
                    REVEAL_LABEL,
                    Box::new(RevealActiveInFinder),
                    |this, pane, _, cx| this.reveal_pane_in_finder(pane, cx),
                )
                .disabled(path.is_none()),
            )
            .item(
                item(
                    "在资源管理器视图中显示",
                    Box::new(RevealActiveInExplorer),
                    Prototype::reveal_pane_in_explorer,
                )
                .disabled(!in_tree),
            )
    }

    pub(super) fn close_pane(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        match pane {
            Pane::Document(id) => self.close_document(id, window, cx),
            Pane::Diff => self.close_preview(window, cx),
            Pane::Graph => self.close_graph(window, cx),
            Pane::Welcome => {}
        }
    }

    /// Every tab but `keep`; edited documents are asked about together.
    pub(super) fn close_other_panes(
        &mut self,
        keep: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keep == Pane::Welcome {
            return;
        }
        if self.active != keep {
            self.select_pane(keep, window, cx);
        }
        if keep != Pane::Diff && self.preview_diff.is_some() {
            self.close_preview(window, cx);
        }
        if keep != Pane::Graph && self.graph.is_some() {
            self.close_graph(window, cx);
        }
        let others: Vec<_> = self
            .documents
            .iter()
            .map(|doc| doc.id)
            .filter(|id| Pane::Document(*id) != keep)
            .collect();
        let close = self.confirm_close(others, true, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if close.await {
                let _ = this.update_in(cx, |this, window, cx| this.select_pane(keep, window, cx));
            }
        })
        .detach();
    }

    pub(super) fn close_all_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.preview_diff.is_some() {
            self.close_preview(window, cx);
        }
        if self.graph.is_some() {
            self.close_graph(window, cx);
        }
        let all = self.documents.iter().map(|doc| doc.id).collect();
        self.confirm_close(all, true, window, cx).detach();
    }

    pub(super) fn copy_pane_path(&mut self, pane: Pane, relative: bool, cx: &mut Context<Self>) {
        let Some(path) = self.pane_path(pane) else {
            return;
        };
        let text = if relative {
            self.relative(&path).display().to_string()
        } else {
            path.display().to_string()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.message = "已复制路径".into();
        cx.notify();
    }

    pub(super) fn reveal_pane_in_finder(&mut self, pane: Pane, cx: &mut Context<Self>) {
        if let Some(path) = self.pane_path(pane) {
            self.reveal_in_finder(path, cx);
        }
    }

    /// Shows the Explorer with the tab's file selected; folders on the way are expanded as
    /// they load.
    pub(super) fn reveal_pane_in_explorer(
        &mut self,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Pane::Document(_) = pane else {
            return;
        };
        let Some(path) = self.pane_path(pane) else {
            return;
        };
        self.sidebar = Sidebar::Explorer;
        self.sidebar_visible = true;
        self.select_pane(pane, window, cx);
        self.select_tree_path(path, window, cx);
    }

    pub(super) fn active_pane_action(
        &mut self,
        run: PaneAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.active;
        run(self, pane, window, cx);
    }
}
