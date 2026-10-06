//! Saving, closing and quitting with unsaved changes, untitled buffers, auto-save, and files
//! that change on disk while open — the workbench side of [`crate::save`].
//!
//! - **Save:** ⌘S saves the active tab (an untitled one asks for a path), ⇧⌘S saves it under a
//!   new name, ⌥⌘S saves all.
//! - **Conflicts:** a file that changed on disk since it was loaded asks 覆盖 / 重新加载 / 比较.
//! - **Closing:** closing a tab, a window or the app with unsaved changes asks
//!   「要保存对 X 的更改吗？」.
//! - **External changes:** an unedited open file follows the disk silently; an edited one gets
//!   a banner (重新加载 / 比较 / 保留我的). A file deleted on disk keeps its buffer, marked 已删除.
//! - **Agent layer:** [`buffer_text`] and [`on_buffer_saved`].

use super::{
    DiffSource, DiffTab, Document, DocumentOwner, DocumentOwners, Pane, Workbench, language_for,
};
use crate::indent::{self, Indent};
use crate::replace;
use crate::save::{self, Answer, AutoSave, DiskState, OnDisk, SaveError};
use crate::theme;
use gpui_kit::{
    EntityInputHandler,
    assets::IconName,
    component::{
        Sizable,
        button::{Button, ButtonVariants},
        h_flex,
        input::{EditorState, InputEvent, TabSize},
    },
    prelude::FluentBuilder,
    *,
};
use std::{
    collections::{BTreeSet, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};
use workspace_editor_core::DocumentId;

gpui_kit::actions!(
    file,
    [
        Save,
        SaveAs,
        SaveAll,
        NewUntitled,
        CloseEditor,
        Quit,
        ToggleLineEnding,
        AutoSaveOff,
        AutoSaveAfterDelay,
        AutoSaveOnFocusChange
    ]
);

// VS Code's 查看: 切换自动换行.
gpui_kit::actions!(view, [ToggleSoftWrap]);

/// Untitled buffers have no file, so no device / inode: they get ids from a device number
/// no real file system uses.
const UNTITLED_DEVICE: u64 = u64::MAX;
static NEXT_UNTITLED: AtomicU64 = AtomicU64::new(1);

/// All open documents of all windows, for the agent layer.
pub struct OpenDocuments(pub DocumentOwners);
impl Global for OpenDocuments {}

type SavedHook = Rc<dyn Fn(&Path, &mut App)>;
/// Callbacks run after any buffer is written to disk.
#[derive(Default)]
struct SavedHooks(Vec<SavedHook>);
impl Global for SavedHooks {}

/// The text of `path` if it is open with unsaved edits in any window (what an agent should
/// read instead of the file on disk). Remembers which version the agent saw.
pub fn buffer_text(path: &Path, cx: &mut App) -> Option<String> {
    let owners = cx.try_global::<OpenDocuments>()?.0.clone();
    let views: Vec<_> = owners
        .borrow()
        .values()
        .filter(|owner| owner.path == path)
        .filter_map(|owner| owner.view.upgrade())
        .collect();
    views.into_iter().find_map(|view| {
        view.update(cx, |this, cx| {
            let doc = this
                .documents
                .iter_mut()
                .find(|doc| doc.path == path && doc.dirty)?;
            doc.agent_read = Some(doc.version);
            Some(doc.editor.read(cx).text().to_string())
        })
    })
}

/// Runs `hook` with the path of every buffer saved to disk from now on (each workbench window
/// registers one for its agent sessions; hooks of closed windows do nothing).
pub fn on_buffer_saved(cx: &mut App, hook: impl Fn(&Path, &mut App) + 'static) {
    cx.default_global::<SavedHooks>().0.push(Rc::new(hook));
}

fn notify_saved(path: &Path, cx: &mut App) {
    let hooks = cx
        .try_global::<SavedHooks>()
        .map(|hooks| hooks.0.clone())
        .unwrap_or_default();
    for hook in hooks {
        hook(path, cx);
    }
}

/// The file on disk changed while its buffer has edits: the banner offers these.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Banner {
    Changed(DiskState),
}

