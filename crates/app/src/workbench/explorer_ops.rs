//! Explorer file operations, VS Code style: a right-click menu on every row (and on the
//! workspace header) plus keyboard shortcuts on the focused tree. Names are edited inline in
//! the row; deletion moves to the Trash after a confirmation. The filesystem work runs in the
//! background (`file_ops`); file watching then refreshes Git status and the quick-open index,
//! and the affected folders are listed again right away.

use super::{Pane, TreeRow, Workbench};
use crate::{file_ops, files::Entry};
use gpui_kit::{
    component::{
        input::{InputEvent, InputState},
        menu::{PopupMenu, PopupMenuItem},
    },
    *,
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

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
        RevealInFinder,
        FindInFolder,
        SelectAllFiles
    ]
);

/// Copied or cut Explorer entries, shared by all windows.
#[derive(Default)]
pub struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
    // Identifies the clipboard operation, including its consumed (empty) state.
    revision: Rc<()>,
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

impl Workbench {
    /// The folder new entries and pasted files go into: the selected folder, the selected
    /// file's folder, or the workspace root.
    fn target_folder(&self) -> Option<PathBuf> {
        match &self.explorer.selection {
            Some(path) => {
                let directory = self
                    .explorer
                    .rows
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
        self.explorer.set_selected(vec![path]);
        self.explorer.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn select_tree_click(
        &mut self,
        path: PathBuf,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if modifiers.shift {
            let start = self.explorer.anchor.as_ref().and_then(|anchor| {
                self.explorer
                    .rows
                    .iter()
                    .position(|row| &row.entry.path == anchor)
            });
            let end = self
                .explorer
                .rows
                .iter()
                .position(|row| row.entry.path == path);
            if let (Some(start), Some(end)) = (start, end) {
                let paths = self.explorer.rows[start.min(end)..=start.max(end)]
                    .iter()
                    .filter(|row| !row.pending && self.root.as_ref() != Some(&row.entry.path))
                    .map(|row| row.entry.path.clone())
                    .collect::<Vec<_>>();
                if !modifiers.platform {
                    self.explorer.selected.clear();
                }
                self.explorer.selected.extend(paths);
                self.explorer.selection = Some(path);
            } else {
                self.explorer.set_selected(vec![path]);
            }
        } else if modifiers.platform {
            if !self.explorer.selected.insert(path.clone()) {
                self.explorer.selected.remove(&path);
            }
            self.explorer.anchor = Some(path.clone());
            // Keep a focused row even when the last highlighted item is toggled off.
            self.explorer.selection = Some(path);
        } else {
            self.explorer.set_selected(vec![path]);
        }
        self.explorer.focus.focus(window, cx);
        cx.notify();
    }

    fn selected_file_paths(&self) -> Vec<PathBuf> {
        file_ops::top_level_paths(
            self.explorer
                .selected
                .iter()
                .filter(|path| self.root.as_ref() != Some(*path))
                .cloned(),
        )
    }

    /// The right-click menu for `path` (a row, or the workspace root for the header).
    pub(super) fn explorer_menu(&self, path: &Path, can_paste: bool, menu: PopupMenu) -> PopupMenu {
        let root = self.root.as_deref() == Some(path);
        let folder = root
            || self
                .explorer
                .rows
                .iter()
                .any(|row| row.entry.path == path && row.entry.directory);
        let menu = menu
            .action_context(self.explorer.focus.clone())
            .item(PopupMenuItem::new("新建文件…").action(Box::new(NewFile)))
            .item(PopupMenuItem::new("新建文件夹…").action(Box::new(NewFolder)))
            .item(PopupMenuItem::new(REVEAL_LABEL).action(Box::new(RevealInFinder)))
            .separator();
        // As in VS Code: folders only, in a group of its own after the everyday entries.
        let menu = if folder {
            menu.item(PopupMenuItem::new("在文件夹中查找…").action(Box::new(FindInFolder)))
                .separator()
        } else {
            menu
        };
        menu.item(
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
                .disabled(root || self.explorer.selected.len() != 1)
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
                .checked(self.explorer.show_hidden)
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
            let Some(index) = self
                .explorer
                .rows
                .iter()
                .position(|row| row.entry.path == *dir)
            else {
                return;
            };
            if !self.explorer.expanded.contains(dir) {
                self.load_directory(dir.clone(), window, cx);
            }
            let depth = self.explorer.rows[index].depth + 1;
            self.explorer.rows.insert(
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
                            .explorer
                            .edit
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
        self.explorer.edit = Some(TreeEdit {
            kind,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn cancel_tree_edit(&mut self, cx: &mut Context<Self>) {
        if self.explorer.edit.take().is_some() {
            self.explorer.rows.retain(|row| !row.pending);
            cx.notify();
        }
    }

    fn commit_tree_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.explorer.edit.take() else {
            return;
        };
        self.explorer.rows.retain(|row| !row.pending);
        let name = edit.input.read(cx).value().to_string();
        let kind = edit.kind.clone();
        if let EditKind::Rename(path) = &kind
            && path
                .file_name()
                .is_some_and(|old| old.to_string_lossy() == name.trim())
        {
            self.explorer.focus.focus(window, cx);
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
                        this.explorer.set_selected(vec![path.clone()]);
                        if opens {
                            this.open_file(path, this.root.clone(), window, cx);
                        } else {
                            this.explorer.focus.focus(window, cx);
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
        if self.explorer.expanded.contains(&dir) {
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
        self.explorer.selected = self
            .explorer
            .selected
            .iter()
            .map(|path| moved(path).unwrap_or_else(|| path.clone()))
            .collect();
        self.explorer.selection = self
            .explorer
            .selection
            .as_deref()
            .map(|path| moved(path).unwrap_or_else(|| path.to_path_buf()));
        self.explorer.anchor = self
            .explorer
            .anchor
            .as_deref()
            .map(|path| moved(path).unwrap_or_else(|| path.to_path_buf()));
        let expanded: Vec<_> = self
            .explorer
            .expanded
            .iter()
            .filter_map(|p| moved(p))
            .collect();
        self.explorer.expanded.retain(|p| !p.starts_with(old));
        self.explorer.restore_expanded.extend(expanded);
    }

    pub(super) fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_file_paths();
        if paths.is_empty() {
            return;
        }
        let title = if paths.len() == 1 {
            format!(
                "删除“{}”？",
                paths[0].file_name().unwrap_or_default().to_string_lossy()
            )
        } else {
            format!("删除选中的 {} 个项目？", paths.len())
        };
        let (trash, detail) = if cfg!(target_os = "macos") {
            ("移到废纸篓", "可以从废纸篓里恢复。")
        } else {
            ("移到回收站", "可以从回收站里恢复。")
        };
        let names = paths
            .iter()
            .take(5)
            .map(|path| {
                self.relative(path)
                    .display()
                    .to_string()
                    .replace(super::SINGLE_LINE, "⏎")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&format!("{detail}\n{names}")),
            &crate::workbench::prompt_buttons(&[trash, "取消"]),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let results = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| {
                            let result = file_ops::trash(&path);
                            (path, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let mut directories = std::collections::BTreeSet::new();
                let mut errors = Vec::new();
                for (path, result) in results {
                    if let Err(error) = result {
                        errors.push(format!("{}：{error}", path.display()));
                        continue;
                    }
                    this.explorer
                        .selected
                        .retain(|selected| !selected.starts_with(&path));
                    if this
                        .explorer
                        .selection
                        .as_ref()
                        .is_some_and(|p| p.starts_with(&path))
                    {
                        this.explorer.selection = this.explorer.selected.last().cloned();
                    }
                    if this
                        .explorer
                        .anchor
                        .as_ref()
                        .is_some_and(|p| p.starts_with(&path))
                    {
                        this.explorer.anchor = this.explorer.selection.clone();
                    }
                    // Unedited tabs close; edited buffers stay available after deletion.
                    let closing: Vec<_> = this
                        .documents
                        .iter()
                        .filter(|doc| doc.path.starts_with(&path) && !doc.dirty)
                        .map(|doc| doc.id)
                        .collect();
                    for id in closing {
                        this.remove_document(id, window, cx);
                    }
                    for doc in &mut this.documents {
                        if doc.path.starts_with(&path) {
                            doc.deleted = true;
                            doc.banner = None;
                        }
                    }
                    if let Some(parent) = path.parent() {
                        directories.insert(parent.to_path_buf());
                    }
                }
                for directory in directories {
                    this.relist(directory, window, cx);
                }
                if !errors.is_empty() {
                    this.message = format!("删除失败：{}", errors.join("\n"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        let paths = self.selected_file_paths();
        if paths.is_empty() {
            return;
        }
        cx.set_global(FileClipboard {
            paths,
            cut,
            ..Default::default()
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
            // Consume this cut once; failed paths are restored unless a newer copy replaces it.
            cx.set_global(FileClipboard::default());
        }
        self.transfer_files(paths, dir, cut, window, cx);
    }

    /// Shared by paste and Finder drops; IO runs off the render thread.
    pub(super) fn transfer_files(
        &mut self,
        paths: Vec<PathBuf>,
        dir: PathBuf,
        cut: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = file_ops::top_level_paths(paths);
        if paths.is_empty() {
            return;
        }
        let selection = self.explorer.selected.clone();
        let clipboard_revision = cx.try_global::<FileClipboard>().map(|c| c.revision.clone());
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
        let app = cx.to_async();
        cx.spawn_in(window, async move |this, cx| {
            let results = job.await;
            // The clipboard belongs to the app, even if the originating window closes.
            if cut {
                let failed: Vec<_> = results
                    .iter()
                    .filter(|(_, result)| result.is_err())
                    .map(|(source, _)| source.clone())
                    .collect();
                app.update(|cx| {
                    if !failed.is_empty()
                        && cx.try_global::<FileClipboard>().is_some_and(|clipboard| {
                            clipboard.paths.is_empty()
                                && clipboard_revision.as_ref().is_some_and(|revision| {
                                    Rc::ptr_eq(&clipboard.revision, revision)
                                })
                        })
                    {
                        cx.set_global(FileClipboard {
                            paths: failed,
                            cut: true,
                            ..Default::default()
                        });
                    }
                });
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let untouched = this.explorer.selected == selection;
                let mut targets = Vec::new();
                let mut errors = Vec::new();
                let mut directories = std::collections::BTreeSet::from([dir.clone()]);
                for (source, result) in results {
                    match result {
                        Ok(target) => {
                            if cut {
                                this.follow_move(&source, &target);
                                if let Some(parent) = source.parent() {
                                    directories.insert(parent.to_path_buf());
                                }
                            }
                            targets.push(target);
                        }
                        Err(error) => {
                            errors.push(format!("{}：{error}", source.display()));
                        }
                    }
                }
                if !targets.is_empty() {
                    this.explorer.collapsed = false;
                }
                if !targets.is_empty()
                    && !this.explorer.expanded.contains(&dir)
                    && this.explorer.rows.iter().any(|row| row.entry.path == dir)
                {
                    this.load_directory(dir.clone(), window, cx);
                    directories.remove(&dir);
                }
                for directory in directories {
                    this.relist(directory, window, cx);
                }
                if !targets.is_empty() && untouched {
                    this.explorer.set_selected(targets);
                }
                if !errors.is_empty() {
                    this.message = format!("粘贴失败：{}", errors.join("\n"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_selection_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        let mut paths: Vec<_> = self.explorer.selected.iter().cloned().collect();
        if paths.is_empty() {
            paths.extend(self.root.clone());
        }
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|path| {
                if relative {
                    self.root
                        .as_deref()
                        .and_then(|root| path.strip_prefix(root).ok())
                        .map(|p| p.display().to_string())
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| ".".into())
                } else {
                    path.display().to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.message = "已复制路径".into();
        cx.notify();
    }

    pub(super) fn reveal_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self
            .explorer
            .selection
            .clone()
            .or_else(|| self.root.clone())
        {
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
            .on_action(cx.listener(|this, _: &SelectAllFiles, _, cx| {
                if this.explorer.edit.is_none() && !this.explorer.collapsed {
                    let paths = this
                        .explorer
                        .rows
                        .iter()
                        .skip(1)
                        .filter(|row| !row.pending)
                        .map(|row| row.entry.path.clone())
                        .collect();
                    this.explorer.set_selected(paths);
                    cx.notify();
                }
            }))
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
                if this.explorer.selected.len() == 1
                    && let Some(path) = this.explorer.selected.first().cloned()
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
            .on_action(cx.listener(|this, _: &FindInFolder, window, cx| {
                // The selected folder, a selected file's folder, or the whole workspace.
                if let Some(folder) = this.target_folder() {
                    this.find_in_folder(&folder, window, cx);
                }
            }))
    }

    /// Opening a document from elsewhere moves the Explorer highlight back to it.
    pub(super) fn clear_tree_selection_for(&mut self, pane: Pane) {
        if !self.explorer.focus_on_open
            && let Pane::Document(id) = pane
            && let Some(document) = self.documents.iter().find(|doc| doc.id == id)
            && !self.explorer.selected.contains(&document.path)
        {
            self.explorer.set_selected(Vec::new());
        }
    }
}

impl super::sidebar::Explorer {
    /// The primary row and range anchor accompany the set of highlighted paths.
    pub(super) fn set_selected(&mut self, paths: Vec<PathBuf>) {
        self.anchor = paths.first().cloned();
        self.selection = paths.last().cloned();
        self.selected = paths.into_iter().collect();
    }
}
