//! Explorer file operations, VS Code style: a right-click menu on every row (and on the
//! workspace header) plus keyboard shortcuts on the focused tree. Names are edited inline in
//! the row; deletion moves to the Trash after a confirmation. The filesystem work runs in the
//! background (`file_ops`); file watching then refreshes Git status and the quick-open index,
//! and the affected folders are listed again right away.

use super::{Pane, Prototype, TreeRow};
use crate::{file_ops, files::Entry};
use gpui_kit::{
    component::{
        input::{InputEvent, InputState},
        menu::{PopupMenu, PopupMenuItem},
    },
    *,
};
use std::path::{Path, PathBuf};

gpui_kit::actions!(
    explorer,
    [
        NewFile,
        NewFolder,
        Rename,
        Delete,
        CopyFiles,
        CutFiles,
        PasteFiles,
        CopyPath,
        CopyRelativePath,
        RevealInFinder
    ]
);

/// Copied or cut Explorer entries, shared by all windows.
#[derive(Default)]
pub struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

impl Global for FileClipboard {}

#[derive(Clone, PartialEq)]
pub(super) enum EditKind {
    Rename(PathBuf),
    /// A new entry inside this folder; the edit row sits under it.
    NewFile(PathBuf),
    NewFolder(PathBuf),
}

pub(super) struct TreeEdit {
    pub kind: EditKind,
    pub input: Entity<InputState>,
    _subscription: Subscription,
}

#[cfg(target_os = "macos")]
pub(super) const REVEAL_LABEL: &str = "在访达中显示";
#[cfg(not(target_os = "macos"))]
pub(super) const REVEAL_LABEL: &str = "在文件管理器中显示";

impl Prototype {
    /// The folder new entries and pasted files go into: the selected folder, the selected
    /// file's folder, or the workspace root.
    fn target_folder(&self) -> Option<PathBuf> {
        match &self.tree_selection {
            Some(path) => {
                let directory = self
                    .tree
                    .iter()
                    .find(|row| row.entry.path == *path)
                    .is_some_and(|row| row.entry.directory);
                if directory {
                    Some(path.clone())
                } else {
                    path.parent().map(Path::to_path_buf)
                }
            }
            None => self.root.clone(),
        }
    }

