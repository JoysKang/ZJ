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
    /// Tags `commit` (a full hash): annotated with `message`, lightweight when `None`; `push`
    /// then pushes the tag to the tag remote under the same lock.
    CreateTag {
        name: String,
        commit: String,
        message: Option<String>,
        push: bool,
    },
    /// Pushes one tag to the tag remote, never forced.
    PushTag {
        name: String,
    },
    /// Deletes a local tag; `remote` deletes it on the tag remote first, so a failure there
    /// leaves the local tag to retry from.
    DeleteTag {
        name: String,
        remote: bool,
    },
    /// Applies a generated partial patch for one changed file: to the index (`cached`, stage /
    /// unstage selected lines) or to the worktree (revert selected lines).
    ApplyPatch {
        path: PathBuf,
        patch: String,
        cached: bool,
    },
    /// `git stash push`; `untracked` also stashes untracked files. An empty message lets Git
    /// name the stash after the branch and commit.
    StashPush {
        message: String,
        untracked: bool,
    },
    /// Applies `stash@{index}`, checked to still be `oid`; `pop` also drops it once applied
    /// cleanly (Git keeps it when the apply conflicts).
    StashApply {
        index: usize,
        oid: String,
        pop: bool,
    },
    /// Deletes `stash@{index}`, checked to still be `oid`.
    StashDrop {
        index: usize,
        oid: String,
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
        // Fetching and tags only touch refs named in the request, so a stale view of files
        // and branches cannot mislead them.
        if current != *request.expected
            && !matches!(
                request.operation,
                WriteOperation::Fetch
                    | WriteOperation::CreateTag { .. }
                    | WriteOperation::PushTag { .. }
                    | WriteOperation::DeleteTag { .. }
                    | WriteOperation::StashDrop { .. }
            )
        {
            return Err(error("文件、暂存区或分支已变化，请刷新后重试"));
        }
        // Hooks, signatures and network authentication need more time than status queries.
        let writer = Self {
            shared: self.shared.clone(),
            timeout: Duration::from_secs(120),
            grace: Duration::from_secs(1),
        };
        let mut tag_push = None;
        let conflicted = current
            .changes
            .iter()
            .any(|c| c.kind == ChangeKind::Conflict);
        let mut args: Vec<OsString> = vec!["--literal-pathspecs".into()];
        // Discarding untracked files deletes them with `git clean`, after the tracked restore.
        let mut untracked: Vec<PathBuf> = Vec::new();
        // Of those, the ones added with `git add -N`, whose index entry goes first.
        let mut intent: Vec<PathBuf> = Vec::new();
        // Every path the discard deletes, checked afterwards: `git clean -f` skips nested
        // repositories without failing.
        let mut deleting: Vec<PathBuf> = Vec::new();
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
                        WriteOperation::Discard { .. } if selected.kind == ChangeKind::Conflict => {
                            return Err(error("冲突文件不能直接放弃，请先解决冲突"));
                        }
                        // Discarding restores the worktree from the index; staged changes stay.
                        WriteOperation::Discard { .. } if !selected.unstaged() => {
                            return Err(error("文件没有工作区更改；暂存的更改请先取消暂存"));
                        }
                        WriteOperation::Discard { .. }
                            if selected.kind == ChangeKind::Untracked =>
                        {
                            untracked.push(path.clone());
                        }
                        // `git add -N`: restoring from the empty index entry would empty the
                        // file. Drop the entry, then delete it like an untracked file.
                        WriteOperation::Discard { .. } if selected.new_in_worktree() => {
                            intent.push(path.clone());
                            untracked.push(path.clone());
                        }
                        _ => {}
                    }
                }
                deleting = untracked.clone();
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
                // Without any setting, Git refuses diverged branches outright; merging is what
                // it did before asking. The branch's own `rebase` setting counts too (a
                // command-line --no-rebase would override it). Only "not set" (exit 1, no
                // output) means unset; another failure is reported.
                let set = |key: String| -> io::Result<bool> {
                    self.run_command(
                        &request.repo.worktree,
                        &["config".into(), "--get".into(), key.into()],
                        cancel,
                        true,
                    )
                    .map(|value| !value.is_empty())
                };
                let mut keys = vec!["pull.rebase".to_string(), "pull.ff".to_string()];
                if let Some(branch) = current.branch.as_deref().filter(|b| *b != "(detached)") {
                    keys.push(format!("branch.{branch}.rebase"));
                }
                let mut configured = false;
                for key in keys {
                    configured |= set(key)?;
                }
                if !configured {
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
                if !self.verify(&request.repo, &full, cancel)? {
                    return Err(error(format!("找不到分支 {branch}")));
                }
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
                if self.verify(&request.repo, &format!("refs/heads/{name}"), cancel)? {
                    return Err(error(format!("分支 {name} 已存在")));
                }
                args.extend(["switch".into(), "-c".into(), name.into()]);
                if let Some(start) = start {
                    if !crate::graph::is_hash(start) {
                        return Err(error("无效的提交"));
                    }
                    if !self.verify(&request.repo, &format!("{start}^{{commit}}"), cancel)? {
                        return Err(error("找不到这个提交"));
                    }
                    args.push(start.into());
                }
            }
            WriteOperation::CreateTag {
                name,
                commit,
                message,
                push,
            } => {
                self.check_tag_name(&request.repo, name, cancel)?;
                if self.has_tag(&request.repo, name, cancel)? {
                    return Err(error(format!("标签 {name} 已存在")));
                }
                if !crate::graph::is_hash(commit) {
                    return Err(error("无效的提交"));
                }
                if !self.verify(&request.repo, &format!("{commit}^{{commit}}"), cancel)? {
                    return Err(error("找不到这个提交"));
                }
                if *push {
                    // Checked before tagging, so a missing remote leaves nothing half done.
                    let remote = self.tag_remote(&request.repo, &current, cancel)?;
                    tag_push = Some(tag_push_args(&remote, format!("refs/tags/{name}")));
                }
                args.push("tag".into());
                if let Some(message) = message {
                    if message.trim().is_empty() || message.len() > 65_536 || message.contains('\0')
                    {
                        return Err(error("标签说明不能为空，且不能超过 64 KiB"));
                    }
                    args.extend(["-a".into(), "-F".into(), "-".into()]);
                    input = Some(message.as_bytes());
                }
                args.extend([name.into(), commit.into()]);
            }
            WriteOperation::PushTag { name } => {
                self.check_tag_name(&request.repo, name, cancel)?;
                if !self.has_tag(&request.repo, name, cancel)? {
                    return Err(error(format!("找不到标签 {name}")));
                }
                let remote = self.tag_remote(&request.repo, &current, cancel)?;
                args = tag_push_args(&remote, format!("refs/tags/{name}"));
            }
            WriteOperation::StashPush { message, untracked } => {
                if conflicted {
                    return Err(error("请先解决冲突"));
                }
                let stashable = current
                    .changes
                    .iter()
                    .any(|c| *untracked || c.kind != ChangeKind::Untracked);
                if !stashable {
                    return Err(error("没有可以 stash 的更改"));
                }
                if message.len() > 65_536 || message.contains('\0') || message.contains('\n') {
                    return Err(error("stash 说明须为一行，且不能超过 64 KiB"));
                }
                // No paths here, and with --literal-pathspecs `stash push -u` saves untracked
                // files but leaves them in the worktree (its clean step uses a magic pathspec).
                args = vec!["stash".into(), "push".into()];
                if *untracked {
                    args.push("--include-untracked".into());
                }
                if !message.trim().is_empty() {
                    args.extend(["-m".into(), message.trim().into()]);
                }
            }
            WriteOperation::StashApply { index, oid, .. }
            | WriteOperation::StashDrop { index, oid } => {
                let name = format!("stash@{{{index}}}");
                let found = self
                    .run_command(
                        &request.repo.worktree,
                        &[
                            "rev-parse".into(),
                            "--verify".into(),
                            "--quiet".into(),
                            name.clone().into(),
                        ],
                        cancel,
                        true,
                    )
                    .map(|out| String::from_utf8_lossy(&out).trim().to_owned())?;
                if found != *oid {
                    return Err(error("stash 列表已变化，请重新选择"));
                }
                args = vec!["stash".into()];
                args.push(match &request.operation {
                    WriteOperation::StashApply { pop: true, .. } => "pop".into(),
                    WriteOperation::StashApply { .. } => "apply".into(),
                    _ => "drop".into(),
                });
                args.push(name.into());
            }
            WriteOperation::DeleteTag { name, remote } => {
                self.check_tag_name(&request.repo, name, cancel)?;
                if !self.has_tag(&request.repo, name, cancel)? {
                    return Err(error(format!("找不到标签 {name}")));
                }
                if *remote {
                    let remote = self.tag_remote(&request.repo, &current, cancel)?;
                    let delete = tag_push_args(&remote, format!(":refs/tags/{name}"));
                    writer
                        .run_with_input(&request.repo.worktree, &delete, cancel, false, None)
                        .map_err(|e| {
                            error(format!("无法从远程 {remote} 删除标签，本地标签未删除：{e}"))
                        })?;
                }
                args.extend(["tag".into(), "-d".into(), name.into()]);
            }
        }
        let partial =
            |e: io::Error| error(format!("{e}。操作可能已经部分完成；请刷新检查仓库状态"));
        if !intent.is_empty() {
            let mut remove: Vec<OsString> = vec![
                "--literal-pathspecs".into(),
                "rm".into(),
                "--cached".into(),
                "-q".into(),
                "--".into(),
            ];
            remove.extend(intent.iter().map(|p| p.as_os_str().to_owned()));
            writer
                .run_with_input(&request.repo.worktree, &remove, cancel, false, None)
                .map_err(partial)?;
        }
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
        if let Some(push) = tag_push {
            output.extend(
                writer
                    .run_with_input(&request.repo.worktree, &push, cancel, false, None)
                    .map_err(|e| error(format!("已创建标签，但推送失败：{e}")))?,
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
        let left: Vec<String> = deleting
            .iter()
            .filter(|path| fs::symlink_metadata(request.repo.worktree.join(path)).is_ok())
            .map(|path| path.display().to_string())
            .collect();
        if !left.is_empty() {
            return Err(error(format!(
                "没能删除：{}（嵌套的 Git 仓库等不会被删除，请在终端处理）",
                left.join("、")
            )));
        }
        Ok(Reply {
            repo: request.repo.id.clone(),
            generation: request.generation,
            output,
        })
    }

    /// Whether `revision` names an object. Only `rev-parse --verify --quiet`'s silent exit 1
    /// means "absent"; a timeout, cancellation or any other Git failure is an error.
    pub(super) fn verify(
        &self,
        repo: &Repository,
        revision: &str,
        cancel: &AtomicBool,
    ) -> io::Result<bool> {
        let output = self.run_command(
            &repo.worktree,
            &[
                "rev-parse".into(),
                "--verify".into(),
                "--quiet".into(),
                revision.into(),
            ],
            cancel,
            // Exit 1 with nothing on stderr comes back as Ok with no output.
            true,
        )?;
        Ok(!output.is_empty())
    }

    fn check_tag_name(&self, repo: &Repository, name: &str, cancel: &AtomicBool) -> io::Result<()> {
        if name.trim().is_empty() || name.starts_with('-') || name.contains('\0') {
            return Err(error("无效的标签名"));
        }
        self.run(
            &repo.worktree,
            &[
                "check-ref-format".into(),
                format!("refs/tags/{name}").into(),
            ],
            cancel,
        )
        .map(|_| ())
        .map_err(|_| error(format!("“{name}”不是有效的标签名")))
    }

    fn has_tag(&self, repo: &Repository, name: &str, cancel: &AtomicBool) -> io::Result<bool> {
        self.verify(repo, &format!("refs/tags/{name}"), cancel)
    }

    /// Where tags go: the current branch's remote, else the only remote, else `origin`.
    fn tag_remote(
        &self,
        repo: &Repository,
        current: &Status,
        cancel: &AtomicBool,
    ) -> io::Result<String> {
        let text = |bytes: Vec<u8>| {
            String::from_utf8(bytes)
                .map_err(io::Error::other)
                .map(|text| text.trim().to_owned())
        };
        if current.upstream.is_some()
            && let Some(branch) = current.branch.as_deref().filter(|b| *b != "(detached)")
            && let Ok(remote) = self.run(
                &repo.worktree,
                &[
                    "config".into(),
                    "--get".into(),
                    format!("branch.{branch}.remote").into(),
                ],
                cancel,
            )
        {
            let remote = text(remote)?;
            if !remote.is_empty() && remote != "." && !remote.starts_with('-') {
                return Ok(remote);
            }
        }
        let remotes = text(self.run(&repo.worktree, &["remote".into()], cancel)?)?;
        let remotes: Vec<&str> = remotes.lines().filter(|r| !r.starts_with('-')).collect();
        match remotes.as_slice() {
            [] => Err(error("仓库没有配置远程")),
            [only] => Ok((*only).to_owned()),
            _ if remotes.contains(&"origin") => Ok("origin".into()),
            _ => Err(error(
                "有多个远程且当前分支没有上游，无法确定推送到哪个远程，请在终端操作",
            )),
        }
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

/// Pushes (or, with `:refs/tags/…`, deletes) exactly one tag ref, never forced.
fn tag_push_args(remote: &str, refspec: String) -> Vec<OsString> {
    vec![
        "--literal-pathspecs".into(),
        "push".into(),
        "--porcelain".into(),
        "--no-force".into(),
        "--".into(),
        remote.into(),
        refspec.into(),
    ]
}
