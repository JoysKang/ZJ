//! Who last changed the cursor's line, in the status bar (`git blame -L n,n`). Asked once the
//! cursor has rested on a line for `BLAME_DELAY`; an edited buffer is blamed as typed, so its
//! own edits show as not committed. Clicking the item copies the commit hash.
//!
//! No timer runs while the cursor stays put; a newer request cancels the Git call in flight.

use super::*;
use gpui_kit::{component::h_flex, prelude::FluentBuilder};
use workspace_editor_git::{Blame, ChangeKind};

const BLAME_DELAY: Duration = Duration::from_millis(400);
/// Edited buffers larger than this are not sent to Git for each blame.
const MAX_BLAME_TEXT: usize = 2 * 1024 * 1024;

/// The document, line (0-based) and edit version a blame is for.
type BlameKey = (DocumentId, u32, u64);

#[derive(Default)]
pub(super) struct BlameState {
    key: Option<BlameKey>,
    pub(super) shown: Option<Result<Blame, String>>,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
}

impl Workbench {
    /// The cursor moved or the buffer changed: blame the line once things rest.
    pub(super) fn schedule_blame(&mut self, cx: &mut Context<Self>) {
        let key = match (self.active, self.cursor) {
            (Pane::Document(id), Some((line, _))) => self
                .document(id)
                .filter(|doc| !doc.untitled)
                .map(|doc| (id, line, doc.version)),
            _ => None,
        };
        if key == self.blame.key {
            return;
        }
        // A different line: what was shown no longer applies.
        if key.map(|k| (k.0, k.1)) != self.blame.key.map(|k| (k.0, k.1)) {
            self.blame.shown = None;
        }
        self.blame.key = key;
        self.blame.cancel.store(true, Ordering::Relaxed);
        self.blame.task = None;
        let Some(key) = key else {
            return;
        };
        self.blame.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLAME_DELAY).await;
            let _ = this.update(cx, |this, cx| this.run_blame(key, cx));
        }));
    }

    /// The repository's status changed (a commit, a checkout): blame the line again.
    pub(super) fn refresh_blame(&mut self, cx: &mut Context<Self>) {
        self.blame.key = None;
        self.schedule_blame(cx);
    }

    fn run_blame(&mut self, key: BlameKey, cx: &mut Context<Self>) {
        let (id, line, _) = key;
        let Some(doc) = self.document(id) else {
            return;
        };
        let Some(group) = self
            .groups
            .iter()
            .filter(|g| doc.path.starts_with(&g.repo.worktree))
            .max_by_key(|g| g.repo.worktree.as_os_str().len())
        else {
            return;
        };
        let Ok(relative) = doc.path.strip_prefix(&group.repo.worktree) else {
            return;
        };
        // New, renamed and untracked files have no history under this name yet.
        let new = group
            .status
            .as_ref()
            .and_then(|s| s.as_ref().ok())
            .and_then(|s| s.changes.iter().find(|c| c.path == relative))
            .is_some_and(|c| {
                matches!(c.kind, ChangeKind::Untracked | ChangeKind::Renamed) || c.index == b'A'
            });
        if new {
            return;
        }
        let contents = doc
            .dirty
            .then(|| doc.editor.read(cx).text().to_string())
            .filter(|text| text.len() <= MAX_BLAME_TEXT);
        if doc.dirty && contents.is_none() {
            return;
        }
        let (repo, relative) = (group.repo.clone(), relative.to_path_buf());
        let service = self.service.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.blame.cancel = cancel.clone();
        let job = cx.background_spawn(async move {
            let contents = contents.map(|text| text.into_bytes());
            service
                .blame_line(
                    &repo,
                    &relative,
                    line as usize + 1,
                    contents.as_deref(),
                    &cancel,
                )
                .map_err(|e| e.to_string())
        });
        self.blame.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.blame.key == Some(key) {
                    this.blame.shown = Some(result);
                    cx.notify();
                }
            });
        }));
    }

    /// The status bar item: author and age, the commit in the tooltip.
    pub(super) fn render_blame(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = theme::colors(cx);
        let (label, tooltip, commit) = match self.blame.shown.as_ref()? {
            Ok(blame) if blame.uncommitted => (
                "尚未提交".to_string(),
                "这一行有尚未提交的更改".to_string(),
                None,
            ),
            Ok(blame) => {
                let now = workspace_editor_agent_history::now_ms();
                let offset = crate::agent_model::local_offset(now);
                let age = crate::agent_model::relative_time(blame.time * 1000, now, offset);
                let short: String = blame.commit.chars().take(7).collect();
                (
                    format!("{}，{age}", blame.author),
                    format!(
                        "{}\n{short} · {} · {age}\n点击复制提交哈希",
                        blame.summary, blame.author
                    ),
                    Some(blame.commit.clone()),
                )
            }
            Err(error) => ("blame 失败".to_string(), error.clone(), None),
        };
        Some(
            h_flex()
                .id("status-blame")
                .h_full()
                .px_2()
                .gap_1()
                .flex_shrink_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(colors.muted)
                .when(commit.is_some(), |item| {
                    item.cursor_pointer().hover(|item| item.bg(colors.hover))
                })
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::GitCommitHorizontal)
                        .size(theme::SMALL_ICON_SIZE),
                )
                .child(label)
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .when_some(commit, |item, commit| {
                    item.on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(commit.clone()));
                        this.message = format!("已复制提交 {}", &commit[..commit.len().min(7)]);
                        cx.notify();
                    }))
                })
                .into_any_element(),
        )
    }
}
