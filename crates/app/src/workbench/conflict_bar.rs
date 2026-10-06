//! Merge conflicts in the editor (logic in `crate::conflicts`): a conflicted file's markers
//! are found in the background after each edit, its two sides tinted (current green, incoming
//! blue), and a bar above the editor resolves the conflict at the cursor — 采用当前 / 采用传入
//! / 保留双方, one at a time or all at once — and moves between conflicts. Once no marker is
//! left, the bar offers to mark the file resolved (save, then stage it).

use super::*;
use crate::conflicts::{self, Choice, Conflict};
use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    input::{RangeDecoration, RangeDecorationCollection, RangeDecorationStyle},
};
use workspace_editor_git::{ChangeKind, WriteOperation, WriteRequest};

#[derive(Default)]
pub(super) struct ConflictDoc {
    /// The markers in the text of `version`.
    list: Arc<Vec<Conflict>>,
    version: Option<u64>,
    decorations: Option<RangeDecorationCollection>,
    task: Option<Task<()>>,
}

impl Document {
    /// How many conflicts the last scan found (`None`: not scanned yet).
    #[cfg(test)]
    pub(super) fn conflicts_found(&self) -> Option<usize> {
        self.conflicts.version.map(|_| self.conflicts.list.len())
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
        doc.conflicts.list = Arc::new(found);
        doc.conflicts.version = Some(version);
        cx.notify();
    }

    /// The active document's conflicts (as last found) and the one at the cursor.
    fn active_conflicts(
        &self,
        cx: &App,
    ) -> Option<(DocumentId, Arc<Vec<Conflict>>, Option<usize>)> {
        let id = self.active_document_id()?;
        let doc = self.document(id)?;
        let list = doc.conflicts.list.clone();
        let current = conflicts::current(&list, doc.editor.read(cx).cursor());
        Some((id, list, current))
    }

    /// Resolves the conflict at the cursor, or (`all`) every conflict, as `choice`.
    pub(super) fn resolve_conflict(
        &mut self,
        choice: Choice,
        all: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, list, current)) = self.active_conflicts(cx) else {
            return;
        };
        let Some(doc) = self.document(id) else {
            return;
        };
        // Markers found in an older text would cut the wrong bytes.
        if doc.conflicts.version != Some(doc.version) {
            return;
        }
        let editor = doc.editor.clone();
        let text = editor.read(cx).text().to_string();
        let chosen: Vec<&Conflict> = match (all, current) {
            (true, _) => list.iter().collect(),
            (false, Some(index)) => vec![&list[index]],
            (false, None) => return,
        };
        let (Some(first), Some(last)) = (chosen.first(), chosen.last()) else {
            return;
        };
        // One replacement from the first conflict to the end of the last: one undo step.
        let span = first.whole.start..last.whole.end;
        let mut replacement = String::new();
        let mut at = span.start;
        for conflict in &chosen {
            replacement.push_str(&text[at..conflict.whole.start]);
            replacement.push_str(&conflicts::resolve(&text, conflict, choice));
            at = conflict.whole.end;
        }
        let range = crate::replace::utf16_offset(&text, span.start)
            ..crate::replace::utf16_offset(&text, span.end);
        let line = first.line as u32;
        editor.update(cx, |state, cx| {
            state.replace_text_in_range(Some(range), &replacement, window, cx);
            state.set_cursor_position(lsp_types::Position::new(line, 0), window, cx);
        });
    }

    /// Moves the cursor to the previous / next conflict (wrapping around).
    pub(super) fn step_conflict(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, list, _)) = self.active_conflicts(cx) else {
            return;
        };
        let Some(editor) = self.active_editor() else {
            return;
        };
        let line = editor.read(cx).cursor_position().line as usize;
        let target = if forward {
            list.iter().find(|c| c.line > line).or(list.first())
        } else {
            list.iter().rev().find(|c| c.line < line).or(list.last())
        };
        if let Some(conflict) = target {
            self.go_to_line(conflict.line as u32, 0, window, cx);
        }
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

    /// The bar above a conflicted document.
    pub(super) fn render_conflict_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, list, current) = self.active_conflicts(cx)?;
        let doc = self.document(id)?;
        let conflicted = self.conflict_entry(&doc.path).is_some();
        if list.is_empty() && !conflicted {
            return None;
        }
        let colors = theme::colors(cx);
        let button =
            |id: &'static str, label: &'static str| Button::new(id).xsmall().ghost().label(label);
        let resolve = |id: &'static str, label: &'static str, choice: Choice, all: bool| {
            button(id, label).on_click(cx.listener(move |this, _, window, cx| {
                this.resolve_conflict(choice, all, window, cx)
            }))
        };
        let bar = h_flex()
            .id("conflict-bar")
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
            );
        if list.is_empty() {
            return Some(
                bar.child(div().flex_1().min_w_0().child("冲突已全部解决。"))
                    .child(
                        button("conflict-mark-resolved", "标记为已解决（暂存）").on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.mark_resolved(id, window, cx)
                            }),
                        ),
                    )
                    .into_any_element(),
            );
        }
        let position = current.map_or(String::new(), |i| {
            format!("第 {} / {} 处冲突", i + 1, list.len())
        });
        Some(
            bar.child(div().flex_1().min_w_0().child(position))
                .child(resolve("conflict-ours", "采用当前", Choice::Ours, false))
                .child(resolve(
                    "conflict-theirs",
                    "采用传入",
                    Choice::Theirs,
                    false,
                ))
                .child(resolve("conflict-both", "保留双方", Choice::Both, false))
                .child(div().w_2())
                .child(resolve(
                    "conflict-all-ours",
                    "全部采用当前",
                    Choice::Ours,
                    true,
                ))
                .child(resolve(
                    "conflict-all-theirs",
                    "全部采用传入",
                    Choice::Theirs,
                    true,
                ))
                .child(
                    Button::new("conflict-previous")
                        .xsmall()
                        .ghost()
                        .icon(gpui_kit::assets::IconName::ChevronUp)
                        .tooltip("上一处冲突")
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.step_conflict(false, window, cx)
                            }),
                        ),
                )
                .child(
                    Button::new("conflict-next")
                        .xsmall()
                        .ghost()
                        .icon(gpui_kit::assets::IconName::ChevronDown)
                        .tooltip("下一处冲突")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.step_conflict(true, window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }
}
