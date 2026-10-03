//! UI intent captures repository identity, paths and the displayed version before prompting.
use super::quick_open::{Pick, PickIcon, PickItem};
use super::*;
use gpui_kit::assets::IconName;
use std::path::Path;
use workspace_editor_git::Branch;

/// The 提交 button and its menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CommitMode {
    Commit,
    CommitAndPush,
    Amend,
}

impl Prototype {
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
                        request.expected.changes.iter().any(|change| {
                            &change.path == *path && change.kind == ChangeKind::Untracked
                        })
                    })
                    .count();
                let tracked = paths.len() - untracked;
                let name = |path: &PathBuf| {
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                let title = match (paths.as_slice(), untracked) {
                    ([path], 1) => format!("确定要删除“{}”吗？", name(path)),
                    ([path], _) => format!("确定要放弃“{}”中的更改吗？", name(path)),
                    _ => format!("确定要放弃 {} 个文件中的全部更改吗？", paths.len()),
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
            let answer = window.prompt(
                PromptLevel::Warning,
                &title,
                Some(&detail),
                &["取消", action],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                if answer.await == Ok(1) {
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
        // As in VS Code, a successful operation just shows up in the refreshed list.
        let success = match request.operation {
            WriteOperation::Push | WriteOperation::Commit { push: true, .. } => "已推送",
            _ => "",
        };
        // Started from the repository header or the status bar, where the commit box (and
        // the message under it) may not be showing: progress goes to the status bar and a
        // failure to a dialog.
        let progress = match &request.operation {
            WriteOperation::Fetch => Some(("正在抓取…".to_string(), "抓取失败".to_string())),
            WriteOperation::Pull => Some(("正在拉取…".into(), "拉取失败".into())),
            WriteOperation::Sync => Some(("正在同步…".into(), "同步失败".into())),
            WriteOperation::Checkout { branch, .. } => {
                Some((format!("正在签出 {branch}…"), format!("无法签出 {branch}")))
            }
            WriteOperation::CreateBranch { name, .. } => Some((
                format!("正在创建分支 {name}…"),
                format!("无法创建分支 {name}"),
            )),
            WriteOperation::CreateTag { name, push, .. } => Some((
                if *push {
                    format!("正在创建并推送标签 {name}…")
                } else {
                    format!("正在创建标签 {name}…")
                },
                format!("无法创建标签 {name}"),
            )),
            WriteOperation::PushTag { name } => Some((
                format!("正在推送标签 {name}…"),
                format!("无法推送标签 {name}"),
            )),
            WriteOperation::DeleteTag { name, .. } => Some((
                format!("正在删除标签 {name}…"),
                format!("无法删除标签 {name}"),
            )),
            _ => None,
        }
        .map(|(text, failed)| {
            let name = request.repo.worktree.file_name().unwrap_or_default();
            (format!("{}：{text}", name.to_string_lossy()), failed)
        });
        if let Some((text, _)) = &progress {
            self.message = text.clone();
        }
        group.write_pending = true;
        group.write_message.clear();
        let service = self.service.clone();
        // Window replacement/close is blocked while writing. Do not cancel a write on refresh.
        let job =
            cx.background_spawn(async move { service.write(&request, &AtomicBool::new(false)) });
        group.write_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await.and_then(|reply| {
                if reply.repo == id && reply.generation == version {
                    Ok(())
                } else {
                    Err(std::io::Error::other("写操作的仓库或版本不匹配"))
                }
            });
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some((text, failed)) = &progress {
                    if this.message == *text {
                        this.message.clear();
                    }
                    if let Err(error) = &result {
                        let detail = format!("仓库：{}\n{error}", worktree.display());
                        // The answer does not matter; the dialog is informational.
                        let _answer = window.prompt(
                            PromptLevel::Critical,
                            failed,
                            Some(&detail),
                            &["确定"],
                            cx,
                        );
                    }
                }
                if let Some(group) = this.groups.iter_mut().find(|g| g.repo.id == id) {
                    group.write_pending = false;
                    group.write_message = match result {
                        Ok(()) => {
                            if submitted_message.as_deref()
                                == Some(group.commit_input.read(cx).value().as_ref())
                            {
                                group
                                    .commit_input
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                            }
                            success.into()
                        }
                        Err(error) => error.to_string(),
                    };
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
            let index = self.groups.iter().position(|g| g.repo.id == id);
            if let Some(group) = index.map(|index| &mut self.groups[index]) {
                group.write_message = "请输入提交消息".into();
                cx.notify();
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
            &["取消", "删除本地标签", "同时删除远程标签"],
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

    pub(super) fn scm_open_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_file(path.to_path_buf(), self.root.clone(), window, cx);
    }
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
            pick,
        });
    }
    items
}
