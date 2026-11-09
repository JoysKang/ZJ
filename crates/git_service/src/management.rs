//! Explicit repository management; writes use the normal identity/snapshot locks.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InProgress {
    Merge,
    Rebase,
    CherryPick,
}
impl InProgress {
    pub fn label(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Remote {
    pub name: String,
    pub fetch_urls: Vec<String>,
    pub push_urls: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct Submodule {
    pub path: PathBuf,
    /// The expanded default fetch URL (or the expanded clone URL for a new module).
    pub url: String,
    name: String,
    pub initialized: bool,
    pub conflicted: bool,
}
#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub locked: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LfsPointer {
    pub oid: String,
    pub size: u64,
}
pub fn parse_lfs_pointer(bytes: &[u8]) -> Option<LfsPointer> {
    if bytes.len() > 1024 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "version https://git-lfs.github.com/spec/v1" {
        return None;
    }
    let oid = lines.next()?.strip_prefix("oid sha256:")?;
    if oid.len() != 64 || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let size = lines.next()?.strip_prefix("size ")?.parse().ok()?;
    if lines.next().is_some() {
        return None;
    }
    Some(LfsPointer {
        oid: oid.into(),
        size,
    })
}

/// Remove URL credentials and query/fragment tokens from both Git output and errors.
pub fn redact_git_output(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for part in text.split_inclusive(char::is_whitespace) {
        if part.match_indices("://").count() > 1 {
            out.push_str("[redacted URLs]");
            out.push_str(&part[part.trim_end().len()..]);
            continue;
        }
        let mut rest = part;
        while let Some(scheme) = rest.find("://") {
            let authority = scheme + 3;
            out.push_str(&rest[..authority]);
            let authority_end = rest[authority..]
                .find('/')
                .map_or(rest.trim_end().len(), |n| authority + n);
            let host_start = rest[authority..authority_end]
                .rfind('@')
                .map_or(authority, |n| authority + n + 1);
            let end = rest[host_start..]
                .find(['/', '?', '#'])
                .map_or(rest.trim_end().len(), |n| host_start + n);
            let host = &rest[authority..end];
            if let Some((_, host)) = host.rsplit_once('@') {
                out.push_str("[redacted]@");
                out.push_str(host);
            } else {
                out.push_str(host);
            }
            rest = &rest[end..];
            if let Some(query) = rest.find(['?', '#'])
                && rest.find("://").is_none_or(|next| query < next)
            {
                out.push_str(&rest[..query]);
                out.push_str("[redacted]");
                rest = &rest[rest.trim_end().len()..];
                break;
            }
        }
        out.push_str(rest);
    }
    out
}
pub fn push_result_summary(text: &str) -> &'static str {
    let flags: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let b = line.as_bytes();
            (b.get(1) == Some(&b'\t')).then(|| b[0])
        })
        .collect();
    if flags.contains(&b'!') {
        "推送失败或部分完成"
    } else if flags.iter().any(|b| matches!(b, b' ' | b'*' | b'+' | b'-')) {
        "远程引用已更新"
    } else if !flags.is_empty() && flags.iter().all(|b| *b == b'=') {
        "远程无变化"
    } else {
        "Git 操作完成（详见结果）"
    }
}

