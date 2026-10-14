//! Merge conflicts: tinted code and per-conflict controls before each opening marker.
//! Scans run in the background. Actions bind the document, scanned version and byte anchor;
//! after all markers are removed, the resolved banner offers save followed by staging.

use super::*;
use crate::conflicts::{self, Choice, Conflict};
use gpui_kit::base::input::BeforeLineWidget;
use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{RangeDecoration, RangeDecorationCollection, RangeDecorationStyle},
};
use workspace_editor_git::{ChangeKind, WriteOperation, WriteRequest};

#[derive(Clone, Copy)]
pub(super) struct ConflictTarget {
    pub(super) document: DocumentId,
    pub(super) version: u64,
    pub(super) start: usize,
}

#[derive(Default)]
pub(super) struct ConflictDoc {
    /// The markers in the text of `version`.
    list: Arc<Vec<Conflict>>,
    version: Option<u64>,
    decorations: Option<RangeDecorationCollection>,
    task: Option<Task<()>>,
}

impl Document {
    /// Conflicts in the current version (`None`: the scan is pending).
    #[cfg(test)]
    pub(super) fn conflicts_found(&self) -> Option<usize> {
        self.conflicts
            .version
            .filter(|version| *version == self.version)
            .map(|_| self.conflicts.list.len())
    }
}

impl Workbench {
    /// The repository and relative path of `path` when Git lists it as conflicted.
    fn conflict_entry(&self, path: &std::path::Path) -> Option<(RepoId, PathBuf)> {
        self.groups.iter().find_map(|group| {
            let relative = path.strip_prefix(&group.repo.worktree).ok()?;
            let status = group.status.as_ref()?.as_ref().ok()?;
            status
                .changes
                .iter()
                .any(|c| c.kind == ChangeKind::Conflict && c.path == relative)
                .then(|| (group.repo.id.clone(), relative.to_path_buf()))
        })
    }

    /// Finds the markers again in documents that are conflicted (or still show markers) and
    /// changed since: after an edit, an open or a status refresh.
    pub(super) fn scan_conflicts(&mut self, cx: &mut Context<Self>) {
        let due: Vec<DocumentId> = self
            .documents
            .iter()
            .filter(|doc| !doc.untitled && doc.conflicts.version != Some(doc.version))
            .filter(|doc| {
                !doc.conflicts.list.is_empty() || self.conflict_entry(&doc.path).is_some()
            })
            .map(|doc| doc.id)
            .collect();
        for id in due {
            let Some(doc) = self.document_mut(id) else {
                continue;
            };
            let version = doc.version;
            let text = doc.editor.read(cx).text().to_string();
            let job = cx.background_spawn(async move { conflicts::find(&text) });
            doc.conflicts.task = Some(cx.spawn(async move |this, cx| {
                let found = job.await;
                let _ = this.update(cx, |this, cx| this.show_conflicts(id, version, found, cx));
            }));
        }
    }

