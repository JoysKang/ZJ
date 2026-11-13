//! UI intent captures repository identity, paths and the displayed version before prompting.
use super::quick_open::{Pick, PickIcon, PickItem, StashAction};
use super::*;
use gpui_kit::assets::IconName;
use std::path::Path;
use workspace_editor_git::Branch;
#[path = "scm_management.rs"]
mod management_ui;
pub(super) use management_ui::GitFlowStep;

/// The 提交 button and its menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CommitMode {
    Commit,
    CommitAndPush,
    Amend,
}

/// Whether the confirmed write changes disk state that Git cannot restore afterwards.
/// Only these ask with 取消 first; everything else keeps the action on Return.
fn irreversible(operation: &WriteOperation) -> bool {
    matches!(
        operation,
        WriteOperation::Discard { .. }
            | WriteOperation::ApplyExternalPatch { .. }
            | WriteOperation::ApplyPatch { cached: false, .. }
            | WriteOperation::AbortOperation { .. }
            | WriteOperation::RemoveWorktree { .. }
    )
}

impl Workbench {
    pub(super) fn scm_paths(
        &self,
        g: usize,
        index: Option<usize>,
        side: DiffSide,
    ) -> Option<WriteRequest> {
        let group = self.groups.get(g)?;
        if group.write_pending {
            return None;
        }
        let status = group.status.as_ref()?.as_ref().ok()?;
        let mut paths = Vec::new();
        let changes = match index {
            Some(index) => std::slice::from_ref(status.changes.get(index)?),
            None => status.changes.as_slice(),
        };
        for change in changes {
            if !match side {
                DiffSide::Staged => change.staged(),
                DiffSide::Worktree => change.unstaged(),
            } {
                continue;
            }
            paths.extend(change.paths(side));
        }
        if paths.is_empty() {
            return None;
        }
        Some(WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation: match side {
                DiffSide::Staged => WriteOperation::Unstage { paths },
                DiffSide::Worktree => WriteOperation::Stage { paths },
            },
        })
    }

    /// Discard request for one change or the whole worktree group; conflicts are left out.
    pub(super) fn scm_discard(&self, g: usize, index: Option<usize>) -> Option<WriteRequest> {
        let group = self.groups.get(g)?;
        if group.write_pending {
            return None;
        }
        let status = group.status.as_ref()?.as_ref().ok()?;
        let changes = match index {
            Some(index) => std::slice::from_ref(status.changes.get(index)?),
            None => status.changes.as_slice(),
        };
        let paths: Vec<PathBuf> = changes
            .iter()
            .filter(|change| change.unstaged() && change.kind != ChangeKind::Conflict)
            .flat_map(|change| change.paths(DiffSide::Worktree))
            .collect();
        (!paths.is_empty()).then(|| WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation: WriteOperation::Discard { paths },
        })
    }

    fn dirty_in(&self, repo: &Repository, paths: &[PathBuf], cx: &Context<Self>) -> bool {
        if self.documents.iter().any(|document| {
            document.dirty
                && paths
                    .iter()
                    .any(|path| document.path == repo.worktree.join(path))
        }) {
            return true;
        }
        // Include a document owned by another window without reading or copying its buffer.
        self.owners.borrow().values().any(|owner| {
            paths
                .iter()
                .any(|path| owner.path == repo.worktree.join(path))
                && owner.view.upgrade().is_some_and(|view| {
                    // The listener already leases this entity mutably; only read other windows.
                    view.entity_id() != cx.entity_id()
                        && view
                            .read(cx)
                            .documents
                            .iter()
                            .any(|d| d.path == owner.path && d.dirty)
                })
        })
    }

    pub(super) fn request_git_write(
        &mut self,
        request: WriteRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Staging a conflicted file marks it resolved: one that still holds conflict markers
        // asks first, as in VS Code.
        let conflicted: Vec<PathBuf> = match &request.operation {
            WriteOperation::Stage { paths } => paths
                .iter()
                .filter(|path| {
                    request
                        .expected
                        .changes
                        .iter()
                        .any(|c| &c.path == *path && c.kind == ChangeKind::Conflict)
                })
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        if conflicted.is_empty() {
            self.confirm_git_write(request, window, cx);
            return;
        }
        let worktree = request.repo.worktree.clone();
        let check = cx.background_spawn(async move {
            conflicted
                .into_iter()
                .filter(|path| has_conflict_markers(&worktree.join(path)))
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let marked = check.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if marked.is_empty() {
                    this.confirm_git_write(request, window, cx);
                    return;
                }
                let names: Vec<String> = marked
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                let (buttons, stage) = crate::workbench::destructive_prompt("仍然暂存");
                let answer = window.prompt(
                    PromptLevel::Warning,
                    &format!("{} 个文件里还有冲突标记，仍要标记为已解决？", marked.len()),
                    Some(&format!(
                        "{}\n暂存后 Git 不再把它们当作冲突，标记会随提交进入历史。",
                        names.join("\n")
                    )),
                    &crate::workbench::prompt_buttons(&buttons),
                    cx,
                );
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await == Ok(stage) {
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.confirm_git_write(request, window, cx)
                        });
                    }
                })
                .detach();
            });
        })
        .detach();
    }

    /// The confirmations other writes need (discarding, dirty buffers, syncing), then the write.
    fn confirm_git_write(
        &mut self,
        request: WriteRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let warning = match &request.operation {
            WriteOperation::Stage { paths } if self.dirty_in(&request.repo, paths, cx) => Some((
                "暂存磁盘版本？".to_string(),
                "文件有未保存的编辑。本次只暂存磁盘内容，编辑缓冲区仍保留；当前版本尚不支持保存。"
                    .to_string(),
                "暂存磁盘版本",
            )),
            WriteOperation::Discard { paths } => {
                let untracked = paths
                    .iter()
                    .filter(|path| {
                        request
                            .expected
                            .changes
                            .iter()
                            .any(|change| &change.path == *path && change.new_in_worktree())
                    })
                    .count();
                let tracked = paths.len() - untracked;
                let name = |path: &PathBuf| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                // The title states the decision and its object; the detail carries the scope.
                let title = match (paths.as_slice(), untracked) {
                    ([path], 1) => format!("删除“{}”？", name(path)),
                    ([path], _) => format!("放弃“{}”中的更改？", name(path)),
                    _ => format!("放弃 {} 个文件中的全部更改？", paths.len()),
                };
                let mut detail = format!("仓库：{}\n", request.repo.worktree.display());
                if tracked > 0 {
                    detail.push_str(&format!("{tracked} 个文件恢复为暂存区版本。"));
                }
                if untracked > 0 {
                    detail.push_str(&format!(
                        "{untracked} 个未跟踪文件将被永久删除，不进废纸篓。"
                    ));
                }
                detail.push_str("此操作无法撤销；编辑缓冲区不会写回磁盘。");
                Some((
                    title,
                    detail,
                    if tracked == 0 {
                        "删除文件"
                    } else {
                        "放弃更改"
                    },
                ))
            }
            WriteOperation::ApplyExternalPatch { path, .. } => Some((
                "应用外部补丁？".into(),
                format!(
                    "仓库：{}\n文件：{}\n验证后将修改磁盘文件；不自动暂存或保存编辑缓冲区。",
                    request.repo.worktree.display(),
                    path.display()
                ),
                "应用补丁",
            )),
            WriteOperation::ApplyPatch { cached: false, .. } => Some((
                "还原所选更改？".to_string(),
                format!(
                    "仓库：{}\n工作区文件里的所选更改将恢复为暂存区版本，无法撤销；编辑缓冲区不会写回磁盘。",
                    request.repo.worktree.display()
                ),
                "还原",
            )),
            WriteOperation::Commit {
                all: true, push, ..
            } => Some((
                "没有暂存的更改。要暂存所有更改并直接提交吗？".to_string(),
                format!(
                    "仓库：{}\n{} 个文件（包括未跟踪文件）将全部暂存后提交{}。",
                    request.repo.worktree.display(),
                    request.expected.changes.len(),
                    if *push {
                        "，然后推送到上游"
                    } else {
                        ""
                    }
                ),
                if *push {
                    "全部暂存、提交并推送"
                } else {
                    "全部暂存并提交"
                },
            )),
            WriteOperation::Commit { amend: true, .. } => Some((
                "修改上次提交？".to_string(),
                format!(
                    "仓库：{}\n把暂存的更改并入上一个提交{}。如果上一个提交已经推送，之后需要强制推送。",
                    request.repo.worktree.display(),
                    if matches!(&request.operation, WriteOperation::Commit { message, .. } if message.trim().is_empty())
                    {
                        "，保留原提交信息"
                    } else {
                        "，并替换提交信息"
                    }
                ),
                "修改上次提交",
            )),
            WriteOperation::Commit { push: true, .. } => Some((
                "提交并推送？".to_string(),
                format!(
                    "仓库：{}\n提交后推送 {} → {}；仅推送当前 HEAD，不强制覆盖远程历史。",
                    request.repo.worktree.display(),
                    request.expected.branch.as_deref().unwrap_or("未知"),
                    request.expected.upstream.as_deref().unwrap_or("未配置上游")
                ),
                "提交并推送",
            )),
            WriteOperation::PublishBranch {
                remote,
                target,
                expected_push_url,
            } => Some((
                "发布当前分支？".into(),
                workspace_editor_git::redact_git_output(&format!(
                    "仓库：{}\n{} → {remote}/refs/heads/{target}\n实际推送地址：{expected_push_url}\n仅推送当前 HEAD，不强推；成功后设置并回读上游。",
                    request.repo.worktree.display(),
                    request.expected.branch.as_deref().unwrap_or("未知")
                )),
                "发布分支",
            )),
            WriteOperation::AddRemote { name, url }
            | WriteOperation::SetRemoteUrl { name, url, .. } => Some((
                format!("保存远程 {name}？"),
                workspace_editor_git::redact_git_output(&format!(
                    "仓库：{}\nFetch 地址：{url}\n显式配置的 pushurl 保持原值，请在远程详情检查实际推送地址。",
                    request.repo.worktree.display()
                )),
                "保存远程",
            )),
            WriteOperation::RenameBranch { name, new_name, .. } => Some((
                format!("重命名本地分支 {name}？"),
                format!("新名称：{new_name}\n远程分支不会重命名。"),
                "重命名",
            )),
            WriteOperation::DeleteBranch { name, .. } => Some((
                format!("安全删除本地分支 {name}？"),
                "只执行 git branch -d；未合并、正在使用的分支会拒绝删除。远程分支保留。".into(),
                "删除本地分支",
            )),
            WriteOperation::SetUpstream { branch, upstream } => Some((
                format!("设置 {branch} 的上游？"),
                format!("目标：{upstream}\n后续拉取与推送将使用此上游。"),
                "设置上游",
            )),
            WriteOperation::AbortOperation { operation } => Some((
                format!("中止 {}？", operation.label()),
                format!(
                    "仓库：{}\nGit 将尝试恢复操作前的状态；本轮冲突解决可能丢失。未保存缓冲区不会写回磁盘。",
                    request.repo.worktree.display()
                ),
                "中止操作",
            )),
            WriteOperation::ContinueOperation { operation } => Some((
                format!("继续 {}？", operation.label()),
                "请确认冲突均已解决并暂存。使用已有提交信息，不打开交互编辑器。".into(),
                "继续操作",
            )),
            WriteOperation::UpdateSubmodule { path, expected_url } => Some((
                "初始化／更新此子模块？".into(),
                workspace_editor_git::redact_git_output(&format!(
                    "仓库：{}\n子模块：{}\n来源地址：{expected_url}\n相对地址由 Git 按父仓库远程解析。更新到父仓库记录的提交，可能连接此来源；不使用 --force 或递归更新。",
                    request.repo.worktree.display(),
                    path.display()
                )),
                "初始化／更新",
            )),
            WriteOperation::AddWorktree { path, branch } => Some((
                "创建工作树？".into(),
                format!(
                    "分支：{branch}\n目录：{}\n目标必须尚不存在。",
                    path.display()
                ),
                "创建工作树",
            )),
            WriteOperation::RemoveWorktree { path, .. } => Some((
                "移除此工作树？".into(),
                format!(
                    "目录：{}\n仅删除此工作树目录，保留分支；有未提交内容、子模块或锁定时 Git 会拒绝。",
                    path.display()
                ),
                "移除工作树",
            )),
            WriteOperation::Push => Some((
                "推送当前分支？".to_string(),
                format!(
                    "仓库：{}\n{} → {}\n仅推送当前 HEAD，不强制覆盖远程历史。",
                    request.repo.worktree.display(),
                    request.expected.branch.as_deref().unwrap_or("未知"),
                    request.expected.upstream.as_deref().unwrap_or("未配置上游")
                ),
                "推送",
            )),
            WriteOperation::PushTag { name } => Some((
                format!("推送标签“{name}”？"),
                format!(
                    "仓库：{}\n推送到当前分支的远程（没有上游时用唯一的远程或 origin）；远程已有同名标签时不会覆盖。",
                    request.repo.worktree.display()
                ),
                "推送",
            )),
            // VS Code asks before syncing too (git.confirmSync).
            WriteOperation::Sync => Some((
                "同步更改？".to_string(),
                format!(
                    "仓库：{}\n此操作将从“{}”拉取 {} 个提交，再向其推送 {} 个提交；不强制覆盖远程历史。",
                    request.repo.worktree.display(),
                    request.expected.upstream.as_deref().unwrap_or("未配置上游"),
                    request.expected.behind,
                    request.expected.ahead
                ),
                "同步",
            )),
            _ => None,
        };
        if let Some((title, detail, action)) = warning {
            let (buttons, proceed) = if irreversible(&request.operation) {
                crate::workbench::destructive_prompt(action)
            } else {
                (vec![action.to_owned(), "取消".to_owned()], 0)
            };
            let answer = window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&detail),
                &crate::workbench::prompt_buttons(&buttons),
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if answer.await == Ok(proceed) {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.start_git_write(request, window, cx)
                    });
                }
            })
            .detach();
        } else {
            self.start_git_write(request, window, cx);
        }
    }

    fn start_git_write(
        &mut self,
        mut request: WriteRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self
            .groups
            .iter_mut()
            .find(|g| g.repo.id == request.repo.id)
        else {
            return;
        };
        if group.write_pending {
            return;
        }
        self.scm_repo = Some(request.repo.id.clone());
        self.write_generation += 1;
        request.generation = self.write_generation;
        let version = request.generation;
        let id = request.repo.id.clone();
        let worktree = request.repo.worktree.clone();
        let submitted_message = match &request.operation {
            WriteOperation::Commit { message, .. } => Some(message.clone()),
            _ => None,
        };
        let branch = request.expected.branch.clone().unwrap_or_default();
        let show_result = matches!(
            request.operation,
            WriteOperation::Push
                | WriteOperation::PublishBranch { .. }
                | WriteOperation::Sync
                | WriteOperation::Commit { push: true, .. }
        );
        // Progress stays in the status bar; every failed write gets one dialog.
        let (completed, progress, failed): (&str, Option<String>, String) = match &request.operation
        {
            WriteOperation::Fetch => ("抓取完成", Some("正在抓取…".into()), "抓取失败".into()),
            WriteOperation::Pull => ("拉取完成", Some("正在拉取…".into()), "拉取失败".into()),
            WriteOperation::Sync => ("同步完成", Some("正在同步…".into()), "同步失败".into()),
            WriteOperation::Checkout { branch, .. } => (
                "已签出分支",
                Some(format!("正在签出 {branch}…")),
                format!("无法签出 {branch}"),
            ),
            WriteOperation::CreateBranch { name, .. } => (
                "分支已创建",
                Some(format!("正在创建分支 {name}…")),
                format!("无法创建分支 {name}"),
            ),
            WriteOperation::CreateTag { name, push, .. } => (
                if *push {
                    "标签已创建并推送"
                } else {
                    "标签已创建"
                },
                Some(if *push {
                    format!("正在创建并推送标签 {name}…")
                } else {
                    format!("正在创建标签 {name}…")
                }),
                format!("无法创建标签 {name}"),
            ),
            WriteOperation::PushTag { name } => (
                "标签已推送",
                Some(format!("正在推送标签 {name}…")),
                format!("无法推送标签 {name}"),
            ),
            WriteOperation::DeleteTag { name, .. } => (
                "标签已删除",
                Some(format!("正在删除标签 {name}…")),
                format!("无法删除标签 {name}"),
            ),
            WriteOperation::Stage { .. } => (
                "已暂存所选更改",
                Some("正在暂存…".into()),
                "暂存失败".into(),
            ),
            WriteOperation::Unstage { .. } => (
                "已取消暂存",
                Some("正在取消暂存…".into()),
                "取消暂存失败".into(),
            ),
            WriteOperation::Discard { .. } => (
                "已放弃所选更改",
                Some("正在放弃更改…".into()),
                "放弃更改失败".into(),
            ),
            WriteOperation::Commit { push: true, .. } => (
                "已提交并推送",
                Some("正在提交并推送…".into()),
                "提交并推送失败".into(),
            ),
            WriteOperation::Commit { amend: true, .. } => (
                "已修改上次提交",
                Some("正在修改上次提交…".into()),
                "修改上次提交失败".into(),
            ),
            WriteOperation::Commit { .. } => {
                ("已提交", Some("正在提交…".into()), "提交失败".into())
            }
            WriteOperation::Push => ("推送完成", Some("正在推送…".into()), "推送失败".into()),
            WriteOperation::ApplyPatch { cached: true, .. } => (
                "暂存区已更新",
                Some("正在更新暂存区…".into()),
                "更新暂存区失败".into(),
            ),
            WriteOperation::ApplyPatch { .. } => (
                "已还原所选更改",
                Some("正在还原所选更改…".into()),
                "还原所选更改失败".into(),
            ),
            WriteOperation::StashPush { .. } => (
                "Stash 已创建",
                Some("正在创建 Stash…".into()),
                "创建 Stash 失败".into(),
            ),
            WriteOperation::StashApply { pop: true, .. } => (
                "Stash 已弹出",
                Some("正在弹出 Stash…".into()),
                "弹出 Stash 失败".into(),
            ),
            WriteOperation::StashApply { .. } => (
                "Stash 已应用",
                Some("正在应用 Stash…".into()),
                "应用 Stash 失败".into(),
            ),
            WriteOperation::StashDrop { .. } => (
                "Stash 已删除",
                Some("正在删除 Stash…".into()),
                "删除 Stash 失败".into(),
            ),
            WriteOperation::PublishBranch { .. } => (
                "分支已发布",
                Some("正在发布分支…".into()),
                "发布分支失败".into(),
            ),
            _ => (
                "Git 操作已完成",
                Some("正在执行 Git 操作…".into()),
                "Git 操作失败".into(),
            ),
        };
        let progress = progress.map(|text| {
            let name = request.repo.worktree.file_name().unwrap_or_default();
            format!("{}：{text}", name.to_string_lossy())
        });
        if let Some(text) = &progress {
            self.message = text.clone();
            self.write_progress = Some(version);
        }
        group.write_pending = true;
        // Publishing names the ref it created; other writes have no count to report, so
        // their completion label is the honest result.
        let published = match &request.operation {
            WriteOperation::PublishBranch { remote, target, .. } => {
                Some(format!("已发布到 {remote}/{target}"))
            }
            _ => None,
        };
        let service = self.service.clone();
        // Window replacement/close is blocked while writing. Do not cancel a write on refresh.
        let job =
            cx.background_spawn(async move { service.write(&request, &AtomicBool::new(false)) });
        group.write_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await.and_then(|reply| {
                if reply.repo == id && reply.generation == version {
                    Ok(reply.output)
                } else {
                    Err(std::io::Error::other("写操作的仓库或版本不匹配"))
                }
            });
            let _ = this.update_in(cx, |this, window, cx| {
                // Only the write whose line is showing may clear it: another repository's
                // write can take the status bar while this one runs.
                let owns_progress = this.write_progress == Some(version);
                if owns_progress {
                    this.write_progress = None;
                    if let Some(text) = &progress
                        && this.message == *text
                    {
                        this.message.clear();
                    }
                }
                let g = this.groups.iter().position(|group| group.repo.id == id);
                if let Some(g) = g {
                    this.groups[g].write_pending = false;
                }
                match result {
                    Ok(output) => {
                        if let Some(g) = g {
                            let unchanged = submitted_message.as_deref()
                                == Some(this.groups[g].commit_input.read(cx).value().as_ref());
                            if unchanged {
                                let input = this.groups[g].commit_input.clone();
                                input.update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            let output = String::from_utf8_lossy(&output);
                            let push_summary = workspace_editor_git::push_result_summary(&output);
                            let summary = match published {
                                Some(ref published) => published.clone(),
                                None if show_result
                                    && push_summary == "远程无变化"
                                    && (completed != "同步完成"
                                        || output.contains("Already up to date")) =>
                                {
                                    "远端已是最新".to_owned()
                                }
                                None => completed.to_owned(),
                            };
                            // While another repository is still writing, its 「正在…」 line
                            // stays; this result is shown with the others once it is free.
                            if let Some(line) = this.git_result_line(g, &summary) {
                                this.deferred_results.push(line);
                                if this.deferred_results.len() > 3 {
                                    this.deferred_results.remove(0);
                                }
                            }
                            this.release_write_results(cx);
                        }
                    }
                    Err(error) => {
                        // The dialog is global, so a failure still shows after its repository
                        // row went away.
                        let detail =
                            format!("仓库：{}\n分支：{branch}\n\n{error}", worktree.display());
                        let name = worktree
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .replace(SINGLE_LINE, "⏎");
                        let summary = if name.is_empty() {
                            failed.clone()
                        } else {
                            format!("{name}：{failed}")
                        };
                        this.report_git_failure(summary, detail, window, cx);
                        // Other repositories' finished writes still deserve their line.
                        this.release_write_results(cx);
                    }
                }
                this.refresh(window, cx);
                this.graph_reload(&id, window, cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn scm_commit(
        &mut self,
        id: RepoId,
        mode: CommitMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.iter().find(|g| g.repo.id == id) else {
            return;
        };
        let Some(Ok(status)) = &group.status else {
            return;
        };
        let message = group.commit_input.read(cx).value().to_string();
        // Amending may keep the last message; a new commit needs one.
        if mode != CommitMode::Amend && message.trim().is_empty() {
            if let Some(index) = self.groups.iter().position(|g| g.repo.id == id) {
                let input = self.groups[index].commit_input.clone();
                // Reveal the message box: the hint is useless if it stays collapsed.
                self.groups[index].expanded = true;
                self.note_git_result(index, "请输入提交消息", cx);
                self.rebuild_rows();
                input.focus_handle(cx).focus(window, cx);
            }
            return;
        }
        let request = WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation: WriteOperation::Commit {
                message,
                amend: mode == CommitMode::Amend,
                push: mode == CommitMode::CommitAndPush,
                // Nothing staged: commit everything, as VS Code's smart commit does.
                all: mode != CommitMode::Amend && !status.changes.iter().any(|c| c.staged()),
            },
        };
        self.request_git_write(request, window, cx);
    }

    /// A whole-repository write against the displayed status; `None` while another write runs
    /// or when the status did not load.
    pub(super) fn scm_request(&self, g: usize, operation: WriteOperation) -> Option<WriteRequest> {
        let group = self.groups.get(g)?;
        if group.write_pending {
            return None;
        }
        let status = group.status.as_ref()?.as_ref().ok()?;
        Some(WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation,
        })
    }

    pub(super) fn scm_request_for(
        &mut self,
        repo: &RepoId,
        operation: WriteOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = self.groups.iter().position(|g| &g.repo.id == repo) else {
            return;
        };
        match self.scm_request(g, operation) {
            Some(request) => self.request_git_write(request, window, cx),
            None => {
                self.message = "仓库正在执行 Git 操作，或状态没有加载".into();
                cx.notify();
            }
        }
    }

    pub(super) fn scm_checkout(
        &mut self,
        repo: &RepoId,
        branch: String,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scm_request_for(
            repo,
            WriteOperation::Checkout { branch, remote },
            window,
            cx,
        );
    }

    pub(super) fn scm_create_branch(
        &mut self,
        repo: &RepoId,
        name: String,
        start: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.scm_request_for(
            repo,
            WriteOperation::CreateBranch { name, start },
            window,
            cx,
        );
    }

    /// Asks for the name, then the optional message, of a tag at `commit`.
    pub(super) fn open_create_tag(
        &mut self,
        repo: &RepoId,
        commit: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let short: String = commit.chars().take(7).collect();
        let item = PickItem {
            label: String::new(),
            detail: String::new(),
            icon: PickIcon::Lucide(IconName::Tag),
            keys: Vec::new(),
            pick: Pick::TagName {
                repo: repo.clone(),
                commit,
            },
        };
        self.open_picker(
            vec![item],
            format!("新标签名称（在 {short}）"),
            "",
            window,
            cx,
        );
    }

    /// VS Code Git Graph's 删除标签…: only here, or on the remote as well.
    pub(super) fn scm_delete_tag(
        &mut self,
        repo: &RepoId,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(worktree) = self
            .groups
            .iter()
            .find(|g| &g.repo.id == repo)
            .map(|g| g.repo.worktree.clone())
        else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("删除标签“{name}”？"),
            Some(&format!(
                "仓库：{}\n同时删除远程标签时，先删远程的，成功后再删本地的。",
                worktree.display()
            )),
            &crate::workbench::prompt_buttons(&["取消", "删除本地标签", "同时删除远程标签"]),
            cx,
        );
        let repo = repo.clone();
        cx.spawn_in(window, async move |this, cx| {
            let remote = match answer.await {
                Ok(1) => false,
                Ok(2) => true,
                _ => return,
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.scm_request_for(
                    &repo,
                    WriteOperation::DeleteTag { name, remote },
                    window,
                    cx,
                )
            });
        })
        .detach();
    }

    /// Asks for the name of a branch to create at HEAD (or at `start`) and switch to.
    pub(super) fn open_create_branch_at(
        &mut self,
        g: usize,
        start: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let placeholder = match &start {
            Some(start) => format!("新分支名称（从 {}）", &start[..start.len().min(7)]),
            None => "新分支名称".to_string(),
        };
        let item = PickItem {
            label: String::new(),
            detail: String::new(),
            icon: PickIcon::Lucide(IconName::Plus),
            keys: Vec::new(),
            pick: Pick::CreateBranch {
                repo: group.repo.id.clone(),
                start,
            },
        };
        self.open_picker(vec![item], placeholder, "", window, cx);
    }

    pub(super) fn open_create_branch(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_create_branch_at(g, None, window, cx);
    }

    /// VS Code's 签出到…: 创建新分支… first, then local branches, then remote-tracking ones,
    /// each most recently committed first.
    pub(super) fn open_branch_picker(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let repo = group.repo.clone();
        let service = self.service.clone();
        let job = cx.background_spawn({
            let repo = repo.clone();
            async move { service.branches(&repo, &AtomicBool::new(false)) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let branches = job.await;
            let _ = this.update_in(cx, |this, window, cx| match branches {
                Ok(branches) => {
                    let items = branch_items(&repo.id, branches);
                    let name = repo.worktree.file_name().unwrap_or_default();
                    let placeholder = format!("选择要签出的分支（{}）", name.to_string_lossy());
                    this.open_picker(items, placeholder, "没有匹配的分支", window, cx);
                }
                Err(error) => {
                    this.message = format!("无法列出分支：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 暂存 (Stash)…: the message is typed in the picker; the two rows choose whether
    /// untracked files go too.
    pub(super) fn open_stash_push(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let items = [false, true]
            .into_iter()
            .map(|untracked| PickItem {
                label: String::new(),
                detail: String::new(),
                icon: PickIcon::Lucide(IconName::Archive),
                keys: Vec::new(),
                pick: Pick::StashPush {
                    repo: group.repo.id.clone(),
                    untracked,
                },
            })
            .collect();
        self.open_picker(items, "Stash 说明（可选）".into(), "", window, cx);
    }

    /// 应用 / 弹出 / 删除 Stash…: the stashes, newest first.
    pub(super) fn open_stash_picker(
        &mut self,
        g: usize,
        action: StashAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g) else {
            return;
        };
        let repo = group.repo.clone();
        let service = self.service.clone();
        let job = cx.background_spawn({
            let repo = repo.clone();
            async move { service.stashes(&repo, &AtomicBool::new(false)) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let stashes = job.await;
            let _ = this.update_in(cx, |this, window, cx| match stashes {
                Ok(stashes) if stashes.is_empty() => {
                    this.message = "没有 stash".into();
                    cx.notify();
                }
                Ok(stashes) => {
                    let now = crate::time::now_ms();
                    let offset = crate::time::local_offset(now);
                    let items = stashes
                        .into_iter()
                        .map(|stash| PickItem {
                            label: stash.message.replace(SINGLE_LINE, " "),
                            detail: format!(
                                "stash@{{{}}} · {}",
                                stash.index,
                                crate::time::relative_time(stash.time * 1000, now, offset)
                            ),
                            icon: PickIcon::Lucide(IconName::Archive),
                            keys: Vec::new(),
                            pick: Pick::Stash {
                                repo: repo.id.clone(),
                                index: stash.index,
                                oid: stash.oid,
                                action,
                            },
                        })
                        .collect();
                    let what = match action {
                        StashAction::Apply => "选择要应用的 stash",
                        StashAction::Pop => "选择要弹出的 stash（应用后删除）",
                        StashAction::Drop => "选择要删除的 stash",
                    };
                    let name = repo
                        .worktree
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let placeholder = format!("{what} · {name}");
                    this.open_picker(items, placeholder, "没有匹配的 stash", window, cx);
                }
                Err(error) => {
                    this.message = format!("无法列出 stash：{error}");
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Runs a chosen stash action; deleting asks first.
    pub(super) fn scm_stash_action(
        &mut self,
        repo: &RepoId,
        index: usize,
        oid: String,
        action: StashAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let operation = match action {
            StashAction::Apply | StashAction::Pop => WriteOperation::StashApply {
                index,
                oid,
                pop: action == StashAction::Pop,
            },
            StashAction::Drop => WriteOperation::StashDrop { index, oid },
        };
        if action != StashAction::Drop {
            self.scm_request_for(repo, operation, window, cx);
            return;
        }
        let (buttons, drop) = crate::workbench::destructive_prompt("删除");
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("删除 stash@{{{index}}}？"),
            Some("删除后无法从这里恢复。"),
            &crate::workbench::prompt_buttons(&buttons),
            cx,
        );
        let repo = repo.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(drop) {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.scm_request_for(&repo, operation, window, cx)
                });
            }
        })
        .detach();
    }

    pub(super) fn scm_open_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file(path.to_path_buf(), self.root.clone(), window, cx);
        let generation = self.file_generation;
        let path = path.to_path_buf();
        let inspect = path.clone();
        let root = self.root_for(&path);
        let job = cx.background_spawn(async move {
            let bytes = crate::files::read_prefix(root.as_deref(), &inspect, 1025).ok()?;
            workspace_editor_git::parse_lfs_pointer(&bytes)
        });
        cx.spawn_in(window, async move |this, cx| {
            let Some(pointer) = job.await else { return; };
            let _ = this.update_in(cx, |this, _, cx| {
                if this.file_generation != generation { return; }
                let detail = format!("Git LFS 指针（尚未下载真实内容）：{}\nSHA-256：{}\n真实内容大小：{} 字节\n当前显示指针文本；ZJ 不自动下载或还原 LFS 对象。请用外部 git lfs pull 获取真实内容后重新打开；已经还原的工作区文件可正常查看。", path.display(), pointer.oid, pointer.size);
                // The status bar shows one line; hovering it shows all of this.
                this.message = detail;
                cx.notify();
            });
        }).detach();
    }
}

/// Whether a file (its first 8 MiB) still holds a complete conflict.
fn has_conflict_markers(path: &Path) -> bool {
    crate::files::read_prefix(None, path, crate::files::MAX_FILE_BYTES as u64)
        .is_ok_and(|bytes| !crate::conflicts::find(&String::from_utf8_lossy(&bytes)).is_empty())
}

fn branch_items(repo: &RepoId, branches: Vec<Branch>) -> Vec<PickItem> {
    let local: HashSet<&str> = branches
        .iter()
        .filter(|b| !b.remote)
        .map(|b| b.name.as_str())
        .collect();
    let mut items = vec![PickItem {
        label: String::new(),
        detail: String::new(),
        icon: PickIcon::Lucide(IconName::Plus),
        keys: Vec::new(),
        pick: Pick::CreateBranch {
            repo: repo.clone(),
            start: None,
        },
    }];
    let (locals, remotes): (Vec<&Branch>, Vec<&Branch>) = branches.iter().partition(|b| !b.remote);
    for branch in locals.into_iter().chain(remotes) {
        let subject = branch.subject.replace(SINGLE_LINE, " ");
        let pick = if branch.head {
            Pick::Close
        } else if branch.remote {
            // A remote branch whose local branch already exists switches to that one.
            match branch.name.split_once('/') {
                Some((_, name)) if local.contains(name) => Pick::Checkout {
                    repo: repo.clone(),
                    branch: name.to_string(),
                    remote: false,
                },
                _ => Pick::Checkout {
                    repo: repo.clone(),
                    branch: branch.name.clone(),
                    remote: true,
                },
            }
        } else {
            Pick::Checkout {
                repo: repo.clone(),
                branch: branch.name.clone(),
                remote: false,
            }
        };
        let kind = match (branch.head, branch.remote) {
            (true, _) => "当前分支",
            (_, true) => "远程分支",
            _ => "",
        };
        let detail = [kind, &branch.short, &subject]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        items.push(PickItem {
            label: branch.name.clone(),
            detail,
            icon: PickIcon::Lucide(if branch.remote {
                IconName::Cloud
            } else {
                IconName::GitBranch
            }),
            keys: Vec::new(),
            pick,
        });
    }
    items
}

impl Workbench {
    /// The repositories added by hand to this window's folder.
    pub(super) fn extra_repos(&self, cx: &App) -> Vec<PathBuf> {
        let Some(root) = &self.root else {
            return Vec::new();
        };
        cx.global::<crate::settings::Settings>()
            .extra_repos
            .get(root.to_string_lossy().as_ref())
            .map(|repos| repos.iter().map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    /// 添加仓库…: a folder holding a repository that discovery did not find; it is kept for
    /// this workspace folder in the settings.
    pub(super) fn add_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let answer = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("添加仓库".into()),
        });
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = answer.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let identified = cx
                .background_spawn(async move { service.identify(&path, &AtomicBool::new(false)) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let repo = match identified {
                    Ok(repo) => repo,
                    Err(error) => {
                        this.message = format!("不是 Git 仓库：{error}");
                        cx.notify();
                        return;
                    }
                };
                if this.groups.iter().any(|g| g.repo.id == repo.id) {
                    this.message = format!("{} 已在列表中", repo.worktree.display());
                    cx.notify();
                    return;
                }
                let key = root.to_string_lossy().into_owned();
                let worktree = repo.worktree.to_string_lossy().into_owned();
                this.change_settings(window, cx, move |settings| {
                    let repos = settings.extra_repos.entry(key).or_default();
                    if !repos.contains(&worktree) {
                        repos.push(worktree);
                    }
                });
                this.message = format!("已添加仓库 {}", repo.worktree.display());
                this.refresh(window, cx);
            });
        })
        .detach();
    }

    /// Whether group `g` was added by hand (and so can be removed from the list).
    pub(super) fn is_extra_repo(&self, g: usize, cx: &App) -> bool {
        self.groups
            .get(g)
            .is_some_and(|group| self.extra_repos(cx).contains(&group.repo.worktree))
    }

    pub(super) fn remove_extra_repository(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(root), Some(group)) = (self.root.clone(), self.groups.get(g)) else {
            return;
        };
        let key = root.to_string_lossy().into_owned();
        let worktree = group.repo.worktree.to_string_lossy().into_owned();
        self.change_settings(window, cx, move |settings| {
            if let Some(repos) = settings.extra_repos.get_mut(&key) {
                repos.retain(|repo| *repo != worktree);
                if repos.is_empty() {
                    settings.extra_repos.remove(&key);
                }
            }
        });
        self.refresh(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::irreversible;
    use std::path::PathBuf;
    use workspace_editor_git::WriteOperation;

    #[test]
    fn only_writes_that_cannot_be_undone_ask_with_cancel_first() {
        assert!(irreversible(&WriteOperation::Discard { paths: vec![] }));
        assert!(irreversible(&WriteOperation::ApplyExternalPatch {
            path: PathBuf::new(),
            patch: String::new(),
        }));
        // Reverting selected worktree lines is irreversible; applying them to the index is not.
        assert!(irreversible(&WriteOperation::ApplyPatch {
            path: PathBuf::new(),
            patch: String::new(),
            cached: false,
        }));
        assert!(!irreversible(&WriteOperation::ApplyPatch {
            path: PathBuf::new(),
            patch: String::new(),
            cached: true,
        }));
        // Staging, committing, pushing and safe branch deletion stay on Return.
        assert!(!irreversible(&WriteOperation::Push));
        assert!(!irreversible(&WriteOperation::DeleteBranch {
            name: String::new(),
            oid: String::new(),
        }));
    }
}
