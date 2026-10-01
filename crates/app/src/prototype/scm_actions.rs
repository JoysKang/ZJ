//! UI intent captures repository identity, paths and the displayed version before prompting.
use super::*;
use std::path::Path;

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
                "暂存磁盘版本？",
                "文件有未保存的编辑。本次只暂存磁盘内容，编辑缓冲区仍保留；当前版本尚不支持保存。"
                    .to_string(),
                "暂存磁盘版本",
            )),
            WriteOperation::Discard { paths } => Some((
                "放弃工作区更改？",
                format!(
                    "仓库：{}\n{} 个路径将恢复为暂存区版本。磁盘上的更改无法撤销；编辑缓冲区不会写回磁盘。",
                    request.repo.worktree.display(),
                    paths.len()
                ),
                "放弃更改",
            )),
            WriteOperation::Push => Some((
                "推送当前分支？",
                format!(
                    "仓库：{}\n{} → {}\n仅推送当前 HEAD，不强制覆盖远程历史。",
                    request.repo.worktree.display(),
                    request.expected.branch.as_deref().unwrap_or("未知"),
                    request.expected.upstream.as_deref().unwrap_or("未配置上游")
                ),
                "推送",
            )),
            _ => None,
        };
        if let Some((title, detail, action)) = warning {
            let answer = window.prompt(
                PromptLevel::Warning,
                title,
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
        let submitted_message = match &request.operation {
            WriteOperation::Commit { message } => Some(message.clone()),
            _ => None,
        };
        let success = match request.operation {
            WriteOperation::Stage { .. } => "已暂存",
            WriteOperation::Unstage { .. } => "已取消暂存",
            WriteOperation::Discard { .. } => "已放弃磁盘更改",
            WriteOperation::Commit { .. } => "提交成功",
            WriteOperation::Push => "推送成功",
        };
        group.write_pending = true;
        group.write_message = "正在执行 Git 操作…".into();
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
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn scm_commit(
        &mut self,
        id: RepoId,
        push: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(group) = self.groups.iter().find(|g| g.repo.id == id) else {
            return;
        };
        let Some(Ok(status)) = &group.status else {
            return;
        };
        let request = WriteRequest {
            repo: group.repo.clone(),
            generation: 0,
            expected: status.clone(),
            operation: if push {
                WriteOperation::Push
            } else {
                WriteOperation::Commit {
                    message: group.commit_input.read(cx).value().to_string(),
                }
            },
        };
        self.request_git_write(request, window, cx);
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