/// ⌘Q and the menu's 退出: every window with unsaved changes asks in turn, then the app
/// quits. (A quit that the system starts, such as logging out, cannot be held up yet.)
pub fn quit(cx: &mut App) {
    let owners = cx.global::<OpenDocuments>().0.clone();
    let mut seen = HashSet::new();
    let windows: Vec<(AnyWindowHandle, WeakEntity<Workbench>)> = owners
        .borrow()
        .values()
        .filter(|owner| seen.insert(owner.view.entity_id()))
        .filter(|owner| {
            owner
                .view
                .upgrade()
                .is_some_and(|view| view.read(cx).documents.iter().any(|doc| doc.dirty))
        })
        .map(|owner| (owner.window, owner.view.clone()))
        .collect();
    cx.spawn(async move |cx| {
        let mut flow = save::QuitFlow::new(windows);
        while let Some((handle, view)) = flow.next() {
            let ask = handle.update(cx, |_, window, cx| {
                window.activate_window();
                view.update(cx, |this, cx| {
                    let ids = this.documents.iter().map(|doc| doc.id).collect();
                    this.confirm_close(ids, false, window, cx)
                })
            });
            match ask {
                Ok(Ok(task)) => flow.answered(task.await),
                // The window is already gone: nothing left to ask.
                _ => flow.answered(true),
            }
        }
        if flow.should_quit() {
            cx.update(|cx| {
                // Every unsaved buffer was saved or discarded: no snapshot should outlive us.
                super::recovery::forget_everything(cx);
                cx.quit();
            });
        }
    })
    .detach();
}

