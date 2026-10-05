//! Replace in the Search view, as in VS Code:
//! - the chevron left of the query shows the replace input (with AB, 保留大小写) and 全部替换;
//! - each result shows the match struck through in red with the replacement in green;
//! - hovering a match offers 替换 / 忽略, and hovering a file 全部替换 / 忽略;
//! - clicking a match opens the file's diff (原文件 ↔ 替换后) instead of the file;
//! - 全部替换 asks first ("将在 N 个文件中替换 M 处").
//!
//! Files are rewritten through `file_ops::replace_in_file`. It works atomically and skips
//! files that changed since the search. Files open with unsaved edits are skipped. Open files
//! without edits are reloaded, so their editor undo still works. The last replace can be
//! undone file by file while nobody has touched the files since.

use super::{DiffSource, DiffTab, Pane, Workbench};
use crate::file_ops::{self, Rewritten};
use crate::files::FileStamp;
use crate::replace::{self, Replacement};
use crate::text_search::{FileMatches, MAX_MATCHES, Matcher};
use gpui_kit::{
    EntityInputHandler,
    component::input::{InputEvent, InputState},
    *,
};
use std::{ops::Range, path::PathBuf};

pub(super) struct ReplaceState {
    pub open: bool,
    pub input: Entity<InputState>,
    pub preserve_case: bool,
    pub running: bool,
    task: Option<Task<()>>,
    /// The last replace, for 撤销.
    undo: Vec<Undo>,
    /// What the last replace (or undo) did, and whether something was skipped.
    pub summary: Option<(String, bool)>,
    _subscription: Subscription,
}

struct Undo {
    path: PathBuf,
    original: Vec<u8>,
    written: FileStamp,
}

/// One file to rewrite: the spans the search found there (and their lines, for a buffer
/// with unsaved edits, whose offsets differ from the file's).
struct Job {
    path: PathBuf,
    stamp: Option<FileStamp>,
    spans: Vec<Range<usize>>,
    lines: Option<Vec<u32>>,
}

impl ReplaceState {
    pub fn new(window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("替换"));
        // The previews in the results follow the replacement as it is typed.
        let subscription = cx.subscribe(&input, |_: &mut Workbench, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            open: false,
            input,
            preserve_case: false,
            running: false,
            task: None,
            undo: Vec::new(),
            summary: None,
            _subscription: subscription,
        }
    }

    pub fn has_undo(&self) -> bool {
        !self.undo.is_empty()
    }
}

impl Workbench {
    pub(super) fn search_replacement(&self, cx: &App) -> Replacement {
        Replacement::new(
            self.search.replace.input.read(cx).value().to_string(),
            self.search.regex,
            self.search.replace.preserve_case,
        )
    }

    /// The preview of a result line with replace on: `(text, removed ranges, added ranges)`.
    pub(super) fn replace_preview(
        &self,
        preview: &str,
        ranges: &[Range<usize>],
        cx: &App,
    ) -> (String, Vec<Range<usize>>, Vec<Range<usize>>) {
        let replacement = self.search_replacement(cx);
        let finder = self.search.finder.as_ref();
        let mut text = String::with_capacity(preview.len() + 16);
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        let mut last = 0;
        for range in ranges.iter() {
            if range.start >= range.end || range.start < last {
                continue;
            }
            text.push_str(&preview[last..range.start]);
            let start = text.len();
            text.push_str(&preview[range.clone()]);
            removed.push(start..text.len());
            // Groups are taken from the preview line; the literal template is the fallback.
            let with = finder
                .and_then(|finder| finder.replacement_for(preview, range, &replacement))
                .unwrap_or_else(|| replacement.template.clone())
                .replace(['\r', '\n'], "⏎");
            let start = text.len();
            text.push_str(&with);
            added.push(start..text.len());
            last = range.end;
        }
        text.push_str(&preview[last..]);
        (text, removed, added)
    }

    pub(super) fn toggle_search_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let replace = &mut self.search.replace;
        replace.open = !replace.open;
        let input = if replace.open {
            replace.input.clone()
        } else {
            self.search.query.clone()
        };
        input.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn rebuild_after_edit(&mut self) {
        let search = &mut self.search;
        search.results.retain(|file| file.count > 0);
        search.matches = search.results.iter().map(|file| file.count).sum();
        search.rebuild_rows();
    }