    fn show_conflicts(
        &mut self,
        id: DocumentId,
        version: u64,
        found: Vec<Conflict>,
        cx: &mut Context<Self>,
    ) {
        let colors = theme::colors(cx);
        let weak = cx.weak_entity();
        let Some(doc) = self.document_mut(id).filter(|doc| doc.version == version) else {
            return;
        };
        let mut decorations = Vec::new();
        for conflict in &found {
            let fill = |range: std::ops::Range<usize>, color| {
                RangeDecoration::new(range)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color)
            };
            decorations.push(fill(
                conflict.ours_marker.start..conflict.ours.end,
                colors.conflict_ours,
            ));
            decorations.push(fill(
                conflict.theirs.start..conflict.theirs_marker.end,
                colors.conflict_theirs,
            ));
            for marker in [
                &conflict.ours_marker,
                &conflict.separator,
                &conflict.theirs_marker,
            ] {
                decorations.push(fill(marker.clone(), colors.conflict_marker));
            }
            if let Some(base) = &conflict.base {
                decorations.push(fill(base.clone(), colors.conflict_marker));
            }
        }
        match &doc.conflicts.decorations {
            Some(collection) => collection.set(decorations, cx),
            None if !decorations.is_empty() => {
                let collection = doc.editor.update(cx, |state, cx| {
                    state.create_range_decorations_collection(decorations, cx)
                });
                doc.conflicts.decorations = Some(collection);
            }
            None => {}
        }
        let widgets = found
            .iter()
            .map(|conflict| {
                let weak = weak.clone();
                let target = ConflictTarget {
                    document: id,
                    version,
                    start: conflict.whole.start,
                };
                BeforeLineWidget {
                    id: target.start as u64,
                    anchor_byte: target.start,
                    height: theme::CONFLICT_ACTION_HEIGHT,
                    render: Rc::new(move |_, cx| {
                        weak.update(cx, |this, cx| this.render_conflict_actions(target, cx))
                            .ok()
                            .flatten()
                    }),
                }
            })
            .collect();
        doc.editor
            .update(cx, |editor, cx| editor.set_before_line_widgets(widgets, cx));
        doc.conflicts.list = Arc::new(found);
        doc.conflicts.version = Some(version);
        cx.notify();
    }

    fn conflict_at(&self, target: ConflictTarget) -> Option<(&Document, &Conflict)> {
        if self.active != Pane::Document(target.document) {
            return None;
        }
        let doc = self.document(target.document)?;
        if doc.version != target.version || doc.conflicts.version != Some(target.version) {
            return None;
        }
        let conflict = doc
            .conflicts
            .list
            .iter()
            .find(|c| c.whole.start == target.start)?;
        Some((doc, conflict))
    }

    fn render_conflict_actions(
        &self,
        target: ConflictTarget,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.conflict_at(target)?;
        let colors = theme::colors(cx);
        let action = |id, label, choice| {
            Button::new((id, target.start))
                .xsmall()
                .text()
                .label(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.resolve_conflict(target, choice, window, cx)
                }))
        };
        let separator = || div().text_color(colors.muted).child("|");
        Some(
            h_flex()
                .id(("conflict-actions", target.start))
                .h(theme::CONFLICT_ACTION_HEIGHT)
                .gap_1()
                .whitespace_nowrap()
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.muted)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(action("conflict-ours", "采用当前更改", Choice::Ours))
                .child(separator())
                .child(action("conflict-theirs", "采用传入的更改", Choice::Theirs))
                .child(separator())
                .child(action("conflict-both", "保留双方更改", Choice::Both))
                .child(separator())
                .child(
                    Button::new(("conflict-compare", target.start))
                        .xsmall()
                        .text()
                        .label("比较更改")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.compare_conflict(target, window, cx)
                        })),
                )
                .test_support()
                .into_any_element(),
        )
    }

    /// Resolve precisely the clicked conflict; one replacement is one undo step.
    pub(super) fn resolve_conflict(
        &mut self,
        target: ConflictTarget,
        choice: Choice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((doc, conflict)) = self.conflict_at(target) else {
            return;
        };
        let editor = doc.editor.clone();
        let text = editor.read(cx).text().to_string();
        let replacement = conflicts::resolve(&text, conflict, choice);
        let range = crate::replace::utf16_offset(&text, conflict.whole.start)
            ..crate::replace::utf16_offset(&text, conflict.whole.end);
        let line = conflict.line as u32;
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(range), &replacement, window, cx);
            state.set_cursor_position(lsp_types::Position::new(line, 0), window, cx);
            state.focus(window, cx);
        });
    }

    /// Compare the clicked conflict's two sides from the buffer, including unsaved edits.
    fn compare_conflict(
        &mut self,
        target: ConflictTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((doc, conflict)) = self.conflict_at(target) else {
            return;
        };
        let text = doc.editor.read(cx).text().to_string();
        let ours = text[conflict.ours.clone()].to_string();
        let theirs = text[conflict.theirs.clone()].to_string();
        let path = doc.path.clone();
        let name = doc.name();
        let navigation = self.file_generation;
        let patch = cx.background_spawn(async move { crate::save::text_patch(&ours, &theirs) });
        cx.spawn_in(window, async move |this, cx| {
            let patch = patch.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.file_generation != navigation || this.conflict_at(target).is_none() {
                    return;
                }
                this.show_diff_tab(
                    DiffTab {
                        label: format!("{name}（当前 ↔ 传入）"),
                        tooltip: format!("{} · 当前更改 ↔ 传入的更改", path.display()),
                        path,
                        source: DiffSource::Local(patch.into()),
                    },
                    window,
                    cx,
                );
            });
        })
        .detach();
    }

    /// 标记为已解决: saves the file if edited, then stages it. The save changes the file the
    /// status was taken of, so the stage is checked against a status read after it.
    pub(super) fn mark_resolved(
        &mut self,
        id: DocumentId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some((repo, relative)) = self.conflict_entry(&doc.path) else {
            return;
        };
        let Some(repo) = self
            .groups
            .iter()
            .find(|g| g.repo.id == repo)
            .map(|g| g.repo.clone())
        else {
            return;
        };
        let save = doc.dirty.then(|| self.save_document(id, false, window, cx));
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            if let Some(save) = save
                && !save.await
            {
                return;
            }
            let status = {
                let repo = repo.clone();
                cx.background_spawn(
                    async move { service.status(&repo, 0, &AtomicBool::new(false)) },
                )
                .await
            };
            let _ = this.update_in(cx, |this, window, cx| match status {
                Ok(status) => {
                    let request = WriteRequest {
                        repo,
                        generation: 0,
                        expected: Arc::new(status),
                        operation: WriteOperation::Stage {
                            paths: vec![relative],
                        },
                    };
                    this.request_git_write(request, window, cx);
                }
                Err(error) => {
                    this.message = format!("无法读取仓库状态：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Only the resolved state is a banner; unresolved actions live beside their code.
    pub(super) fn render_conflict_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.active_document_id()?;
        let doc = self.document(id)?;
        if doc.conflicts.version != Some(doc.version)
            || !doc.conflicts.list.is_empty()
            || self.conflict_entry(&doc.path).is_none()
        {
            return None;
        }
        let colors = theme::colors(cx);
        Some(
            h_flex()
                .id("conflict-resolved-bar")
                .w_full()
                .h(theme::BANNER_HEIGHT)
                .px_3()
                .gap_1()
                .flex_shrink_0()
                .bg(colors.banner)
                .border_b_1()
                .border_color(colors.border)
                .text_size(theme::TEXT_CAPTION)
                .text_color(colors.foreground)
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::GitMerge)
                        .text_color(colors.conflict),
                )
                .child(div().flex_1().min_w_0().child("冲突已全部解决。"))
                .child(
                    Button::new("conflict-mark-resolved")
                        .xsmall()
                        .ghost()
                        .label("标记为已解决（暂存）")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.mark_resolved(id, window, cx)
                        })),
                )
                .into_any_element(),
        )
    }
}