impl Workbench {
    pub(super) fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.iter().find(|doc| doc.id == id)
    }

    pub(super) fn document_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.iter_mut().find(|doc| doc.id == id)
    }

    fn active_document_id(&self) -> Option<DocumentId> {
        match self.active {
            Pane::Document(id) => Some(id),
            _ => None,
        }
    }

    /// Every edit: mark the buffer edited (unless it is a reload) and restart auto-save.
    pub(super) fn document_changed(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reloaded = self.reloading.remove(&id);
        let auto_save = cx.global::<crate::settings::Settings>().auto_save;
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        doc.version += 1;
        if reloaded {
            return;
        }
        doc.dirty = true;
        if auto_save == AutoSave::AfterDelay && !doc.untitled {
            let ticket = doc.auto_save.poke();
            doc.auto_save_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(save::AUTO_SAVE_DELAY).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    let due = this
                        .document(id)
                        .is_some_and(|doc| doc.dirty && !doc.deleted && doc.auto_save.fire(ticket));
                    if due {
                        this.save_document(id, false, window, cx).detach();
                    }
                });
            }));
        }
        self.schedule_snapshot(id, window, cx);
    }

    /// files.autoSave = onFocusChange: the window lost focus or another tab was chosen.
    pub(super) fn auto_save_on_focus_change(
        &mut self,
        only: Option<DocumentId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<crate::settings::Settings>().auto_save != AutoSave::OnFocusChange {
            return;
        }
        let ids: Vec<DocumentId> = self
            .documents
            .iter()
            // A file deleted on disk is only recreated by an explicit save.
            .filter(|doc| doc.dirty && !doc.untitled && !doc.saving && !doc.deleted)
            .filter(|doc| only.is_none_or(|id| doc.id == id))
            .map(|doc| doc.id)
            .collect();
        for id in ids {
            self.save_document(id, false, window, cx).detach();
        }
    }

    pub(super) fn set_auto_save(
        &mut self,
        mode: AutoSave,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.change_settings(window, cx, |settings| settings.auto_save = mode);
        self.message = match mode {
            AutoSave::Off => "自动保存：关闭",
            AutoSave::AfterDelay => "自动保存：编辑后 1 秒",
            AutoSave::OnFocusChange => "自动保存：失去焦点时",
        }
        .into();
        cx.notify();
    }

    /// The editor and its subscription for a document's text.
    pub(super) fn document_editor(
        &mut self,
        id: DocumentId,
        path: &Path,
        text: String,
        indent: Indent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<EditorState>, Subscription) {
        let (language, _) = language_for(path);
        let view = cx.weak_entity();
        let untitled = id.device == UNTITLED_DEVICE;
        let editor = cx.new(|cx| {
            // The workbench's own find widget takes ⌘F (find_widget.rs).
            let mut state = EditorState::new(window, cx)
                .language(language)
                .searchable(false)
                .tab_size(tab_size(indent))
                .soft_wrap(soft_wrap_default(language))
                .default_value(text);
            if !untitled {
                super::navigation::attach(&mut state, path, view);
            }
            state
        });
        let subscription = cx.subscribe_in(
            &editor,
            window,
            move |this: &mut Self, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.document_changed(id, window, cx);
                    this.markdown_buffer_changed(id, cx);
                    if this.active == Pane::Document(id) {
                        this.find_document_changed(cx);
                    }
                    cx.notify();
                }
            },
        );
        (editor, subscription)
    }

    /// ⌘N: an empty Untitled-N buffer; saving it asks where.
    pub(super) fn new_untitled(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DocumentId {
        let id = DocumentId {
            device: UNTITLED_DEVICE,
            inode: NEXT_UNTITLED.fetch_add(1, Ordering::Relaxed),
        };
        let path = save::untitled_name(
            self.owners
                .borrow()
                .values()
                .filter(|owner| owner.path.is_relative())
                .map(|owner| owner.path.clone()),
        );
        let indent = indent::language_default(&path);
        let (editor, subscription) =
            self.document_editor(id, &path, String::new(), indent, window, cx);
        self.documents
            .push(Document::new(id, path.clone(), editor, subscription));
        self.owners.borrow_mut().insert(
            id,
            DocumentOwner {
                path,
                view: cx.weak_entity(),
                window: window.window_handle(),
            },
        );
        self.select_pane(Pane::Document(id), window, cx);
        id
    }

    /// ⌘W: closes the active tab (asking first when it has unsaved edits); with no tab
    /// open, the window.
    pub(super) fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active {
            Pane::Document(id) => self.close_document(id, window, cx),
            Pane::Diff => self.close_preview(window, cx),
            Pane::Graph => self.close_graph(window, cx),
            Pane::Large => self.close_large(window, cx),
            Pane::Welcome => self.close_window_after_confirm(window, cx),
        }
    }

    /// ⌘S on the active tab.
    pub(super) fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_document_id() {
            self.save_document(id, false, window, cx).detach();
        }
    }

    /// ⇧⌘S on the active tab.
    pub(super) fn save_active_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_document_id() {
            self.save_as(id, window, cx).detach();
        }
    }

    /// ⌥⌘S: every edited buffer of this window, one after another. Resolves to whether all
    /// were saved.
    pub(super) fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let ids: Vec<DocumentId> = self
            .documents
            .iter()
            .filter(|doc| doc.dirty)
            .map(|doc| doc.id)
            .collect();
        self.save_each(ids, window, cx)
    }

    fn save_each(
        &mut self,
        ids: Vec<DocumentId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        cx.spawn_in(window, async move |this, cx| {
            let mut all = true;
            for id in ids {
                let Ok(task) = this.update_in(cx, |this, window, cx| {
                    this.save_document(id, false, window, cx)
                }) else {
                    return false;
                };
                all &= task.await;
            }
            all
        })
    }

    /// Writes one buffer. `overwrite` skips the check for changes on disk (覆盖). Resolves
    /// to whether the buffer was saved.
    pub(super) fn save_document(
        &mut self,
        id: DocumentId,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(doc) = self.document(id) else {
            return Task::ready(false);
        };
        if doc.untitled {
            return self.save_as(id, window, cx);
        }
        if doc.saving {
            // Another save (often auto-save) is writing: wait for it, then save what is newer.
            // Quitting with 保存 must not fail just because the two met.
            return cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(20))
                        .await;
                    match this.update(cx, |this, _| {
                        this.document(id).map(|doc| (doc.saving, doc.dirty))
                    }) {
                        Ok(Some((true, _))) => {}
                        Ok(Some((false, false))) => return true,
                        Ok(Some((false, true))) => break,
                        _ => return false,
                    }
                }
                match this.update_in(cx, |this, window, cx| {
                    this.save_document(id, overwrite, window, cx)
                }) {
                    Ok(task) => task.await,
                    Err(_) => false,
                }
            });
        }
        let text = doc.editor.read(cx).text().to_string();
        let bytes = save::encode(&text, doc.crlf, doc.bom);
        let expected = if overwrite || doc.deleted {
            None
        } else {
            doc.disk
        };
        let path = doc.path.clone();
        let version = doc.version;
        if let Some(doc) = self.document_mut(id) {
            doc.saving = true;
        }
        let target = path.clone();
        let work =
            cx.background_spawn(async move { save::write(&target, &bytes, expected.as_ref()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let next = this.update_in(cx, |this, window, cx| {
                if let Some(doc) = this.document_mut(id) {
                    doc.saving = false;
                }
                match result {
                    Ok(state) => {
                        this.saved(id, &path, state, version, window, cx);
                        None
                    }
                    Err(error) => Some(this.save_failed(id, error, window, cx)),
                }
            });
            match next {
                Ok(None) => true,
                Ok(Some(task)) => task.await,
                Err(_) => false,
            }
        })
    }

    fn saved(
        &mut self,
        id: DocumentId,
        path: &Path,
        state: DiskState,
        version: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_deleted = self.document(id).is_some_and(|doc| doc.deleted);
        if let Some(doc) = self.document_mut(id) {
            doc.disk = Some(state);
            doc.banner = None;
            doc.deleted = false;
            // Edits made while the write was running keep the tab edited.
            doc.dirty = doc.version != version;
        }
        if self.document(id).is_some_and(|doc| !doc.dirty) {
            self.forget_snapshot(id, cx);
        }
        eprintln!("event=document_saved bytes={}", state.stamp.len);
        self.message = format!(
            "已保存 {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        // Git status of the repository that holds the file; a new file also needs its folder
        // listed again (the watcher does both too, but not for files outside the workspace).
        let repos: HashSet<_> = self
            .groups
            .iter()
            .filter(|group| path.starts_with(&group.repo.worktree))
            .map(|group| group.repo.id.clone())
            .collect();
        if !repos.is_empty() {
            self.refresh_repos(Some(repos), window, cx);
        }
        if was_deleted && let Some(parent) = path.parent() {
            self.relist_folder(parent.to_path_buf(), window, cx);
        }
        self.apply_saved_settings(id, cx);
        notify_saved(path, cx);
        cx.notify();
    }

    fn save_failed(
        &mut self,
        id: DocumentId,
        error: SaveError,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(doc) = self.document(id) else {
            return Task::ready(false);
        };
        let name = doc.name();
        match error {
            SaveError::Conflict => {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "文件已在磁盘上更改",
                    Some(&format!("「{name}」已被其他程序修改。")),
                    &crate::workbench::prompt_buttons(&[
                        "覆盖",
                        "重新加载（丢弃我的修改）",
                        "比较",
                        "取消",
                    ]),
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    let choice = answer.await.ok();
                    let next = this.update_in(cx, |this, window, cx| match choice {
                        Some(0) => Some(this.save_document(id, true, window, cx)),
                        Some(1) => {
                            this.reload_from_disk(id, window, cx);
                            None
                        }
                        Some(2) => {
                            this.compare_with_disk(id, window, cx);
                            None
                        }
                        _ => None,
                    });
                    match next {
                        Ok(Some(task)) => task.await,
                        _ => false,
                    }
                })
            }
            SaveError::ReadOnly(why) => {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &format!("无法保存「{name}」"),
                    Some(&format!("{why}。可以另存为其他文件。")),
                    &crate::workbench::prompt_buttons(&["另存为…", "取消"]),
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await != Ok(0) {
                        return false;
                    }
                    match this.update_in(cx, |this, window, cx| this.save_as(id, window, cx)) {
                        Ok(task) => task.await,
                        Err(_) => false,
                    }
                })
            }
            SaveError::Io(error) => {
                self.message = format!("保存「{name}」失败：{error}");
                cx.notify();
                Task::ready(false)
            }
        }
    }

    /// ⇧⌘S (and saving an untitled buffer): asks for a path with the system panel.
    pub(super) fn save_as(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let Some(doc) = self.document(id) else {
            return Task::ready(false);
        };
        let directory = doc
            .path
            .parent()
            .filter(|dir| dir.is_absolute())
            .map(Path::to_path_buf)
            .or_else(|| self.root.clone())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("/"));
        let name = doc.name();
        let chosen = cx.prompt_for_new_path(&directory, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.message = format!("无法打开保存面板：{error}");
                        cx.notify();
                    });
                    return false;
                }
                _ => return false,
            };
            let Ok(task) =
                this.update_in(cx, |this, window, cx| this.write_as(id, path, window, cx))
            else {
                return false;
            };
            task.await
        })
    }

    /// Writes the buffer to a new `path` and makes the tab that file.
    pub(super) fn write_as(
        &mut self,
        id: DocumentId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        if self
            .documents
            .iter()
            .any(|doc| doc.id != id && doc.path == path)
        {
            self.message = format!("「{}」已在另一个标签中打开", path.display());
            cx.notify();
            return Task::ready(false);
        }
        let Some(doc) = self.document(id) else {
            return Task::ready(false);
        };
        let text = doc.editor.read(cx).text().to_string();
        let bytes = save::encode(&text, doc.crlf, doc.bom);
        let version = doc.version;
        let target = path.clone();
        let work = cx.background_spawn(async move { save::write(&target, &bytes, None) });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(state) => {
                    let (language, language_name) = language_for(&path);
                    let mut was_untitled = false;
                    if let Some(doc) = this.document_mut(id) {
                        was_untitled = doc.untitled;
                        doc.path = path.clone();
                        doc.untitled = false;
                        doc.language = language_name;
                        doc.editor
                            .update(cx, |state, cx| state.set_highlighter(language, cx));
                    }
                    // An untitled buffer only had the plain-text default; now it has a language.
                    if was_untitled {
                        this.set_indent(id, indent::language_default(&path), cx);
                        this.set_soft_wrap(id, soft_wrap_default(language), window, cx);
                    }
                    this.markdown_path_changed(id, language, cx);
                    if let Some(owner) = this.owners.borrow_mut().get_mut(&id) {
                        owner.path = path.clone();
                    }
                    this.saved(id, &path, state, version, window, cx);
                    if let Some(parent) = path.parent() {
                        this.relist_folder(parent.to_path_buf(), window, cx);
                    }
                    true
                }
                Err(error) => {
                    this.message = format!("另存为失败：{error}");
                    cx.notify();
                    false
                }
            })
            .unwrap_or(false)
        })
    }

    /// Closing `ids` (a tab, or all tabs of a window when `remove` is false and the window
    /// closes itself): asks about the edited ones and saves on 保存. Resolves to whether the
    /// close may go ahead; with `remove`, the tabs are closed then.
    pub(super) fn confirm_close(
        &mut self,
        ids: Vec<DocumentId>,
        remove: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let dirty: Vec<DocumentId> = ids
            .iter()
            .copied()
            .filter(|id| self.document(*id).is_some_and(|doc| doc.dirty))
            .collect();
        if dirty.is_empty() {
            if remove {
                for id in ids {
                    self.remove_document(id, window, cx);
                }
            }
            return Task::ready(true);
        }
        let names: Vec<String> = dirty
            .iter()
            .filter_map(|id| self.document(*id).map(Document::name))
            .collect();
        if dirty.len() == 1 {
            self.select_pane(Pane::Document(dirty[0]), window, cx);
        }
        let (title, detail, buttons) = if names.len() == 1 {
            (
                format!("要保存对“{}”的更改吗？", names[0]),
                "如果不保存，你的更改将丢失。".to_string(),
                ["保存", "不保存", "取消"],
            )
        } else {
            (
                format!("要保存对以下 {} 个文件的更改吗？", names.len()),
                format!("{}\n\n如果不保存，你的更改将丢失。", names.join("\n")),
                ["全部保存", "全部不保存", "取消"],
            )
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &title,
            Some(&detail),
            &crate::workbench::prompt_buttons(&buttons),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let answer = Answer::from_button(answer.await.ok());
            let resolved = match answer {
                Answer::Cancel => false,
                Answer::DontSave => true,
                Answer::Save => {
                    let Ok(task) =
                        this.update_in(cx, |this, window, cx| this.save_each(dirty, window, cx))
                    else {
                        return false;
                    };
                    task.await
                }
            };
            if resolved && remove {
                let _ = this.update_in(cx, |this, window, cx| {
                    for id in ids {
                        this.remove_document(id, window, cx);
                    }
                });
            }
            resolved
        })
    }

    /// The window's close button / ⌘W on the last tab: ask, then close the window.
    pub(super) fn close_window_after_confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closing {
            return;
        }
        self.closing = true;
        let ids = self.documents.iter().map(|doc| doc.id).collect();
        let ask = self.confirm_close(ids, false, window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let close = ask.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.closing = false;
                if close {
                    // Answered: the window closes without asking again, and nothing of it
                    // needs recovering.
                    let ids: Vec<DocumentId> = this.documents.iter().map(|doc| doc.id).collect();
                    for id in ids {
                        this.forget_snapshot(id, cx);
                    }
                    for doc in &mut this.documents {
                        doc.dirty = false;
                    }
                    window.remove_window();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Replaces a buffer's text as one edit that does not mark it edited, keeping the cursor
    /// where it was (reloads from disk, Search view replaces in unedited files).
    pub(super) fn set_buffer_text(
        &mut self,
        id: DocumentId,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.document(id).map(|doc| doc.editor.clone()) else {
            return;
        };
        let changed = editor.update(cx, |state, cx| {
            let old = state.text().to_string();
            if old == text {
                return false;
            }
            let (start, end, new_end) = replace::changed_span(&old, text);
            let selection = state.selected_range();
            let utf16 = replace::utf16_offset(&old, start)..replace::utf16_offset(&old, end);
            state.replace_text_in_range(Some(utf16), &text[start..new_end], window, cx);
            let keep = |offset: usize| {
                let offset = offset.min(text.len());
                (0..=offset)
                    .rev()
                    .find(|i| text.is_char_boundary(*i))
                    .unwrap_or(0)
            };
            state.set_selected_range(keep(selection.start)..keep(selection.end), cx);
            true
        });
        if changed {
            self.reloading.insert(id);
        }
    }

    /// Looks at the files of open tabs (those in `paths`, or all) and follows changes made by
    /// other programs. Our own saves are not changes: their state is recorded when written.
    pub(super) fn check_disk(
        &mut self,
        paths: Option<&BTreeSet<PathBuf>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.large_check_disk(paths, cx);
        let jobs: Vec<(DocumentId, PathBuf, DiskState)> = self
            .documents
            .iter()
            .filter(|doc| !doc.untitled && !doc.saving)
            .filter(|doc| paths.is_none_or(|paths| paths.contains(&doc.path)))
            .filter_map(|doc| Some((doc.id, doc.path.clone(), doc.disk?)))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let work = cx.background_spawn(async move {
            jobs.into_iter()
                .map(|(id, path, known)| (id, known, save::check(&path, &known)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                for (id, known, result) in results {
                    // A save (or another check) finished meanwhile: this answer compares the
                    // disk with a state that is no longer the tab's.
                    let current = this
                        .document(id)
                        .is_some_and(|doc| doc.disk == Some(known) && !doc.saving);
                    if !current {
                        continue;
                    }
                    match result {
                        Ok(on_disk) => this.on_disk_changed(id, on_disk, window, cx),
                        Err(error) => eprintln!("event=disk_check_failed error={error}"),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_disk_changed(
        &mut self,
        id: DocumentId,
        on_disk: OnDisk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        if doc.saving {
            return;
        }
        let mut snapshot = false;
        match on_disk {
            OnDisk::Same => {}
            OnDisk::Touched(state) => doc.disk = Some(state),
            OnDisk::Deleted => {
                if !doc.deleted {
                    doc.deleted = true;
                    doc.unedited_when_deleted = (!doc.dirty).then_some(doc.version);
                    // The buffer is now the only copy: closing it asks first, and it is
                    // snapshotted like an edit.
                    doc.dirty = true;
                    snapshot = true;
                }
            }
            OnDisk::Changed { state, bytes } => {
                if doc.deleted && doc.unedited_when_deleted == Some(doc.version) {
                    doc.dirty = false;
                    doc.snapshot.poke();
                    if let Some(key) = doc.snapshot_on_disk.take() {
                        super::recovery::remove_snapshot(key, cx);
                    }
                }
                doc.deleted = false;
                if doc.dirty
                    && save::encode(&doc.editor.read(cx).text().to_string(), doc.crlf, doc.bom)
                        == bytes
                {
                    // The disk now holds what the buffer has: nothing is unsaved any more.
                    doc.dirty = false;
                    doc.disk = Some(state);
                    doc.banner = None;
                    return;
                }
                if doc.dirty {
                    doc.banner = Some(Banner::Changed(state));
                    return;
                }
                let Some((text, crlf, bom)) = decode(&bytes) else {
                    doc.banner = Some(Banner::Changed(state));
                    return;
                };
                doc.disk = Some(state);
                doc.crlf = crlf;
                doc.bom = bom;
                self.set_buffer_text(id, &text, window, cx);
            }
        }
        if snapshot {
            self.schedule_snapshot(id, window, cx);
        }
    }

    /// 重新加载: the file on disk replaces the buffer, which is then unedited.
    pub(super) fn reload_from_disk(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_from_disk_at(id, None, window, cx);
    }

    /// An agent wrote the file after reading this buffer's unsaved text, so its version
    /// contains the edits: the buffer follows it, unless something was typed meanwhile (then
    /// the usual banner asks).
    pub(super) fn follow_agent_write(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document(id) else {
            return;
        };
        if doc.dirty && doc.agent_read == Some(doc.version) {
            self.reload_from_disk_at(id, Some(doc.version), window, cx);
        } else {
            let paths = BTreeSet::from([doc.path.clone()]);
            self.check_disk(Some(&paths), window, cx);
        }
    }

    /// `only_at`: reload only if the buffer is still at this version; otherwise the change
    /// goes through [`Self::check_disk`].
    fn reload_from_disk_at(
        &mut self,
        id: DocumentId,
        only_at: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.document(id).map(|doc| doc.path.clone()) else {
            return;
        };
        let work = cx.background_spawn(async move {
            let bytes = std::fs::read(&path)?;
            let stamp = crate::files::FileStamp::read(&path)?;
            Ok::<_, std::io::Error>((DiskState::of(stamp, &bytes), bytes))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(version) = only_at
                    && let Some(doc) = this.document(id)
                    && doc.version != version
                {
                    let paths = BTreeSet::from([doc.path.clone()]);
                    this.check_disk(Some(&paths), window, cx);
                    return;
                }
                match result.map(|(state, bytes)| (state, decode(&bytes))) {
                    Ok((state, Some((text, crlf, bom)))) => {
                        this.set_buffer_text(id, &text, window, cx);
                        if let Some(doc) = this.document_mut(id) {
                            doc.disk = Some(state);
                            doc.crlf = crlf;
                            doc.bom = bom;
                            doc.dirty = false;
                            doc.banner = None;
                            doc.deleted = false;
                        }
                        this.forget_snapshot(id, cx);
                    }
                    Ok((_, None)) => {
                        this.message = "磁盘上的文件不是 UTF-8 文本，未重新加载".into()
                    }
                    Err(error) => this.message = format!("重新加载失败：{error}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 保留我的: keep the buffer; the next save overwrites the file on disk.
    pub(super) fn keep_mine(&mut self, id: DocumentId, cx: &mut Context<Self>) {
        if let Some(doc) = self.document_mut(id)
            && let Some(Banner::Changed(state)) = doc.banner.take()
        {
            doc.disk = Some(state);
        }
        cx.notify();
    }

    /// 比较: the diff editor with the file on disk on the left and the buffer on the right.
    pub(super) fn compare_with_disk(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document(id) else {
            return;
        };
        let path = doc.path.clone();
        let name = doc.name();
        let mine = doc.editor.read(cx).text().to_string();
        let work = cx.background_spawn({
            let path = path.clone();
            async move {
                let bytes = std::fs::read(&path)?;
                let (disk, ..) =
                    decode(&bytes).ok_or_else(|| std::io::Error::other("不是 UTF-8 文本"))?;
                Ok::<_, std::io::Error>(save::text_patch(&disk, &mine))
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(patch) => {
                    let tab = DiffTab {
                        label: format!("{name}（磁盘上 ↔ 我的）"),
                        tooltip: format!("{} · 磁盘上的文件 ↔ 未保存的修改", path.display()),
                        path,
                        source: DiffSource::Local(patch.into()),
                    };
                    this.show_diff_tab(tab, window, cx);
                }
                Err(error) => {
                    this.message = format!("无法比较：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The status bar's LF / CRLF: converts the buffer's line breaks (one undoable edit);
    /// the next save writes them.
    pub(super) fn toggle_line_ending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.active_document_id() else {
            return;
        };
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        doc.crlf = !doc.crlf;
        let crlf = doc.crlf;
        let editor = doc.editor.clone();
        editor.update(cx, |state, cx| {
            let old = state.text().to_string();
            let new = save::convert_line_endings(&old, crlf);
            if new != old {
                let all = 0..replace::utf16_offset(&old, old.len());
                state.replace_text_in_range(Some(all), &new, window, cx);
            }
        });
        if let Some(doc) = self.document_mut(id) {
            doc.dirty = true;
        }
        cx.notify();
    }

    /// The status bar's indentation menu: changes how this buffer indents, not its text.
    pub(super) fn set_indent(&mut self, id: DocumentId, indent: Indent, cx: &mut Context<Self>) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        doc.indent = indent;
        doc.editor
            .update(cx, |state, cx| state.set_tab_size(tab_size(indent), cx));
        eprintln!(
            "event=indent_changed hard_tabs={} width={}",
            indent.hard_tabs, indent.width
        );
        cx.notify();
    }

    /// ⌥Z: wraps long lines of the active buffer or stops wrapping them (not remembered).
    pub(super) fn toggle_soft_wrap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(doc) = self.active_document_id().and_then(|id| self.document(id)) {
            self.set_soft_wrap(doc.id, !doc.soft_wrap, window, cx);
        }
    }

    fn set_soft_wrap(
        &mut self,
        id: DocumentId,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document_mut(id) else {
            return;
        };
        doc.soft_wrap = wrap;
        doc.editor
            .update(cx, |state, cx| state.set_soft_wrap(wrap, window, cx));
        eprintln!("event=soft_wrap_changed enabled={wrap}");
        cx.notify();
    }

    fn relist_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer.expanded.contains(&dir) || self.root.as_deref() == Some(dir.as_path()) {
            self.reload_directory(dir, window, cx);
        }
    }

    /// The banner over an edited tab whose file changed on disk.
    pub(super) fn render_disk_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.active_document_id()?;
        let doc = self.document(id)?;
        let colors = theme::colors(cx);
        let recovered = doc.recovered && doc.banner.is_none() && !doc.deleted;
        let (text, actions): (&str, bool) = if doc.banner.is_some() {
            ("磁盘上的文件已更改。", true)
        } else if doc.deleted {
            ("磁盘上的文件已删除。保存会重新创建它。", false)
        } else if recovered {
            ("已恢复上次异常退出前未保存的修改。", false)
        } else {
            return None;
        };
        let can_discard = recovered && !doc.untitled;
        let button =
            |id: &'static str, label: &'static str| Button::new(id).xsmall().ghost().label(label);
        Some(
            h_flex()
                .id("disk-banner")
                .w_full()
                .h(theme::BANNER_HEIGHT)
                .px_3()
                .gap_2()
                .flex_shrink_0()
                .bg(colors.banner)
                .border_b_1()
                .border_color(colors.border)
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.foreground)
                .child(
                    gpui_kit::component::Icon::new(IconName::TriangleAlert)
                        .text_color(colors.modified),
                )
                .child(div().flex_1().min_w_0().child(text))
                .when(recovered, |banner| {
                    banner
                        .child(button("recovery-keep", "保留").on_click(
                            cx.listener(move |this, _, _, cx| this.keep_recovered(id, cx)),
                        ))
                        .when(can_discard, |banner| {
                            banner.child(
                                button("recovery-discard", "放弃，用磁盘上的版本").on_click(
                                    cx.listener(move |this, _, window, cx| {
                                        this.reload_from_disk(id, window, cx)
                                    }),
                                ),
                            )
                        })
                })
                .when(actions, |banner| {
                    banner
                        .child(button("disk-reload", "重新加载").on_click(cx.listener(
                            move |this, _, window, cx| this.reload_from_disk(id, window, cx),
                        )))
                        .child(button("disk-compare", "比较").on_click(cx.listener(
                            move |this, _, window, cx| this.compare_with_disk(id, window, cx),
                        )))
                        .child(
                            button("disk-keep", "保留我的").on_click(
                                cx.listener(move |this, _, _, cx| this.keep_mine(id, cx)),
                            ),
                        )
                })
                .into_any_element(),
        )
    }
}

impl Document {
    /// A document whose text came from somewhere else than a file (an untitled buffer).
    pub(super) fn new(
        id: DocumentId,
        path: PathBuf,
        editor: Entity<EditorState>,
        subscription: Subscription,
    ) -> Self {
        let (language_id, language) = language_for(&path);
        let indent = indent::language_default(&path);
        Self {
            id,
            path,
            editor,
            dirty: false,
            readonly: false,
            bytes: 0,
            language,
            crlf: false,
            bom: false,
            indent,
            soft_wrap: soft_wrap_default(language_id),
            disk: None,
            untitled: true,
            version: 0,
            saving: false,
            deleted: false,
            unedited_when_deleted: None,
            banner: None,
            agent_read: None,
            auto_save: Default::default(),
            auto_save_task: None,
            untitled_key: None,
            snapshot: Default::default(),
            snapshot_task: None,
            snapshot_on_disk: None,
            recovered: false,
            markdown: None,
            _subscription: subscription,
        }
    }

    pub(super) fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

/// Prose wraps by default, code does not.
fn soft_wrap_default(language: &str) -> bool {
    matches!(language, "markdown" | "plain")
}

fn tab_size(indent: Indent) -> TabSize {
    TabSize {
        tab_size: indent.width,
        hard_tabs: indent.hard_tabs,
    }
}

/// File bytes as buffer text: the byte order mark is taken off (and remembered), the line
/// ending style is detected from the first line break.
fn decode(bytes: &[u8]) -> Option<(String, bool, bool)> {
    let text = std::str::from_utf8(bytes).ok()?;
    let bom = text.starts_with('\u{feff}');
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    Some((text.to_string(), replace::eol_of(text) == "\r\n", bom))
}
