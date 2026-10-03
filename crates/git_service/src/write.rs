//! Disk/index writes. A UI snapshot is checked again under the same lock as queries.
use super::*;
use std::{
    hash::{Hash, Hasher},
    os::unix::fs::MetadataExt,
};

#[derive(Clone, Debug)]
pub enum WriteOperation {
    Stage {
        paths: Vec<PathBuf>,
    },
    Unstage {
        paths: Vec<PathBuf>,
    },
    Discard {
        paths: Vec<PathBuf>,
    },
    /// `amend` replaces the last commit (an empty message keeps its message); `push` pushes
    /// the new commit to the configured upstream under the same lock, never forced; `all`
    /// stages every change first (VS Code's smart commit when nothing is staged).
    Commit {
        message: String,
        amend: bool,
        push: bool,
        all: bool,
    },
    Push,
    /// `git fetch --all --prune`: remote-tracking branches only, the worktree is untouched.
    Fetch,
    /// `git pull` from the configured upstream; the user's `pull.rebase` / `pull.ff` apply.
    Pull,
    /// Pull, then push (VS Code's 同步更改).
    Sync,
    /// Switches to a local branch, or (`remote`) creates the local branch tracking a
    /// remote-tracking one, as `git switch --track` names it.
    Checkout {
        branch: String,
        remote: bool,
    },
    /// Creates a branch at `start` (a commit; HEAD when `None`) and switches to it.
    CreateBranch {
        name: String,
        start: Option<String>,
    },
    /// Applies a generated partial patch for one changed file: to the index (`cached`, stage /
    /// unstage selected lines) or to the worktree (revert selected lines).
    ApplyPatch {
        path: PathBuf,
        patch: String,
        cached: bool,
    },
}

#[derive(Clone, Debug)]
pub struct WriteRequest {
    pub repo: Repository,
    pub generation: u64,
    pub expected: Arc<Status>,
    pub operation: WriteOperation,
}

fn stamp(path: &Path, hash: &mut impl Hasher) -> io::Result<()> {
    path.hash(hash);
    match fs::symlink_metadata(path) {
        Ok(m) => (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
            .hash(hash),
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0u8.hash(hash),
        Err(e) => return Err(e),
    }
    Ok(())
}
pub(super) fn snapshot_version(repo: &Repository) -> io::Result<u64> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    stamp(&repo.id.0.join("index"), &mut hash)?;
    stamp(&repo.id.0.join("HEAD"), &mut hash)?;
    Ok(hash.finish())
}
pub(super) fn worktree_version(repo: &Repository, status: &Status, index: u64) -> io::Result<u64> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    index.hash(&mut hash);
    for change in &status.changes {
        validate_relative_path(&change.path)?;
        stamp(&repo.worktree.join(&change.path), &mut hash)?;
        if let Some(old) = &change.original_path {
            validate_relative_path(old)?;
            stamp(&repo.worktree.join(old), &mut hash)?;
        }
    }
    Ok(hash.finish())
}

