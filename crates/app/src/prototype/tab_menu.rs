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
        RevealActiveInExplorer,
        ReopenClosedEditor
    ]
);

/// A closed document tab, for ⇧⌘T: the file and where the cursor was.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ClosedTab {
    pub path: PathBuf,
    /// UTF-8 offset of the cursor when the tab closed.
    pub offset: usize,
}

/// The window's closed-tab stack (in memory only): most recent first, one entry per path.
#[derive(Default)]
pub(super) struct ClosedTabs {
    tabs: Vec<ClosedTab>,
}

/// VS Code keeps a hundred; twenty is plenty for a window that holds at most 20 documents.
const MAX_CLOSED: usize = 20;

impl ClosedTabs {
    /// Re-closing a path replaces its entry and moves it to the top.
    pub(super) fn push(&mut self, tab: ClosedTab) {
        self.tabs.retain(|kept| kept.path != tab.path);
        self.tabs.insert(0, tab);
        self.tabs.truncate(MAX_CLOSED);
    }

    /// The most recently closed tab whose file still exists; missing files are dropped.
    pub(super) fn pop_existing(&mut self) -> Option<ClosedTab> {
        loop {
            let tab = self.tabs.first()?;
            if tab.path.is_file() {
                return Some(self.tabs.remove(0));
            }
            self.tabs.remove(0);
        }
    }

    #[cfg(test)]
    fn paths(&self) -> Vec<&PathBuf> {
        self.tabs.iter().map(|tab| &tab.path).collect()
    }
}

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

    /// ⇧⌘T: reopens the most recently closed tab whose file is still there, cursor restored.
    pub(super) fn reopen_closed_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.closed_tabs.pop_existing() else {
            self.message = "没有可以重新打开的编辑器".into();
            cx.notify();
            return;
        };
        // open_file loads in the background and applies the pending placement.
        self.nav.pending_place = Some((
            tab.path.clone(),
            super::navigation::Placement::Offset(tab.offset),
        ));
        self.open_file(tab.path, self.root.clone(), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::{ClosedTab, ClosedTabs, MAX_CLOSED};
    use std::path::PathBuf;

    fn tab(name: &str, offset: usize) -> ClosedTab {
        ClosedTab {
            path: PathBuf::from(name),
            offset,
        }
    }

    #[test]
    fn reclosing_a_path_keeps_only_the_latest() {
        let mut tabs = ClosedTabs::default();
        tabs.push(tab("/a.rs", 3));
        tabs.push(tab("/b.rs", 0));
        tabs.push(tab("/a.rs", 40));
        assert_eq!(
            tabs.paths(),
            [&PathBuf::from("/a.rs"), &PathBuf::from("/b.rs")]
        );
        assert_eq!(tabs.pop_existing(), None); // none of them exists
    }

    #[test]
    fn the_stack_is_capped() {
        let mut tabs = ClosedTabs::default();
        for i in 0..MAX_CLOSED + 5 {
            tabs.push(tab(&format!("/{i}.rs"), i));
        }
        assert_eq!(tabs.paths().len(), MAX_CLOSED);
        assert_eq!(tabs.tabs[0], tab("/24.rs", 24));
        assert!(!tabs.paths().contains(&&PathBuf::from("/0.rs")));
    }

    #[test]
    fn reopening_skips_files_that_are_gone() {
        let root = std::env::temp_dir().join(format!("zj-closed-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let kept = root.join("kept.rs");
        std::fs::write(&kept, "fn main() {}\n").unwrap();
        let mut tabs = ClosedTabs::default();
        tabs.push(tab(kept.to_str().unwrap(), 7));
        tabs.push(tab(root.join("gone.rs").to_str().unwrap(), 1));
        assert_eq!(
            tabs.pop_existing(),
            Some(ClosedTab {
                path: kept.clone(),
                offset: 7
            })
        );
        assert_eq!(tabs.pop_existing(), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
