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
    Commit {
        message: String,
    },
    Push,
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
        if current != *request.expected {
            return Err(error("文件、暂存区或分支已变化，请刷新后重试"));
        }
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
            WriteOperation::Commit { message } => {
                if message.trim().is_empty() || message.len() > 65_536 || message.contains('\0') {
                    return Err(error("提交信息不能为空，且不能超过 64 KiB"));
                }
                if !current.changes.iter().any(Change::staged) {
                    return Err(error("暂存区没有更改"));
                }
                if current
                    .changes
                    .iter()
                    .any(|c| c.kind == ChangeKind::Conflict)
                {
                    return Err(error("请先解决冲突"));
                }
                args.extend(["commit".into(), "--file=-".into()]);
                input = Some(message.as_bytes());
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
                if current.upstream.is_none() {
                    return Err(error(
                        "此分支尚未配置上游。请先在终端执行 git push -u，再刷新",
                    ));
                }
                // Resolve the exact configured upstream, rather than inheriting push.default,
                // pushRemote or remote.push (which may publish other branches).
                let remote = self.run(
                    &request.repo.worktree,
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
                        &request.repo.worktree,
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
                args.extend([
                    "push".into(),
                    "--porcelain".into(),
                    "--no-force".into(),
                    "--no-follow-tags".into(),
                    "--".into(),
                    remote.into(),
                    format!("HEAD:{target}").into(),
                ]);
            }
        }
        // Hooks, signatures and network authentication need more time than status queries.
        let writer = Self {
            shared: self.shared.clone(),
            timeout: Duration::from_secs(120),
        };
        let partial =
            |e: io::Error| error(format!("{e}。操作可能已经部分完成；请刷新检查仓库状态"));
        let mut output = writer
            .run_with_input(&request.repo.worktree, &args, cancel, false, input)
            .map_err(partial)?;
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
}
