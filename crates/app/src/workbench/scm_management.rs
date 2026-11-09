//! Multi-step Git management pickers. Every mutation ends at the normal write confirmation.
use super::*;
use workspace_editor_git::{InProgress, redact_git_output};

#[derive(Clone)]
pub(in crate::workbench) enum GitFlowStep {
    PublishRemote { remote: String, url: String },
    PublishTarget { remote: String, url: String },
    AddRemoteName,
    AddRemoteUrl { name: String },
    EditRemote { name: String, old_url: String },
    RemoteDetails(String),
    Rename { name: String, oid: String },
    RenameName { name: String, oid: String },
    Delete { name: String, oid: String },
    Upstream(String),
    Continue(InProgress),
    Abort(InProgress),
    Submodule { path: PathBuf, url: String },
    OpenWorktree(PathBuf),
    RemoveWorktree { path: PathBuf, head: String },
    WorktreePath(String),
    NewWorktreePath(String),
    CloneSource,
    CreatePath { source: Option<String> },
}
impl GitFlowStep {
    pub(in crate::workbench) fn accepts_text(&self) -> bool {
        matches!(
            self,
            Self::PublishTarget { .. }
                | Self::AddRemoteName
                | Self::AddRemoteUrl { .. }
                | Self::EditRemote { .. }
                | Self::RenameName { .. }
                | Self::NewWorktreePath(_)
                | Self::CloneSource
                | Self::CreatePath { .. }
        )
    }
}
fn item(
    repo: &RepoId,
    label: impl Into<String>,
    detail: impl Into<String>,
    step: GitFlowStep,
) -> PickItem {
    PickItem {
        label: label.into(),
        detail: detail.into(),
        icon: PickIcon::Lucide(IconName::GitBranch),
        keys: Vec::new(),
        pick: Pick::GitFlow {
            repo: repo.clone(),
            step,
        },
    }
}
impl Workbench {
    fn git_text_step(
        &mut self,
        repo: &RepoId,
        title: &str,
        step: GitFlowStep,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_picker(
            vec![item(repo, "输入后按 Enter 继续", title, step)],
            title.into(),
            "",
            window,
            cx,
        );
    }
    /// Lists actual push URLs (including pushurl and insteadOf rewrites), then asks for a target.
    pub(in crate::workbench) fn open_publish_branch(
        &mut self,
        g: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g).filter(|g| !g.write_pending) else {
            return;
        };
        let repo = group.repo.clone();
        self.write_generation += 1;
        let query_generation = self.write_generation;
        let service = self.service.clone();
        let query_repo = repo.clone();
        let job = cx
            .background_spawn(async move { service.remotes(&query_repo, &AtomicBool::new(false)) });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.write_generation != query_generation
                    || this.quick_open.is_some()
                    || this.group_of(&repo.id).is_none()
                {
                    return;
                }
                match result {
                    Ok(remotes) if !remotes.is_empty() => {
                        let items = remotes
                            .into_iter()
                            .map(|remote| {
                                item(
                                    &repo.id,
                                    remote.name.clone(),
                                    redact_git_output(&format!(
                                        "实际推送地址：{}",
                                        remote.push_urls.join("\n")
                                    )),
                                    GitFlowStep::PublishRemote {
                                        remote: remote.name,
                                        url: remote.push_urls.first().cloned().unwrap_or_default(),
                                    },
                                )
                            })
                            .collect();
                        this.open_picker(
                            items,
                            "发布分支：明确选择远程".into(),
                            "没有匹配的远程",
                            window,
                            cx,
                        );
                    }
                    Ok(_) => {
                        this.message = "尚无远程，请先在「管理远程…」添加地址".into();
                        cx.notify();
                    }
                    Err(e) => {
                        this.message = redact_git_output(&format!("无法列出远程：{e}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    /// A focused list for each management area; failed queries remain visible errors.
    pub(in crate::workbench) fn open_git_management(
        &mut self,
        g: usize,
        area: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.get(g).filter(|g| !g.write_pending) else {
            return;
        };
        let repo = group.repo.clone();
        let branch = group
            .status
            .as_ref()
            .and_then(|s| s.as_ref().ok())
            .and_then(|s| s.branch.clone());
        self.write_generation += 1;
        let query_generation = self.write_generation;
        let service = self.service.clone();
        let query_repo = repo.clone();
        let job = cx.background_spawn(async move {
            let cancel = AtomicBool::new(false);
            let repo = query_repo;
            let mut items = Vec::new();
            match area {
                "remote" => {
                    items.push(item(
                        &repo.id,
                        "添加远程…",
                        "仅保存配置，不自动连接",
                        GitFlowStep::AddRemoteName,
                    ));
                    for remote in service.remotes(&repo, &cancel)? {
                        let detail = redact_git_output(&format!(
                            "远程：{}\nFetch：{}\nPush：{}",
                            remote.name,
                            remote.fetch_urls.join("\n"),
                            remote.push_urls.join("\n")
                        ));
                        items.push(item(
                            &repo.id,
                            format!("查看／复制 {}", remote.name),
                            detail.clone(),
                            GitFlowStep::RemoteDetails(detail),
                        ));
                        if let [url] = remote.fetch_urls.as_slice() {
                            items.push(item(
                                &repo.id,
                                format!("修改 {} 的 Fetch 地址…", remote.name),
                                "显式 pushurl 保持原值；多地址配置请在终端修改",
                                GitFlowStep::EditRemote {
                                    name: remote.name,
                                    old_url: url.clone(),
                                },
                            ));
                        }
                    }
                }
                "branch" => {
                    for b in service
                        .branches(&repo, &cancel)?
                        .into_iter()
                        .filter(|b| !b.remote)
                    {
                        items.push(item(
                            &repo.id,
                            format!("重命名 {}…", b.name),
                            format!("上游：{}", b.upstream.as_deref().unwrap_or("未设置")),
                            GitFlowStep::Rename {
                                name: b.name.clone(),
                                oid: b.oid.clone(),
                            },
                        ));
                        if !b.head {
                            items.push(item(
                                &repo.id,
                                format!("安全删除本地 {}…", b.name),
                                "不强制删除；远程分支保留",
                                GitFlowStep::Delete {
                                    name: b.name,
                                    oid: b.oid,
                                },
                            ));
                        }
                    }
                    if branch.as_deref().is_some_and(|b| b != "(detached)") {
                        for b in service
                            .branches(&repo, &cancel)?
                            .into_iter()
                            .filter(|b| b.remote)
                        {
                            items.push(item(
                                &repo.id,
                                format!("设置当前分支上游为 {}", b.name),
                                "拉取与推送均使用此目标",
                                GitFlowStep::Upstream(format!("refs/remotes/{}", b.name)),
                            ));
                        }
                    }
                }
                "operation" => {
                    if let Some(operation) = service.in_progress(&repo, &cancel)? {
                        items.push(item(
                            &repo.id,
                            format!("继续 {}…", operation.label()),
                            "先用现有冲突工具解决、保存并暂存冲突文件",
                            GitFlowStep::Continue(operation),
                        ));
                        items.push(item(
                            &repo.id,
                            format!("中止 {}…", operation.label()),
                            "Git 尝试恢复操作前状态，需确认",
                            GitFlowStep::Abort(operation),
                        ));
                    }
                }
                "submodule" => {
                    for module in service.submodules(&repo, &cancel)? {
                        items.push(item(
                            &repo.id,
                            format!(
                                "{}：{}",
                                if module.initialized {
                                    "更新子模块"
                                } else {
                                    "初始化子模块"
                                },
                                module.path.display()
                            ),
                            if module.conflicted {
                                "父仓库中的子模块提交存在冲突，请先解决"
                            } else if module.initialized {
                                "已初始化；更新到父仓库记录的提交"
                            } else {
                                "未初始化；可能需要连接该子模块远程"
                            },
                            GitFlowStep::Submodule {
                                path: module.path,
                                url: module.url,
                            },
                        ));
                    }
                }
                "worktree" => {
                    for tree in service.worktrees(&repo, &cancel)? {
                        let detail = format!(
                            "{}{}",
                            tree.branch.as_deref().unwrap_or("detached HEAD"),
                            if tree.locked { " · 已锁定" } else { "" }
                        );
                        items.push(item(
                            &repo.id,
                            format!("打开 {}", tree.path.display()),
                            detail.clone(),
                            GitFlowStep::OpenWorktree(tree.path.clone()),
                        ));
                        if tree.path != repo.worktree && !tree.locked {
                            items.push(item(
                                &repo.id,
                                format!("移除 {}…", tree.path.display()),
                                detail,
                                GitFlowStep::RemoveWorktree {
                                    path: tree.path,
                                    head: tree.head,
                                },
                            ));
                        }
                    }
                    for b in service
                        .branches(&repo, &cancel)?
                        .into_iter()
                        .filter(|b| !b.remote && !b.head)
                    {
                        items.push(item(
                            &repo.id,
                            format!("为 {} 创建工作树…", b.name),
                            "选择尚不存在的绝对路径",
                            GitFlowStep::WorktreePath(b.name),
                        ));
                    }
                }
                _ => return Err(std::io::Error::other("未知的 Git 管理区域")),
            }
            Ok(items)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result: std::io::Result<Vec<PickItem>> = job.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.write_generation != query_generation
                    || this.quick_open.is_some()
                    || this.group_of(&repo.id).is_none()
                {
                    return;
                }
                match result {
                    Ok(items) if items.is_empty() => {
                        this.message = match area {
                            "operation" => "没有正在进行的 merge、rebase 或 cherry-pick",
                            "submodule" => "没有子模块",
                            _ => "没有可管理的项目",
                        }
                        .into();
                        cx.notify();
                    }
                    Ok(items) => this.open_picker(
                        items,
                        format!("Git 管理 · {}", repo.worktree.display()),
                        "没有匹配项目",
                        window,
                        cx,
                    ),
                    Err(e) => {
                        this.message = redact_git_output(&format!("Git 查询失败：{e}"));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    pub(in crate::workbench) fn scm_git_flow(
        &mut self,
        repo: &RepoId,
        step: GitFlowStep,
        query: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if step.accepts_text() && query.trim().is_empty() {
            self.message = "请输入非空值后继续".into();
            cx.notify();
            return;
        }
        let operation = match step {
            GitFlowStep::PublishRemote { remote, url } => {
                self.git_text_step(
                    repo,
                    &format!("发布到 {remote}：输入远端分支名（例如 main）"),
                    GitFlowStep::PublishTarget { remote, url },
                    window,
                    cx,
                );
                return;
            }
            GitFlowStep::PublishTarget { remote, url } => WriteOperation::PublishBranch {
                remote,
                target: query,
                expected_push_url: url,
            },
            GitFlowStep::AddRemoteName => {
                self.git_text_step(
                    repo,
                    "输入远程地址（URL 或本地路径）",
                    GitFlowStep::AddRemoteUrl { name: query },
                    window,
                    cx,
                );
                return;
            }
            GitFlowStep::AddRemoteUrl { name } => WriteOperation::AddRemote { name, url: query },
            GitFlowStep::EditRemote { name, old_url } => WriteOperation::SetRemoteUrl {
                name,
                expected_url: old_url,
                url: query,
            },
            GitFlowStep::RemoteDetails(detail) => {
                cx.write_to_clipboard(ClipboardItem::new_string(detail.clone()));
                if let Some(g) = self.group_of(repo) {
                    self.groups[g].set_git_feedback("已复制远程详情", detail, FeedbackKind::Info);
                    self.groups[g].expanded = true;
                    self.rebuild_rows();
                }
                self.message = "已复制脱敏的远程详情（含实际 Push 地址）".into();
                cx.notify();
                return;
            }
            GitFlowStep::Rename { name, oid } => {
                self.git_text_step(
                    repo,
                    &format!("重命名 {name}：输入新分支名"),
                    GitFlowStep::RenameName { name, oid },
                    window,
                    cx,
                );
                return;
            }
            GitFlowStep::RenameName { name, oid } => WriteOperation::RenameBranch {
                name,
                new_name: query,
                oid,
            },
            GitFlowStep::Delete { name, oid } => WriteOperation::DeleteBranch { name, oid },
            GitFlowStep::Upstream(upstream) => {
                let Some(branch) = self
                    .group_of(repo)
                    .and_then(|g| self.groups[g].status.as_ref())
                    .and_then(|s| s.as_ref().ok())
                    .and_then(|s| s.branch.clone())
                else {
                    return;
                };
                WriteOperation::SetUpstream { branch, upstream }
            }
            GitFlowStep::Continue(operation) => WriteOperation::ContinueOperation { operation },
            GitFlowStep::Abort(operation) => WriteOperation::AbortOperation { operation },
            GitFlowStep::Submodule { path, url } => WriteOperation::UpdateSubmodule {
                path,
                expected_url: url,
            },
            GitFlowStep::OpenWorktree(path) => {
                let Some(g) = self.group_of(repo) else {
                    return;
                };
                let common_dir = self.groups[g].repo.common_dir.clone();
                let service = self.service.clone();
                self.write_generation += 1;
                let query_generation = self.write_generation;
                let job =
                    cx.background_spawn(
                        async move { service.identify(&path, &AtomicBool::new(false)) },
                    );
                cx.spawn_in(window, async move |this, cx| {
                    let result = job.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.write_generation != query_generation {
                            return;
                        }
                        match result {
                            Ok(target) if target.common_dir == common_dir => {
                                this.open_directory(target.worktree, window, cx)
                            }
                            Ok(_) => {
                                this.message = "工作树目录身份已变化，请重新列出工作树".into();
                                cx.notify();
                            }
                            Err(e) => {
                                this.message = redact_git_output(&format!("无法打开工作树：{e}"));
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
                return;
            }
            GitFlowStep::RemoveWorktree { path, head } => {
                WriteOperation::RemoveWorktree { path, head }
            }
            GitFlowStep::WorktreePath(branch) => {
                self.git_text_step(
                    repo,
                    "输入新工作树的绝对路径（目录必须尚不存在）",
                    GitFlowStep::NewWorktreePath(branch),
                    window,
                    cx,
                );
                return;
            }
            GitFlowStep::NewWorktreePath(branch) => WriteOperation::AddWorktree {
                path: query.into(),
                branch,
            },
            GitFlowStep::CloneSource => {
                self.git_text_step(
                    repo,
                    "克隆到绝对路径（目录必须尚不存在）",
                    GitFlowStep::CreatePath {
                        source: Some(query),
                    },
                    window,
                    cx,
                );
                return;
            }
            GitFlowStep::CreatePath { source } => {
                self.confirm_create_repository(query.into(), source, window, cx);
                return;
            }
        };
        self.scm_request_for(repo, operation, window, cx);
    }
    pub(in crate::workbench) fn open_create_repository(
        &mut self,
        clone: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let repo = RepoId(PathBuf::new());
        if clone {
            self.git_text_step(
                &repo,
                "克隆：输入仓库 URL 或本地路径",
                GitFlowStep::CloneSource,
                window,
                cx,
            );
        } else {
            self.git_text_step(
                &repo,
                "初始化：输入新仓库的绝对路径（目录必须尚不存在）",
                GitFlowStep::CreatePath { source: None },
                window,
                cx,
            );
        }
    }
    fn confirm_create_repository(
        &mut self,
        path: PathBuf,
        source: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let detail = redact_git_output(&format!(
            "目标：{}\n来源：{}\n只创建新目录；克隆可能连接指定远程。",
            path.display(),
            source.as_deref().unwrap_or("本地初始化")
        ));
        let answer = window.prompt(
            PromptLevel::Warning,
            if source.is_some() {
                "克隆仓库？"
            } else {
                "初始化新仓库？"
            },
            Some(&detail),
            &crate::workbench::prompt_buttons(&["创建", "取消"]),
            cx,
        );
        let service = self.service.clone();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, _, cx| {
                this.message = format!("正在创建仓库：{}…", path.display());
                cx.notify();
            });
            let result = cx
                .background_spawn(async move {
                    service.create_repository(&path, source.as_deref(), &AtomicBool::new(false))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(repo) => {
                    this.message = format!("已创建仓库：{}", repo.worktree.display());
                    this.open_directory(repo.worktree, window, cx);
                    cx.notify();
                }
                Err(e) => {
                    this.message = redact_git_output(&format!("创建仓库失败：{e}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