impl GitService {
    pub fn write(&self, request: &WriteRequest, cancel: &AtomicBool) -> io::Result<Reply> {
        let lock = self.repo_lock(&request.repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        if self.identify(&request.repo.worktree, cancel)? != request.repo {
            return Err(error("仓库身份已变化，请重新打开工作区"));
        }
        let current = self.status_locked(&request.repo, request.generation, cancel)?;
        // Fetching only moves remote-tracking refs, so a stale view cannot mislead it.
        if current != *request.expected && !matches!(request.operation, WriteOperation::Fetch) {
            return Err(error("文件、暂存区或分支已变化，请刷新后重试"));
        }
        let conflicted = current
            .changes
            .iter()
            .any(|c| c.kind == ChangeKind::Conflict);
        let mut args: Vec<OsString> = vec!["--literal-pathspecs".into()];
        // Discarding untracked files deletes them with `git clean`, after the tracked restore.
        let mut untracked: Vec<PathBuf> = Vec::new();
        let mut input = None;
        match &request.operation {
            WriteOperation::Stage { paths }
            | WriteOperation::Unstage { paths }
            | WriteOperation::Discard { paths } => {
                if paths.is_empty() {
                    return Err(error("没有选中文件"));
                }
                for path in paths {
                    validate_relative_path(path)?;
                    let selected = current
                        .changes
                        .iter()
                        .find(|c| &c.path == path || c.original_path.as_ref() == Some(path))
                        .ok_or_else(|| error("选中文件已不在当前更改列表"))?;
                    match &request.operation {
                        WriteOperation::Stage { .. } if !selected.unstaged() => {
                            return Err(error("文件没有工作区更改"));
                        }
                        WriteOperation::Unstage { .. } if !selected.staged() => {
                            return Err(error("文件没有暂存更改"));
                        }
                        WriteOperation::Discard { .. }
                            if !selected.unstaged() || selected.kind == ChangeKind::Conflict =>
                        {
                            return Err(error("冲突文件不能直接放弃，请先解决冲突"));
                        }
                        WriteOperation::Discard { .. }
                            if selected.kind == ChangeKind::Untracked =>
                        {
                            untracked.push(path.clone());
                        }
                        _ => {}
                    }
                }
                match &request.operation {
                    WriteOperation::Stage { .. } => args.extend(["add".into(), "-A".into()]),
                    WriteOperation::Unstage { .. }
                        if current.oid.as_deref() == Some("(initial)") =>
                    {
                        args.extend([
                            "rm".into(),
                            "--cached".into(),
                            "-f".into(),
                            "--ignore-unmatch".into(),
                        ])
                    }
                    WriteOperation::Unstage { .. } => {
                        args.extend(["restore".into(), "--staged".into()])
                    }
                    WriteOperation::Discard { .. } if untracked.len() == paths.len() => {
                        args.extend(["clean".into(), "-f".into(), "-q".into()]);
                        untracked.clear();
                    }
                    WriteOperation::Discard { .. } => {
                        args.extend(["restore".into(), "--worktree".into()])
                    }
                    _ => unreachable!(),
                }
                args.push("--".into());
                args.extend(
                    paths
                        .iter()
                        .filter(|p| !untracked.contains(p))
                        .map(|p| p.as_os_str().to_owned()),
                );
            }
            WriteOperation::Commit {
                message,
                amend,
                push,
                all,
            } => {
                let keep_message = *amend && message.trim().is_empty();
                if (!keep_message && message.trim().is_empty())
                    || message.len() > 65_536
                    || message.contains('\0')
                {
                    return Err(error("提交信息不能为空，且不能超过 64 KiB"));
                }
                if *amend && *push {
                    return Err(error("修改上次提交后需要强制推送，请在终端操作"));
                }
                if *amend && matches!(current.oid.as_deref(), None | Some("(initial)")) {
                    return Err(error("还没有提交，无法修改上次提交"));
                }
                if !*amend && !*all && !current.changes.iter().any(Change::staged) {
                    return Err(error("暂存区没有更改"));
                }
                if *all && current.changes.is_empty() {
                    return Err(error("没有可提交的更改"));
                }
                if current
                    .changes
                    .iter()
                    .any(|c| c.kind == ChangeKind::Conflict)
                {
                    return Err(error("请先解决冲突"));
                }
                if *push {
                    // Checked before committing, so a missing upstream leaves nothing half done.
                    self.push_args(&request.repo, &current, cancel)?;
                }
                args.push("commit".into());
                if *amend {
                    args.push("--amend".into());
                }
                if keep_message {
                    args.push("--no-edit".into());
                } else {
                    args.push("--file=-".into());
                    input = Some(message.as_bytes());
                }
            }
            WriteOperation::ApplyPatch {
                path,
                patch,
                cached,
            } => {
                validate_relative_path(path)?;
                if patch.is_empty() || patch.len() > OUTPUT_LIMIT || patch.contains('\0') {
                    return Err(error("补丁为空或过大"));
                }
                let change = current
                    .changes
                    .iter()
                    .find(|c| &c.path == path)
                    .ok_or_else(|| error("文件已不在当前更改列表"))?;
                if change.kind == ChangeKind::Conflict {
                    return Err(error("冲突文件不能按行暂存"));
                }
                // Context lines carry the whole file, so a stale patch fails instead of
                // applying to text that changed since the diff was shown.
                args.extend(["apply".into(), "--whitespace=nowarn".into()]);
                if *cached {
                    args.push("--cached".into());
                }
                args.push("-".into());
                input = Some(patch.as_bytes());
            }
            WriteOperation::Push => {
                args.extend(self.push_args(&request.repo, &current, cancel)?);
            }
            WriteOperation::Fetch => {
                args.extend(["fetch".into(), "--all".into(), "--prune".into()]);
            }
            WriteOperation::Pull | WriteOperation::Sync => {
                if current.upstream.is_none() {
                    return Err(error(
                        "此分支尚未配置上游。请先在终端执行 git push -u，再刷新",
                    ));
                }
                if conflicted {
                    return Err(error("请先解决冲突"));
                }
                if let WriteOperation::Sync = &request.operation {
                    // Checked before pulling, so a branch that cannot be pushed is left alone.
                    self.push_args(&request.repo, &current, cancel)?;
                }
                args.extend(["pull".into(), "--no-edit".into()]);
                // Without either setting, Git refuses diverged branches outright; merging is
                // what it did before asking.
                let set = |key: &str| {
                    self.run(
                        &request.repo.worktree,
                        &["config".into(), "--get".into(), key.into()],
                        cancel,
                    )
                    .is_ok()
                };
                if !set("pull.rebase") && !set("pull.ff") {
                    args.push("--no-rebase".into());
                }
            }
            WriteOperation::Checkout { branch, remote } => {
                if branch.is_empty() || branch.starts_with('-') || branch.contains('\0') {
                    return Err(error("无效的分支名"));
                }
                if conflicted {
                    return Err(error("请先解决冲突"));
                }
                let full = if *remote {
                    format!("refs/remotes/{branch}")
                } else {
                    format!("refs/heads/{branch}")
                };
                self.verify(&request.repo, &full, cancel)
                    .map_err(|_| error(format!("找不到分支 {branch}")))?;
                args.push("switch".into());
                if *remote {
                    args.push("--track".into());
                }
                args.push(branch.into());
            }
            WriteOperation::CreateBranch { name, start } => {
                if name.trim().is_empty() || name.starts_with('-') || name.contains('\0') {
                    return Err(error("无效的分支名"));
                }
                if conflicted {
                    return Err(error("请先解决冲突"));
                }
                self.run(
                    &request.repo.worktree,
                    &["check-ref-format".into(), "--branch".into(), name.into()],
                    cancel,
                )
                .map_err(|_| error(format!("“{name}”不是有效的分支名")))?;
                if self
                    .verify(&request.repo, &format!("refs/heads/{name}"), cancel)
                    .is_ok()
                {
                    return Err(error(format!("分支 {name} 已存在")));
                }
                args.extend(["switch".into(), "-c".into(), name.into()]);
                if let Some(start) = start {
                    if !crate::graph::is_hash(start) {
                        return Err(error("无效的提交"));
                    }
                    self.verify(&request.repo, &format!("{start}^{{commit}}"), cancel)
                        .map_err(|_| error("找不到这个提交"))?;
                    args.push(start.into());
                }
            }
        }
        // Hooks, signatures and network authentication need more time than status queries.
        let writer = Self {
            shared: self.shared.clone(),
            timeout: Duration::from_secs(120),
        };
        let partial =
            |e: io::Error| error(format!("{e}。操作可能已经部分完成；请刷新检查仓库状态"));
        if let WriteOperation::Commit { all: true, .. } = &request.operation {
            writer
                .run_with_input(
                    &request.repo.worktree,
                    &["add".into(), "-A".into()],
                    cancel,
                    false,
                    None,
                )
                .map_err(partial)?;
        }
        let mut output = writer
            .run_with_input(&request.repo.worktree, &args, cancel, false, input)
            .map_err(partial)?;
        if let WriteOperation::Commit { push: true, .. } | WriteOperation::Sync = &request.operation
        {
            let mut push: Vec<OsString> = vec!["--literal-pathspecs".into()];
            push.extend(self.push_args(&request.repo, &current, cancel)?);
            let done = match request.operation {
                WriteOperation::Sync => "已拉取",
                _ => "已提交",
            };
            output.extend(
                writer
                    .run_with_input(&request.repo.worktree, &push, cancel, false, None)
                    .map_err(|e| error(format!("{done}，但推送失败：{e}")))?,
            );
        }
        if !untracked.is_empty() {
            let mut clean: Vec<OsString> = vec![
                "--literal-pathspecs".into(),
                "clean".into(),
                "-f".into(),
                "-q".into(),
                "--".into(),
            ];
            clean.extend(untracked.iter().map(|p| p.as_os_str().to_owned()));
            output.extend(
                writer
                    .run_with_input(&request.repo.worktree, &clean, cancel, false, None)
                    .map_err(partial)?,
            );
        }
        Ok(Reply {
            repo: request.repo.id.clone(),
            generation: request.generation,
            output,
        })
    }

    fn verify(&self, repo: &Repository, revision: &str, cancel: &AtomicBool) -> io::Result<()> {
        self.run(
            &repo.worktree,
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                revision.into(),
            ],
            cancel,
        )
        .map(|_| ())
    }