impl GitService {
    fn checked_read<T>(
        &self,
        repo: &Repository,
        cancel: &AtomicBool,
        read: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        let common = self.refs_lock(&repo.common_dir);
        let _common = self.lock_repo(&common, cancel)?;
        let private = self.repo_lock(&repo.id);
        let _private = self.lock_repo(&private, cancel)?;
        if self.identify(&repo.worktree, cancel)? != *repo {
            return Err(error("仓库身份已变化"));
        }
        read()
    }
    pub fn remotes(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<Remote>> {
        self.checked_read(repo, cancel, || self.remotes_locked(repo, cancel))
    }
    fn remotes_locked(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<Remote>> {
        let names = self.run(&repo.worktree, &["remote".into()], cancel)?;
        String::from_utf8_lossy(&names)
            .lines()
            .map(|name| {
                let urls = |push| -> io::Result<Vec<String>> {
                    let mut args = vec!["remote".into(), "get-url".into(), "--all".into()];
                    if push {
                        args.push("--push".into());
                    }
                    args.push(name.into());
                    Ok(
                        String::from_utf8_lossy(&self.run(&repo.worktree, &args, cancel)?)
                            .lines()
                            .map(str::to_owned)
                            .collect(),
                    )
                };
                Ok(Remote {
                    name: name.into(),
                    fetch_urls: urls(false)?,
                    push_urls: urls(true)?,
                })
            })
            .collect()
    }
    pub fn in_progress(
        &self,
        repo: &Repository,
        cancel: &AtomicBool,
    ) -> io::Result<Option<InProgress>> {
        self.checked_read(repo, cancel, || self.in_progress_locked(repo))
    }
    fn in_progress_locked(&self, repo: &Repository) -> io::Result<Option<InProgress>> {
        for (name, operation) in [
            ("rebase-merge", InProgress::Rebase),
            ("rebase-apply", InProgress::Rebase),
            ("MERGE_HEAD", InProgress::Merge),
            ("CHERRY_PICK_HEAD", InProgress::CherryPick),
        ] {
            match fs::symlink_metadata(repo.id.0.join(name)) {
                Ok(_) => return Ok(Some(operation)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }
    pub fn submodules(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<Submodule>> {
        self.checked_read(repo, cancel, || self.submodules_locked(repo, cancel))
    }
    fn submodules_locked(
        &self,
        repo: &Repository,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<Submodule>> {
        let raw = self.run(
            &repo.worktree,
            &["ls-files".into(), "--stage".into(), "-z".into()],
            cancel,
        )?;
        let mut modules = std::collections::BTreeMap::new();
        for record in raw.split(|b| *b == 0).filter(|r| r.starts_with(b"160000 ")) {
            let tab = record
                .iter()
                .position(|b| *b == b'\t')
                .ok_or_else(|| error("无效的 gitlink"))?;
            let path = PathBuf::from(OsString::from_vec(record[tab + 1..].into()));
            validate_relative_path(&path)?;
            let conflicted = !record[..tab].ends_with(b" 0");
            modules
                .entry(path)
                .and_modify(|conflict| *conflict |= conflicted)
                .or_insert(conflicted);
        }
        if modules.is_empty() {
            return Ok(Vec::new());
        }
        let config = self.run_command(
            &repo.worktree,
            &[
                "config".into(),
                "--null".into(),
                "--file".into(),
                ".gitmodules".into(),
                "--get-regexp".into(),
                "^submodule\\..*\\.path$".into(),
            ],
            cancel,
            true,
        )?;
        let mut names = HashMap::new();
        for record in config.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let newline = record
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(|| error("无效的子模块配置"))?;
            let key = std::str::from_utf8(&record[..newline]).map_err(io::Error::other)?;
            let name = key
                .strip_suffix(".path")
                .ok_or_else(|| error("无效的子模块路径配置"))?;
            names.insert(
                PathBuf::from(OsString::from_vec(record[newline + 1..].into())),
                name.to_owned(),
            );
        }
        modules
            .into_iter()
            .map(|(path, conflicted)| {
                let name = names.get(&path).ok_or_else(|| {
                    error(format!(
                        "子模块 {} 缺少 .gitmodules 路径配置",
                        path.display()
                    ))
                })?;
                let key = format!("{name}.url");
                let local = self.run_command(
                    &repo.worktree,
                    &["config".into(), "--get".into(), key.clone().into()],
                    cancel,
                    true,
                )?;
                let url = if local.is_empty() {
                    self.run(
                        &repo.worktree,
                        &[
                            "config".into(),
                            "--file".into(),
                            ".gitmodules".into(),
                            "--get".into(),
                            key.into(),
                        ],
                        cancel,
                    )?
                } else {
                    local
                };
                let url = String::from_utf8(url)
                    .map_err(io::Error::other)?
                    .trim()
                    .to_owned();
                check_url(&url)?;
                let initialized = git_marker(&repo.worktree.join(&path)) == GitMarker::Valid;
                let url =
                    self.submodule_connection_url(repo, &path, name, &url, initialized, cancel)?;
                Ok(Submodule {
                    name: name.clone(),
                    initialized,
                    path,
                    url,
                    conflicted,
                })
            })
            .collect()
    }
    fn submodule_connection_url(
        &self,
        repo: &Repository,
        path: &Path,
        name: &str,
        configured: &str,
        initialized: bool,
        cancel: &AtomicBool,
    ) -> io::Result<String> {
        validate_relative_path(path)?;
        let module_name = name
            .strip_prefix("submodule.")
            .ok_or_else(|| error("无效的子模块名称"))?;
        validate_relative_path(Path::new(module_name))?;
        let git_dir = self.run(
            &repo.worktree,
            &[
                "rev-parse".into(),
                "--path-format=absolute".into(),
                "--git-path".into(),
                format!("modules/{module_name}").into(),
            ],
            cancel,
        )?;
        let git_dir = PathBuf::from(String::from_utf8(git_dir).map_err(io::Error::other)?.trim());
        let prefix = if initialized {
            vec![
                OsString::from("-C"),
                repo.worktree.join(path).into_os_string(),
            ]
        } else if git_dir.join("config").is_file() {
            vec![OsString::from("--git-dir"), git_dir.into_os_string()]
        } else {
            if configured.starts_with("./") || configured.starts_with("../") {
                return Err(error(
                    "相对子模块地址尚未初始化，请先在外部 Git 中初始化以确定实际目标",
                ));
            }
            // --get-url expands insteadOf locally and performs no connection.
            let resolved = self.run(
                &repo.worktree,
                &["ls-remote".into(), "--get-url".into(), configured.into()],
                cancel,
            )?;
            let resolved = String::from_utf8(resolved)
                .map_err(io::Error::other)?
                .trim()
                .to_owned();
            check_url(&resolved)?;
            let repeated = self.run(
                &repo.worktree,
                &[
                    "ls-remote".into(),
                    "--get-url".into(),
                    resolved.clone().into(),
                ],
                cancel,
            )?;
            if repeated != format!("{resolved}\n").as_bytes() {
                return Err(error(
                    "子模块地址存在连续重写，请先在外部 Git 中初始化以确定实际目标",
                ));
            }
            return Ok(resolved);
        };
        let query = |args: &[&str], optional| {
            let mut command = prefix.clone();
            command.extend(args.iter().map(|arg| OsString::from(*arg)));
            self.run_command(&repo.worktree, &command, cancel, optional)
        };
        let branch = query(&["symbolic-ref", "--quiet", "--short", "HEAD"], true)?;
        let branch = String::from_utf8(branch).map_err(io::Error::other)?;
        let remote = if branch.trim().is_empty() {
            Vec::new()
        } else {
            query(
                &[
                    "config",
                    "--get",
                    &format!("branch.{}.remote", branch.trim()),
                ],
                true,
            )?
        };
        let remote = String::from_utf8(remote).map_err(io::Error::other)?;
        let remote = if remote.trim().is_empty() {
            "origin"
        } else {
            remote.trim()
        };
        if remote == "." {
            return Err(error("子模块默认远端为本地自身，请在外部 Git 中更新"));
        }
        let urls = query(&["remote", "get-url", "--all", remote], false)?;
        let urls = String::from_utf8(urls).map_err(io::Error::other)?;
        let mut urls = urls.lines();
        let url = urls.next().ok_or_else(|| error("子模块默认远端没有地址"))?;
        if urls.next().is_some() {
            return Err(error("子模块默认远端有多个地址，请在外部 Git 中更新"));
        }
        check_url(url)?;
        Ok(url.to_owned())
    }

    pub fn worktrees(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<Worktree>> {
        self.checked_read(repo, cancel, || self.worktrees_locked(repo, cancel))
    }
    fn worktrees_locked(
        &self,
        repo: &Repository,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<Worktree>> {
        let raw = self.run(
            &repo.worktree,
            &[
                "worktree".into(),
                "list".into(),
                "--porcelain".into(),
                "-z".into(),
            ],
            cancel,
        )?;
        let mut trees: Vec<Worktree> = Vec::new();
        for field in raw.split(|b| *b == 0) {
            if let Some(path) = field.strip_prefix(b"worktree ") {
                trees.push(Worktree {
                    path: PathBuf::from(OsString::from_vec(path.into())),
                    head: String::new(),
                    branch: None,
                    locked: false,
                });
            } else if let Some(tree) = trees.last_mut() {
                if let Some(head) = field.strip_prefix(b"HEAD ") {
                    tree.head = String::from_utf8_lossy(head).into_owned();
                }
                if let Some(branch) = field.strip_prefix(b"branch ") {
                    tree.branch = Some(String::from_utf8_lossy(branch).into_owned());
                }
                if field == b"locked" || field.starts_with(b"locked ") {
                    tree.locked = true;
                }
            }
        }
        Ok(trees)
    }
    pub(super) fn management_args(
        &self,
        repo: &Repository,
        current: &Status,
        operation: &WriteOperation,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<OsString>> {
        let branch_name = |name: &str| -> io::Result<()> {
            if name.is_empty() || name.starts_with('-') || name.contains('\0') {
                return Err(error("无效的分支名"));
            }
            self.run(
                &repo.worktree,
                &[
                    "check-ref-format".into(),
                    format!("refs/heads/{name}").into(),
                ],
                cancel,
            )
            .map(|_| ())
        };
        let remote = |name: &str| -> io::Result<()> {
            if !self
                .remotes_locked(repo, cancel)?
                .iter()
                .any(|r| r.name == name)
            {
                return Err(error("远程列表已变化，请重新选择"));
            }
            Ok(())
        };
        let branch_tip = |name: &str, oid: &str| -> io::Result<()> {
            branch_name(name)?;
            let found = self.run(
                &repo.worktree,
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    format!("refs/heads/{name}").into(),
                ],
                cancel,
            )?;
            if String::from_utf8_lossy(&found).trim() != oid {
                return Err(error("分支提交已变化，请重新选择"));
            }
            Ok(())
        };
        Ok(match operation {
            WriteOperation::PublishBranch {
                remote: name,
                target,
                expected_push_url,
            } => {
                remote(name)?;
                branch_name(target)?;
                if matches!(current.branch.as_deref(), None | Some("(detached)"))
                    || matches!(current.oid.as_deref(), None | Some("(initial)"))
                {
                    return Err(error("请先在本地分支创建提交"));
                }
                let urls = self
                    .remotes_locked(repo, cancel)?
                    .into_iter()
                    .find(|r| r.name == *name)
                    .unwrap()
                    .push_urls;
                if urls.len() != 1 {
                    return Err(error("远程配置了多个推送地址，请在终端发布，避免部分成功"));
                }
                if urls.first() != Some(expected_push_url) {
                    return Err(error("远程推送地址已变化，请重新选择并确认目标"));
                }
                vec![
                    "push".into(),
                    "--porcelain".into(),
                    "--no-force".into(),
                    "--no-follow-tags".into(),
                    "--set-upstream".into(),
                    "--".into(),
                    name.into(),
                    format!("HEAD:refs/heads/{target}").into(),
                ]
            }
            WriteOperation::AddRemote { name, url } => {
                branch_name(name)?;
                check_url(url)?;
                vec![
                    "remote".into(),
                    "add".into(),
                    "--".into(),
                    name.into(),
                    url.into(),
                ]
            }
            WriteOperation::SetRemoteUrl {
                name,
                expected_url,
                url,
            } => {
                remote(name)?;
                check_url(url)?;
                let found = self
                    .remotes_locked(repo, cancel)?
                    .into_iter()
                    .find(|r| r.name == *name)
                    .unwrap();
                if found.fetch_urls.len() != 1 || found.fetch_urls.first() != Some(expected_url) {
                    return Err(error("远程地址已变化或包含多个地址，请重新查看"));
                }
                vec![
                    "remote".into(),
                    "set-url".into(),
                    "--".into(),
                    name.into(),
                    url.into(),
                ]
            }
            WriteOperation::RenameBranch {
                name,
                new_name,
                oid,
            } => {
                branch_tip(name, oid)?;
                branch_name(new_name)?;
                vec!["branch".into(), "-m".into(), name.into(), new_name.into()]
            }
            WriteOperation::DeleteBranch { name, oid } => {
                branch_tip(name, oid)?;
                vec!["branch".into(), "-d".into(), "--".into(), name.into()]
            }
            WriteOperation::SetUpstream { branch, upstream } => {
                branch_name(branch)?;
                if !upstream.starts_with("refs/remotes/") || !self.verify(repo, upstream, cancel)? {
                    return Err(error("请选择存在的远程跟踪分支"));
                }
                vec![
                    "branch".into(),
                    format!("--set-upstream-to={upstream}").into(),
                    "--".into(),
                    branch.into(),
                ]
            }
            WriteOperation::ContinueOperation {
                operation: progress,
            }
            | WriteOperation::AbortOperation {
                operation: progress,
            } => {
                if self.in_progress_locked(repo)? != Some(*progress) {
                    return Err(error("进行中的 Git 操作已变化，请刷新"));
                }
                if matches!(operation, WriteOperation::ContinueOperation { .. })
                    && current
                        .changes
                        .iter()
                        .any(|c| c.kind == ChangeKind::Conflict)
                {
                    return Err(error("请先解决并暂存冲突"));
                }
                vec![
                    "-c".into(),
                    "core.editor=true".into(),
                    "-c".into(),
                    "sequence.editor=true".into(),
                    progress.label().into(),
                    if matches!(operation, WriteOperation::ContinueOperation { .. }) {
                        "--continue".into()
                    } else {
                        "--abort".into()
                    },
                ]
            }
            WriteOperation::UpdateSubmodule { path, expected_url } => {
                validate_relative_path(path)?;
                let module = self
                    .submodules_locked(repo, cancel)?
                    .into_iter()
                    .find(|m| m.path == *path)
                    .ok_or_else(|| error("所选路径不是子模块"))?;
                if module.conflicted {
                    return Err(error("请先解决父仓库中的子模块提交冲突"));
                }
                if module.url != *expected_url {
                    return Err(error("子模块来源地址已变化，请重新确认"));
                }
                vec![
                    "-c".into(),
                    format!("{}.url={}", module.name, module.url).into(),
                    "--literal-pathspecs".into(),
                    "submodule".into(),
                    "update".into(),
                    "--init".into(),
                    "--checkout".into(),
                    "--".into(),
                    path.as_os_str().into(),
                ]
            }
            WriteOperation::AddWorktree { path, branch } => {
                branch_name(branch)?;
                require_new_directory(path)?;
                if !self.verify(repo, &format!("refs/heads/{branch}"), cancel)? {
                    return Err(error("本地分支不存在"));
                }
                vec![
                    "worktree".into(),
                    "add".into(),
                    "--".into(),
                    path.as_os_str().into(),
                    branch.into(),
                ]
            }
            WriteOperation::RemoveWorktree { path, head } => {
                let tree = self
                    .worktrees_locked(repo, cancel)?
                    .into_iter()
                    .find(|t| t.path == *path)
                    .ok_or_else(|| error("工作树列表已变化"))?;
                if tree.head != *head || tree.locked || *path == repo.worktree {
                    return Err(error("工作树已变化、被锁定或正在使用"));
                }
                vec![
                    "worktree".into(),
                    "remove".into(),
                    "--".into(),
                    path.as_os_str().into(),
                ]
            }
            _ => return Err(error("不是仓库管理操作")),
        })
    }
    /// Creates only a previously nonexistent destination; never reinitializes a user's repo.
    pub fn create_repository(
        &self,
        destination: &Path,
        source: Option<&str>,
        cancel: &AtomicBool,
    ) -> io::Result<Repository> {
        require_new_directory(destination)?;
        let parent = destination
            .parent()
            .ok_or_else(|| error("目标没有父目录"))?
            .canonicalize()?;
        let target = parent.join(
            destination
                .file_name()
                .ok_or_else(|| error("无效的目录名"))?,
        );
        let lock = self.refs_lock(&target);
        let _lock = self.lock_repo(&lock, cancel)?;
        require_new_directory(&target)?;
        let writer = Self {
            shared: self.shared.clone(),
            timeout: Duration::from_secs(120),
            grace: Duration::from_secs(1),
        };
        let args = if let Some(source) = source {
            check_url(source)?;
            vec![
                "clone".into(),
                "--".into(),
                source.into(),
                target.as_os_str().into(),
            ]
        } else {
            vec!["init".into(), "--".into(), target.as_os_str().into()]
        };
        // Reserve the namespace atomically; Git never receives an unowned existing directory.
        fs::create_dir(&target)?;
        use std::os::unix::fs::MetadataExt;
        let identity = fs::symlink_metadata(&target)?;
        writer
            .run_write_with_input(&parent, &args, cancel, false, None)
            .map_err(|e| {
                error(redact_git_output(&format!(
                    "{e}；请检查目标目录，失败时可能留下部分文件"
                )))
            })?;
        let after = fs::symlink_metadata(&target)?;
        if (identity.dev(), identity.ino()) != (after.dev(), after.ino()) {
            return Err(error("创建期间目标目录身份已变化，请检查目标目录"));
        }
        self.identify(&target, cancel)
    }
}
fn require_new_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(error("目标必须是尚不存在的绝对路径"));
    }
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
        Ok(_) => Err(error("目标路径已存在（包括符号链接），拒绝覆盖")),
    }
}
fn check_url(url: &str) -> io::Result<()> {
    if url.trim().is_empty()
        || url.starts_with('-')
        || url.chars().any(char::is_control)
        || url.starts_with("ext::")
    {
        return Err(error("无效的远程地址"));
    }
    Ok(())
}