    /// 忽略 on a result line: drop its matches from the results (they will not be replaced).
    pub(super) fn ignore_line(&mut self, file: usize, line: usize, cx: &mut Context<Self>) {
        let Some(found) = self.search.results.get_mut(file) else {
            return;
        };
        if line < found.lines.len() {
            let removed = found.lines.remove(line);
            found.count = found.count.saturating_sub(removed.spans.len());
        }
        if found.lines.is_empty() {
            found.count = 0;
        }
        self.rebuild_after_edit();
        cx.notify();
    }

    /// 忽略 on a file: drop all its results.
    pub(super) fn ignore_file(&mut self, file: usize, cx: &mut Context<Self>) {
        if file < self.search.results.len() {
            self.search.results.remove(file);
        }
        self.rebuild_after_edit();
        cx.notify();
    }

    fn job(found: &FileMatches, only: Option<usize>) -> Job {
        let spans = match only {
            Some(line) => found
                .lines
                .get(line)
                .map(|l| l.spans.clone())
                .unwrap_or_default(),
            None => found
                .lines
                .iter()
                .flat_map(|l| l.spans.iter().cloned())
                .collect(),
        };
        Job {
            path: found.path.clone(),
            stamp: found.stamp,
            spans,
            lines: only
                .and_then(|line| found.lines.get(line))
                .map(|l| vec![l.line]),
        }
    }

    /// 替换 on a result line.
    pub(super) fn replace_line(
        &mut self,
        file: usize,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(found) = self.search.results.get(file) {
            let job = Self::job(found, Some(line));
            self.run_replace(vec![job], window, cx);
        }
    }

    /// 全部替换 on a file.
    pub(super) fn replace_file(
        &mut self,
        file: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(found) = self.search.results.get(file) {
            let job = Self::job(found, None);
            self.run_replace(vec![job], window, cx);
        }
    }

