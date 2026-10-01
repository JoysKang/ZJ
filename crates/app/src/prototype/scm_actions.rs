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
        let submitted_message = match &request.operation {
            WriteOperation::Commit { message } => Some(message.clone()),
            _ => None,
        };
        // As in VS Code, a successful operation just shows up in the refreshed list.
        let success = match request.operation {
            WriteOperation::Push => "已推送",
            _ => "",
        };
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
        let message = group.commit_input.read(cx).value().to_string();
        if !push && message.trim().is_empty() {
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
            operation: if push {
                WriteOperation::Push
            } else {
                WriteOperation::Commit { message }
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