    pub(super) fn select_tree_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tree_selection = Some(path);
        self.explorer_focus.focus(window, cx);
        cx.notify();
    }

    /// The right-click menu for `path` (a row, or the workspace root for the header).
    pub(super) fn explorer_menu(&self, path: &Path, can_paste: bool, menu: PopupMenu) -> PopupMenu {
        let root = self.root.as_deref() == Some(path);
        menu.action_context(self.explorer_focus.clone())
            .item(PopupMenuItem::new("新建文件…").action(Box::new(NewFile)))
            .item(PopupMenuItem::new("新建文件夹…").action(Box::new(NewFolder)))
            .item(PopupMenuItem::new(REVEAL_LABEL).action(Box::new(RevealInFinder)))
            .separator()
            .item(
                PopupMenuItem::new("剪切")
                    .disabled(root)
                    .action(Box::new(CutFiles)),
            )
            .item(
                PopupMenuItem::new("复制")
                    .disabled(root)
                    .action(Box::new(CopyFiles)),
            )
            .item(
                PopupMenuItem::new("粘贴")
                    .disabled(!can_paste)
                    .action(Box::new(PasteFiles)),
            )
            .separator()
            .item(PopupMenuItem::new("复制路径").action(Box::new(CopyPath)))
            .item(PopupMenuItem::new("复制相对路径").action(Box::new(CopyRelativePath)))
            .separator()
            .item(
                PopupMenuItem::new("重命名…")
                    .disabled(root)
                    .action(Box::new(Rename)),
            )
            .item(
                PopupMenuItem::new("删除")
                    .disabled(root)
                    .action(Box::new(Delete)),
            )
            .separator()
            .item(
                PopupMenuItem::new("显示隐藏文件")
                    .checked(self.show_hidden)
                    .action(Box::new(super::ToggleHiddenFiles)),
            )
    }

    /// Starts an inline name edit: renaming the selection, or a new entry in the target folder.
    pub(super) fn start_tree_edit(
        &mut self,
        kind: EditKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_tree_edit(cx);
        let (initial, select) = match &kind {
            EditKind::Rename(path) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                // Select the stem, as Finder and VS Code do.
                let stem = name.rfind('.').filter(|dot| *dot > 0).unwrap_or(name.len());
                (name, 0..stem)
            }
            EditKind::NewFile(_) | EditKind::NewFolder(_) => (String::new(), 0..0),
        };
        if let EditKind::NewFile(dir) | EditKind::NewFolder(dir) = &kind {
            let Some(index) = self.tree.iter().position(|row| row.entry.path == *dir) else {
                return;
            };
            if !self.expanded.contains(dir) {
                self.load_directory(dir.clone(), window, cx);
            }
            let depth = self.tree[index].depth + 1;
            self.tree.insert(
                index + 1,
                TreeRow {
                    entry: Entry {
                        path: dir.clone(),
                        directory: matches!(kind, EditKind::NewFolder(_)),
                        symlink: false,
                    },
                    depth,
                    pending: true,
                },
            );
        }
        let placeholder = match &kind {
            EditKind::Rename(_) => "新名称",
            EditKind::NewFile(_) => "文件名",
            EditKind::NewFolder(_) => "文件夹名",
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(initial)
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_selected_range(select, cx);
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                match event {
                    InputEvent::PressEnter { .. } => this.commit_tree_edit(window, cx),
                    // Leaving the field keeps what was typed, as in VS Code; an empty name cancels.
                    InputEvent::Blur => {
                        if this
                            .tree_edit
                            .as_ref()
                            .is_some_and(|edit| edit.input.read(cx).value().trim().is_empty())
                        {
                            this.cancel_tree_edit(cx);
                        } else {
                            this.commit_tree_edit(window, cx);
                        }
                    }
                    _ => {}
                }
            },
        );
        self.tree_edit = Some(TreeEdit {
            kind,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn cancel_tree_edit(&mut self, cx: &mut Context<Self>) {
        if self.tree_edit.take().is_some() {
            self.tree.retain(|row| !row.pending);
            cx.notify();
        }
    }

    fn commit_tree_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.tree_edit.take() else {
            return;
        };
        self.tree.retain(|row| !row.pending);
        let name = edit.input.read(cx).value().to_string();
        let kind = edit.kind.clone();
        if let EditKind::Rename(path) = &kind
            && path
                .file_name()
                .is_some_and(|old| old.to_string_lossy() == name.trim())
        {
            self.explorer_focus.focus(window, cx);
            cx.notify();
            return;
        }
        let job = cx.background_spawn({
            let kind = kind.clone();
            async move {
                match &kind {
                    EditKind::Rename(path) => file_ops::rename(path, &name),
                    EditKind::NewFile(dir) => file_ops::new_file(dir, &name),
                    EditKind::NewFolder(dir) => file_ops::new_folder(dir, &name),
                }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(path) => {
                        match &kind {
                            EditKind::Rename(old) => {
                                this.follow_move(old, &path);
                                if let Some(parent) = old.parent() {
                                    this.relist(parent.to_path_buf(), window, cx);
                                }
                            }
                            EditKind::NewFile(dir) | EditKind::NewFolder(dir) => {
                                this.relist(dir.clone(), window, cx)
                            }
                        }
                        let opens = matches!(kind, EditKind::NewFile(_));
                        this.tree_selection = Some(path.clone());
                        if opens {
                            this.open_file(path, this.root.clone(), window, cx);
                        } else {
                            this.explorer_focus.focus(window, cx);
                        }
                    }
                    Err(error) => this.message = format!("操作失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Lists a folder again if it is shown (expanded or the root).
    fn relist(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.expanded.contains(&dir) {
            self.reload_directory(dir, window, cx);
        }
    }

    /// Open tabs, the diff tab and the selection follow a renamed or moved entry.
    fn follow_move(&mut self, old: &Path, new: &Path) {
        let moved = |path: &Path| {
            path.strip_prefix(old).ok().map(|rest| {
                if rest.as_os_str().is_empty() {
                    new.to_path_buf()
                } else {
                    new.join(rest)
                }
            })
        };
        for document in &mut self.documents {
            if let Some(path) = moved(&document.path) {
                if let Some(owner) = self.owners.borrow_mut().get_mut(&document.id) {
                    owner.path = path.clone();
                }
                document.path = path;
            }
        }
        if let Some(selection) = self.tree_selection.as_deref().and_then(moved) {
            self.tree_selection = Some(selection);
        }
        let expanded: Vec<_> = self.expanded.iter().filter_map(|p| moved(p)).collect();
        self.expanded.retain(|p| !p.starts_with(old));
        self.restore_expanded.extend(expanded);
    }

    pub(super) fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.tree_selection.clone() else {
            return;
        };
        if self.root.as_deref() == Some(path.as_path()) {
            return;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let (trash, detail) = if cfg!(target_os = "macos") {
            ("移到废纸篓", "可以从废纸篓里恢复。")
        } else {
            ("移到回收站", "可以从回收站里恢复。")
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("删除“{name}”？"),
            Some(detail),
            &[trash, "取消"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let target = path.clone();
            let result = cx
                .background_spawn(async move { file_ops::trash(&target) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.tree_selection = None;
                        // Unedited tabs of what was deleted close; edited ones stay open.
                        let closing: Vec<_> = this
                            .documents
                            .iter()
                            .filter(|doc| doc.path.starts_with(&path) && !doc.dirty)
                            .map(|doc| doc.id)
                            .collect();
                        for id in closing {
                            this.remove_document(id, window, cx);
                        }
                        // The edited ones are now the only copy: marked 已删除, saving recreates them.
                        for doc in &mut this.documents {
                            if doc.path.starts_with(&path) {
                                doc.deleted = true;
                                doc.banner = None;
                            }
                        }
                        if let Some(parent) = path.parent() {
                            this.relist(parent.to_path_buf(), window, cx);
                        }
                    }
                    Err(error) => this.message = format!("删除失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        let Some(path) = self.tree_selection.clone() else {
            return;
        };
        if self.root.as_deref() == Some(path.as_path()) {
            return;
        }
        cx.set_global(FileClipboard {
            paths: vec![path],
            cut,
        });
        cx.notify();
    }

    pub(super) fn paste_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.target_folder() else {
            return;
        };
        let (paths, cut) = match cx.try_global::<FileClipboard>() {
            Some(clipboard) if !clipboard.paths.is_empty() => {
                (clipboard.paths.clone(), clipboard.cut)
            }
            _ => return,
        };
        if cut {
            // A cut is pasted once; the originals are gone afterwards.
            cx.set_global(FileClipboard::default());
        }
        let destination = dir.clone();
        let job = cx.background_spawn(async move {
            paths
                .into_iter()
                .map(|source| {
                    let result = if cut {
                        file_ops::move_into(&source, &destination)
                    } else {
                        file_ops::copy_into(&source, &destination)
                    };
                    (source, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let mut last = None;
                for (source, result) in results {
                    match result {
                        Ok(target) => {
                            if cut {
                                this.follow_move(&source, &target);
                                if let Some(parent) = source.parent() {
                                    this.relist(parent.to_path_buf(), window, cx);
                                }
                            }
                            last = Some(target);
                        }
                        Err(error) => this.message = format!("粘贴失败：{error}"),
                    }
                }
                this.relist(dir, window, cx);
                if last.is_some() {
                    this.tree_selection = last;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_selection_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        let Some(path) = self.tree_selection.clone().or_else(|| self.root.clone()) else {
            return;
        };
        let text = if relative {
            self.root
                .as_deref()
                .and_then(|root| path.strip_prefix(root).ok())
                .map(|p| p.display().to_string())
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| ".".into())
        } else {
            path.display().to_string()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.message = "已复制路径".into();
        cx.notify();
    }

    pub(super) fn reveal_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.tree_selection.clone().or_else(|| self.root.clone()) {
            self.reveal_in_finder(path, cx);
        }
    }

    pub(super) fn reveal_in_finder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.background_spawn(async move {
            let mut command = if cfg!(target_os = "macos") {
                let mut command = std::process::Command::new("open");
                command.arg("-R").arg(&path);
                command
            } else {
                let mut command = std::process::Command::new("xdg-open");
                command.arg(if path.is_dir() {
                    path.as_path()
                } else {
                    path.parent().unwrap_or(&path)
                });
                command
            };
            if let Err(error) = command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                eprintln!("event=reveal_failed error={error}");
            }
        })
        .detach();
    }

    /// The Explorer's key context: shortcuts act on the selected row.
    pub(super) fn explorer_actions(&self, element: Div, cx: &mut Context<Self>) -> Div {
        element
            .key_context("Explorer")
            .on_action(cx.listener(|this, _: &NewFile, window, cx| {
                if let Some(dir) = this.target_folder() {
                    this.start_tree_edit(EditKind::NewFile(dir), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                if let Some(dir) = this.target_folder() {
                    this.start_tree_edit(EditKind::NewFolder(dir), window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Rename, window, cx| {
                if let Some(path) = this.tree_selection.clone()
                    && this.root.as_ref() != Some(&path)
                {
                    this.start_tree_edit(EditKind::Rename(path), window, cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &Delete, window, cx| this.delete_selection(window, cx)),
            )
            .on_action(cx.listener(|this, _: &CopyFiles, _, cx| this.copy_selection(false, cx)))
            .on_action(cx.listener(|this, _: &CutFiles, _, cx| this.copy_selection(true, cx)))
            .on_action(cx.listener(|this, _: &PasteFiles, window, cx| this.paste_files(window, cx)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_selection_path(false, cx)))
            .on_action(
                cx.listener(|this, _: &CopyRelativePath, _, cx| this.copy_selection_path(true, cx)),
            )
            .on_action(cx.listener(|this, _: &RevealInFinder, _, cx| this.reveal_selection(cx)))
    }

    /// Opening a document from elsewhere moves the Explorer highlight back to it.
    pub(super) fn clear_tree_selection_for(&mut self, pane: Pane) {
        if let Pane::Document(id) = pane
            && let Some(document) = self.documents.iter().find(|doc| doc.id == id)
            && self.tree_selection.as_ref() != Some(&document.path)
        {
            self.tree_selection = None;
        }
    }
}