    /// 全部替换 next to the replace input: asks first.
    pub(super) fn confirm_replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.search.results.len();
        let matches: usize = self.search.results.iter().map(|f| f.count).sum();
        if files == 0 || self.search.replace.running {
            return;
        }
        let with = self.search.replace.input.read(cx).value().to_string();
        let detail = if with.is_empty() {
            "删除匹配的文本，可以撤销。".to_string()
        } else {
            format!("替换为“{with}”，可以撤销。")
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("将在 {files} 个文件中替换 {matches} 处"),
            Some(&detail),
            &["替换", "取消"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let jobs = this
                    .search
                    .results
                    .iter()
                    .map(|found| Self::job(found, None))
                    .collect();
                this.run_replace(jobs, window, cx);
            });
        })
        .detach();
    }

    /// Why a file must not be rewritten now: it is open somewhere with unsaved edits.
    fn unsaved_elsewhere(&self, path: &std::path::Path, me: EntityId, cx: &App) -> bool {
        if self
            .documents
            .iter()
            .any(|doc| doc.path == path && doc.dirty)
        {
            return true;
        }
        self.owners
            .borrow()
            .values()
            .filter(|owner| owner.path == path)
            .filter_map(|owner| owner.view.upgrade())
            .filter(|view| view.entity_id() != me)
            .any(|view| {
                view.read(cx)
                    .documents
                    .iter()
                    .any(|doc| doc.path == path && doc.dirty)
            })
    }

    fn run_replace(&mut self, jobs: Vec<Job>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(finder), Some(matcher)) =
            (self.search.finder.clone(), self.search.matcher.clone())
        else {
            return;
        };
        if self.search.replace.running {
            return;
        }
        let replacement = self.search_replacement(cx);
        let me = cx.entity_id();
        let mut skipped: Vec<(PathBuf, String)> = Vec::new();
        // A file with unsaved edits is replaced in its buffer (which stays edited and is not
        // saved), like VS Code; the file on disk is left alone.
        let (in_buffers, jobs): (Vec<Job>, Vec<Job>) = jobs
            .into_iter()
            .filter(|job| !job.spans.is_empty())
            .partition(|job| self.unsaved_elsewhere(&job.path, me, cx));
        let mut buffer_count = 0;
        for job in &in_buffers {
            buffer_count += self.replace_in_buffers(
                &job.path,
                &finder,
                &replacement,
                job.lines.as_deref(),
                window,
                cx,
            );
            self.search.results.retain(|found| found.path != job.path);
        }
        self.search.replace.running = true;
        let work = cx.background_spawn(async move {
            jobs.into_iter()
                .map(|job| {
                    let result = file_ops::replace_in_file(
                        &job.path,
                        job.stamp.as_ref(),
                        &finder,
                        &job.spans,
                        &replacement,
                    );
                    (job.path, result)
                })
                .collect::<Vec<_>>()
        });
        self.search.replace.task = Some(cx.spawn_in(window, async move |this, cx| {
            let outcomes = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.search.replace.running = false;
                let mut undo = Vec::new();
                let (mut files, mut count) = (0, 0);
                for (path, result) in outcomes {
                    match result {
                        Ok(done) => {
                            files += 1;
                            count += done.count;
                            this.after_rewrite(&done, &matcher, window, cx);
                            undo.push(Undo {
                                path: done.path,
                                original: done.original,
                                written: done.written,
                            });
                        }
                        Err(error) => skipped.push((path, error.to_string())),
                    }
                }
                this.rebuild_after_edit();
                this.search.replace.undo = undo;
                let buffers = in_buffers.len();
                let in_editors =
                    format!("在 {buffers} 个未保存的编辑器里替换 {buffer_count} 处（尚未保存）");
                let done = match (files, buffer_count) {
                    (0, 1..) => format!("已{in_editors}"),
                    (_, 1..) => format!("已在 {files} 个文件中替换 {count} 处；另{in_editors}"),
                    _ => format!("已在 {files} 个文件中替换 {count} 处"),
                };
                this.search.replace.summary = Some(summary(done, &skipped));
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// A file was rewritten: search it again for its results, and reload its open buffers.
    fn after_rewrite(
        &mut self,
        done: &Rewritten,
        matcher: &Matcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(found) = self.search.results.iter_mut().find(|f| f.path == done.path) {
            let (lines, count) = matcher
                .search_bytes(done.text.as_bytes(), MAX_MATCHES)
                .unwrap_or_default();
            found.lines = lines;
            found.count = count;
            found.stamp = Some(done.written);
        }
        let state = crate::save::DiskState::of(done.written, done.text.as_bytes());
        self.reload_everywhere(&done.path, &done.text, state, window, cx);
    }

    /// Replaces in the buffers of `path` that have unsaved edits, in all windows: the matches
    /// of the buffer's own text (on `lines` only, for a single result line). Returns how many.
    fn replace_in_buffers(
        &mut self,
        path: &std::path::Path,
        finder: &crate::replace::Finder,
        replacement: &Replacement,
        lines: Option<&[u32]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let mut count = self.replace_in_own_buffer(path, finder, replacement, lines, window, cx);
        let others: Vec<_> = self
            .owners
            .borrow()
            .values()
            .filter(|owner| owner.path == path)
            .filter_map(|owner| Some((owner.window, owner.view.upgrade()?)))
            .filter(|(_, view)| view.entity_id() != cx.entity_id())
            .collect();
        for (handle, view) in others {
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |this, cx| {
                    count +=
                        this.replace_in_own_buffer(path, finder, replacement, lines, window, cx);
                });
            });
        }
        count
    }

    fn replace_in_own_buffer(
        &mut self,
        path: &std::path::Path,
        finder: &crate::replace::Finder,
        replacement: &Replacement,
        lines: Option<&[u32]>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let Some(editor) = self
            .documents
            .iter()
            .find(|doc| doc.path == path && doc.dirty)
            .map(|doc| doc.editor.clone())
        else {
            return 0;
        };
        editor.update(cx, |state, cx| {
            let text = state.text().to_string();
            let replacement = replacement.clone().with_eol(replace::eol_of(&text));
            let line_of = |offset: usize| text[..offset].matches('\n').count() as u32;
            let only: Vec<Range<usize>> = finder
                .find_all(&text, MAX_MATCHES)
                .into_iter()
                .filter(|range| lines.is_none_or(|lines| lines.contains(&line_of(range.start))))
                .collect();
            if only.is_empty() {
                return 0;
            }
            let (start, end) = (only[0].start, only[only.len() - 1].end);
            let (new, count) = finder.replace(&text, Some(&only), &replacement);
            let middle = &new[start..new.len() - (text.len() - end)];
            let utf16 = replace::utf16_offset(&text, start)..replace::utf16_offset(&text, end);
            state.replace_text_in_range(Some(utf16), middle, window, cx);
            count
        })
    }

    /// Puts `text` into every open, unedited buffer of `path`, in all windows.
    fn reload_everywhere(
        &mut self,
        path: &std::path::Path,
        text: &str,
        state: crate::save::DiskState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reload_clean_document(path, text, state, window, cx);
        let others: Vec<_> = self
            .owners
            .borrow()
            .values()
            .filter(|owner| owner.path == path)
            .filter_map(|owner| Some((owner.window, owner.view.upgrade()?)))
            .filter(|(_, view)| view.entity_id() != cx.entity_id())
            .collect();
        for (handle, view) in others {
            let path = path.to_path_buf();
            let text = text.to_string();
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |this, cx| {
                    this.reload_clean_document(&path, &text, state, window, cx)
                });
            });
        }
    }

    /// The file changed on disk because of a replace: an unedited buffer takes the new text
    /// as one edit (so ⌘Z still works), stays clean and records the file's new state.
    pub(super) fn reload_clean_document(
        &mut self,
        path: &std::path::Path,
        text: &str,
        state: crate::save::DiskState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .documents
            .iter()
            .find(|doc| doc.path == path && !doc.dirty)
            .map(|doc| doc.id)
        else {
            return;
        };
        // The buffer lacks the BOM the file may have (it is stripped for editing).
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        self.set_buffer_text(id, text, window, cx);
        if let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) {
            doc.disk = Some(state);
        }
    }

    /// 撤销 after a replace: restore each file nobody has touched since.
    pub(super) fn undo_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let undo = std::mem::take(&mut self.search.replace.undo);
        if undo.is_empty() || self.search.replace.running {
            return;
        }
        self.search.replace.running = true;
        let work = cx.background_spawn(async move {
            undo.into_iter()
                .map(|undo| {
                    let result = file_ops::rewrite(&undo.path, &undo.written, &undo.original);
                    (undo.path, undo.original, result)
                })
                .collect::<Vec<_>>()
        });
        self.search.replace.task = Some(cx.spawn_in(window, async move |this, cx| {
            let outcomes = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.search.replace.running = false;
                let mut restored = 0;
                let mut skipped = Vec::new();
                for (path, original, result) in outcomes {
                    match result {
                        Ok(stamp) => {
                            restored += 1;
                            let state = crate::save::DiskState::of(stamp, &original);
                            let text = String::from_utf8_lossy(&original).into_owned();
                            this.reload_everywhere(&path, &text, state, window, cx);
                        }
                        Err(error) => skipped.push((path, error.to_string())),
                    }
                }
                this.search.replace.summary =
                    Some(summary(format!("已恢复 {restored} 个文件"), &skipped));
                // The results no longer match the files: search again.
                this.schedule_search(std::time::Duration::ZERO, window, cx);
            });
        }));
        cx.notify();
    }

    /// Clicking a match with replace on: the file's diff with its replacements.
    pub(super) fn preview_replace(
        &mut self,
        file: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(found), Some(finder)) =
            (self.search.results.get(file), self.search.finder.clone())
        else {
            return;
        };
        let job = Self::job(found, None);
        let replacement = self.search_replacement(cx);
        let name = found
            .relative
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let path = job.path.clone();
        let work = cx.background_spawn(async move {
            if FileStamp::read(&job.path)?
                != job
                    .stamp
                    .ok_or_else(|| std::io::Error::other(file_ops::STALE))?
            {
                return Err(std::io::Error::other(file_ops::STALE));
            }
            let text = std::fs::read_to_string(&job.path)?;
            let replacement = replacement.with_eol(replace::eol_of(&text));
            let edits = finder
                .edits_at(&text, &job.spans, &replacement)
                .ok_or_else(|| std::io::Error::other(file_ops::STALE))?;
            Ok::<_, std::io::Error>(replace::preview_patch(&text, &edits))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(patch) => {
                    let tab = DiffTab {
                        label: format!("{name} ↔ 替换后"),
                        tooltip: format!("{} · 替换预览", path.display()),
                        path,
                        source: DiffSource::Local(patch.into()),
                    };
                    this.show_diff_tab(tab, window, cx);
                }
                Err(error) => {
                    this.message = format!("无法预览替换：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Opens `tab` as the diff preview, replacing the current one.
    pub(super) fn show_diff_tab(
        &mut self,
        tab: DiffTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_preview(window, cx);
        self.message.clear();
        self.diff.tab = Some(tab);
        self.active = Pane::Diff;
        self.update_welcome_blink(window, cx);
        self.find_update(false, cx);
        self.load_diff(window, cx);
    }
}

fn summary(done: String, skipped: &[(PathBuf, String)]) -> (String, bool) {
    if skipped.is_empty() {
        return (done, false);
    }
    let names: Vec<String> = skipped
        .iter()
        .take(3)
        .map(|(path, why)| {
            format!(
                "{}（{why}）",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })
        .collect();
    let more = if skipped.len() > 3 {
        format!(" 等 {} 个", skipped.len())
    } else {
        String::new()
    };
    (format!("{done}；跳过 {}{more}", names.join("、")), true)
}