    /// `git push` arguments for the current branch to exactly its configured upstream branch,
    /// never forced, ignoring push.default / pushRemote / remote.push.
    fn push_args(
        &self,
        repo: &Repository,
        current: &Status,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<OsString>> {
        if current.upstream.is_none() {
            return Err(error(
                "此分支尚未配置上游。请先在终端执行 git push -u，再刷新",
            ));
        }
        // Resolve the exact configured upstream, rather than inheriting push.default,
        // pushRemote or remote.push (which may publish other branches).
        let remote = self.run(
            &repo.worktree,
            &[
                "rev-parse".into(),
                "--symbolic-full-name".into(),
                "@{upstream}".into(),
            ],
            cancel,
        )?;
        let remote = String::from_utf8(remote).map_err(io::Error::other)?;
        if !remote.trim().starts_with("refs/remotes/") {
            return Err(error("上游不是远程跟踪分支"));
        }
        let branch = current
            .branch
            .as_deref()
            .filter(|b| *b != "(detached)")
            .ok_or_else(|| error("detached HEAD 不能推送"))?;
        let config = |key: String| {
            self.run(
                &repo.worktree,
                &["config".into(), "--get".into(), key.into()],
                cancel,
            )
        };
        let remote = config(format!("branch.{branch}.remote"))?;
        let target = config(format!("branch.{branch}.merge"))?;
        let remote = String::from_utf8(remote)
            .map_err(io::Error::other)?
            .trim()
            .to_owned();
        let target = String::from_utf8(target)
            .map_err(io::Error::other)?
            .trim()
            .to_owned();
        if remote == "." || remote.starts_with('-') || !target.starts_with("refs/heads/") {
            return Err(error("无效的推送目标"));
        }
        Ok(vec![
            "push".into(),
            "--porcelain".into(),
            "--no-force".into(),
            "--no-follow-tags".into(),
            "--".into(),
            remote.into(),
            format!("HEAD:{target}").into(),
        ])
    }
}
