//! Bounded, cancellable system Git operations, serialized by worktree identity.

mod graph;
mod log;
mod ls_files;
mod refs;
mod status;
mod write;
pub use graph::{
    CommitDetails, CommitFile, GraphCommit, GraphRef, RefKind, parse_graph, parse_name_status,
};
pub use log::{Commit, parse_log};
pub use ls_files::{ListedKind, parse_ls_files};
pub use refs::{Branch, parse_branches};
pub use status::{Change, ChangeKind, Status, parse_status};
pub use write::{WriteOperation, WriteRequest};

use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs,
    io::{self, Read, Write},
    os::unix::ffi::OsStringExt,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use workspace_editor_core::{GitMarker, RepoId, Repository, git_marker, is_excluded_dir};

const OUTPUT_LIMIT: usize = 16_000_000;
/// Diffs carry the whole file so the diff editor can rebuild both sides.
const FULL_CONTEXT: &str = "--unified=1000000";
/// Patch paths always start with `a/` and `b/`: partial staging and `git apply -p1` rely on
/// it, whatever `diff.noprefix` or `diff.mnemonicPrefix` the user has set.
const SRC_PREFIX: &str = "--src-prefix=a/";
const DST_PREFIX: &str = "--dst-prefix=b/";
const ERROR_LIMIT: usize = 32_000;

#[derive(Clone, Copy, Debug)]
pub enum DiffSide {
    Staged,
    Worktree,
}

/// A request always carries a resolved repository and a generation.
#[derive(Clone, Debug)]
pub enum Operation {
    Status,
    UntrackedDiff {
        path: PathBuf,
    },
    Diff {
        side: DiffSide,
        path: PathBuf,
        original_path: Option<PathBuf>,
    },
    /// One file of a commit against its first parent (`None`: a root commit).
    CommitDiff {
        commit: String,
        parent: Option<String>,
        path: PathBuf,
        original_path: Option<PathBuf>,
    },
}

/// Which commits the graph lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphScope {
    /// Local and remote-tracking branches, tags and HEAD.
    All,
    /// Local branches, tags and HEAD.
    Local,
    /// One branch: `refs/heads/main` or `refs/remotes/origin/main`.
    Branch(String),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub repo: Repository,
    pub generation: u64,
    pub operation: Operation,
}

pub struct Reply {
    pub repo: RepoId,
    pub generation: u64,
    pub output: Vec<u8>,
}

struct Shared {
    active: Mutex<usize>,
    available: Condvar,
    limit: usize,
    repo_locks: Mutex<HashMap<RepoId, std::sync::Weak<RepoLock>>>,
}

#[derive(Clone)]
pub struct GitService {
    shared: Arc<Shared>,
    timeout: Duration,
    /// How long a timed-out or cancelled Git gets after SIGTERM before SIGKILL. Writes need it:
    /// Git removes `index.lock` on SIGTERM but not on SIGKILL. Read queries take no locks.
    grace: Duration,
}

/// Serializes operations on one repository. Waiters sleep on the condvar and wake as soon as
/// the holder releases; the timeout only bounds how late a cancellation is noticed.
#[derive(Default)]
struct RepoLock {
    busy: Mutex<bool>,
    released: Condvar,
}

struct RepoGuard<'a>(&'a RepoLock);
impl Drop for RepoGuard<'_> {
    fn drop(&mut self) {
        *self.0.busy.lock().unwrap() = false;
        self.0.released.notify_one();
    }
}

/// How often a waiter re-checks its cancellation flag while another operation holds the lock.
const CANCEL_CHECK: Duration = Duration::from_millis(100);

struct Permit<'a>(&'a Shared);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap() -= 1;
        self.0.available.notify_one();
    }
}

/// Variables that point Git at another repository, work tree or index.
fn is_routing_variable(key: &str) -> bool {
    matches!(
        key,
        "GIT_DIR"
            | "GIT_WORK_TREE"
            | "GIT_INDEX_FILE"
            | "GIT_COMMON_DIR"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_NAMESPACE"
            | "GIT_PREFIX"
            | "GIT_CEILING_DIRECTORIES"
            | "GIT_DISCOVERY_ACROSS_FILESYSTEM"
            | "GIT_LITERAL_PATHSPECS"
            | "GIT_GLOB_PATHSPECS"
            | "GIT_NOGLOB_PATHSPECS"
            | "GIT_ICASE_PATHSPECS"
            | "GIT_QUARANTINE_PATH"
    ) || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
        || key == "GIT_CONFIG_COUNT"
        || key == "GIT_CONFIG_PARAMETERS"
}

/// Whether the child has exited, without reaping it (it stays a zombie holding its PID).
fn exited(pid: u32) -> bool {
    // SAFETY: an all-zero siginfo_t is valid; waitid only writes into it.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a valid out pointer; WNOWAIT leaves the child waitable for `wait`.
    let found = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } == 0;
    #[cfg(target_os = "linux")]
    // SAFETY: waitid filled a SIGCHLD record, whose si_pid is set (0 when nothing exited).
    let pid = unsafe { info.si_pid() };
    #[cfg(not(target_os = "linux"))]
    let pid = info.si_pid;
    found && pid != 0
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "操作已取消")
}

impl GitService {
    pub fn new(concurrency: usize, timeout: Duration) -> io::Result<Self> {
        if !(1..=4).contains(&concurrency) {
            return Err(error("Git 并发必须在 1 至 4 之间"));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                active: Mutex::new(0),
                available: Condvar::new(),
                limit: concurrency,
                repo_locks: Mutex::new(HashMap::new()),
            }),
            timeout,
            grace: Duration::ZERO,
        })
    }

    fn permit(&self, cancel: &AtomicBool) -> io::Result<Permit<'_>> {
        let mut active = self.shared.active.lock().unwrap();
        while *active >= self.shared.limit {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            active = self
                .shared
                .available
                .wait_timeout(active, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        *active += 1;
        Ok(Permit(&self.shared))
    }

    fn run(&self, root: &Path, args: &[OsString], cancel: &AtomicBool) -> io::Result<Vec<u8>> {
        self.run_command(root, args, cancel, false)
    }

    fn run_command(
        &self,
        root: &Path,
        args: &[OsString],
        cancel: &AtomicBool,
        allow_difference: bool,
    ) -> io::Result<Vec<u8>> {
        self.run_with_input(root, args, cancel, allow_difference, None)
    }

    fn run_with_input(
        &self,
        root: &Path,
        args: &[OsString],
        cancel: &AtomicBool,
        allow_difference: bool,
        input: Option<&[u8]>,
    ) -> io::Result<Vec<u8>> {
        let _permit = self.permit(cancel)?;
        let started = Instant::now();
        let mut command = Command::new("git");
        // A desktop launch must not inherit another shell's repository/index routing; the rest
        // (GIT_SSH_COMMAND, GIT_ASKPASS, GIT_CONFIG_GLOBAL, ...) is the user's and stays.
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(is_routing_variable) {
                command.env_remove(key);
            }
        }
        let mut child = command
            .arg("--no-optional-locks")
            .args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut output = Output::new(OUTPUT_LIMIT);
        let mut errors = Output::new(ERROR_LIMIT);
        let mut stdin = child.stdin.take();
        let mut remaining = input.unwrap_or_default();
        // Nonblocking pipes keep helpers holding a pipe inside the same deadline.
        let result = (|| {
            nonblocking(stdout.as_raw_fd())?;
            nonblocking(stderr.as_raw_fd())?;
            if let Some(stdin) = &stdin {
                nonblocking(stdin.as_raw_fd())?;
            }
            let mut status = None;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(cancelled());
                }
                if started.elapsed() >= self.timeout {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "Git 操作超时"));
                }
                if let Some(pipe) = &mut stdin {
                    if !remaining.is_empty() {
                        match pipe.write(remaining) {
                            Ok(n) => remaining = &remaining[n..],
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                            Err(e) => return Err(e),
                        }
                    }
                    if remaining.is_empty() {
                        stdin = None;
                    }
                }
                output.read_available(&mut stdout)?;
                errors.read_available(&mut stderr)?;
                if output.overflow {
                    return Err(error("Git 输出超过 16 MB；查询受限，不能判断为干净"));
                }
                if status.is_none() {
                    status = child.try_wait()?;
                }
                if output.closed
                    && errors.closed
                    && let Some(status) = status
                {
                    return Ok(status);
                }
                thread::sleep(Duration::from_millis(10));
            }
        })();
        if result.is_err() {
            let group = -(child.id() as i32);
            if !self.grace.is_zero() {
                // SAFETY: process_group(0) gave this child its own group; a negative PID targets that group.
                unsafe {
                    libc::kill(group, libc::SIGTERM);
                }
                let asked = Instant::now();
                while asked.elapsed() < self.grace && !exited(child.id()) {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            // Not reaped yet, so the group ID cannot have been reused; helpers die with it.
            // SAFETY: as above; the leader is at worst a zombie that still owns the group ID.
            unsafe {
                libc::kill(group, libc::SIGKILL);
            }
            let _ = child.wait();
        }
        let status = result?;
        // `git diff --no-index` returns 1 for a successful comparison with differences.
        if !status.success()
            && !(allow_difference && status.code() == Some(1) && errors.bytes.is_empty())
        {
            return Err(error(format!(
                "Git 退出码 {:?}: {}",
                status.code(),
                String::from_utf8_lossy(&errors.bytes)
            )));
        }
        Ok(output.bytes)
    }

    pub fn identify(&self, path: &Path, cancel: &AtomicBool) -> io::Result<Repository> {
        // One process answers all three; discovery runs this once per repository.
        let bytes = self.run(
            path,
            &[
                "rev-parse".into(),
                "--path-format=absolute".into(),
                "--git-dir".into(),
                "--show-toplevel".into(),
                "--git-common-dir".into(),
            ],
            cancel,
        )?;
        let mut lines = bytes
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| fs::canonicalize(PathBuf::from(OsString::from_vec(line.to_vec()))));
        let mut next = || {
            lines
                .next()
                .unwrap_or_else(|| Err(error("git rev-parse 输出不完整")))
        };
        Ok(Repository {
            id: RepoId(next()?),
            worktree: next()?,
            common_dir: next()?,
        })
    }

    pub fn execute(&self, request: &Request, cancel: &AtomicBool) -> io::Result<Reply> {
        let lock = self.repo_lock(&request.repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        self.execute_locked(request, cancel)
    }

    fn repo_lock(&self, id: &RepoId) -> Arc<RepoLock> {
        let mut locks = self.shared.repo_locks.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        let lock = locks.get(id).and_then(|l| l.upgrade()).unwrap_or_default();
        locks.insert(id.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Waits for the repository without holding a concurrency permit (permits are taken per
    /// Git process, after this lock).
    fn lock_repo<'a>(&self, lock: &'a RepoLock, cancel: &AtomicBool) -> io::Result<RepoGuard<'a>> {
        let mut busy = lock.busy.lock().unwrap();
        while *busy {
            if cancel.load(Ordering::Relaxed) {
                return Err(cancelled());
            }
            busy = lock.released.wait_timeout(busy, CANCEL_CHECK).unwrap().0;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        *busy = true;
        Ok(RepoGuard(lock))
    }

    fn execute_locked(&self, request: &Request, cancel: &AtomicBool) -> io::Result<Reply> {
        let (args, allow_difference) = match &request.operation {
            Operation::UntrackedDiff { path } => {
                validate_relative_path(path)?;
                // The selected row may have been staged since its last status snapshot.
                let tracked = self.run(
                    &request.repo.worktree,
                    &[
                        "--literal-pathspecs".into(),
                        "ls-files".into(),
                        "--cached".into(),
                        "-z".into(),
                        "--".into(),
                        path.as_os_str().to_owned(),
                    ],
                    cancel,
                )?;
                if !tracked.is_empty() {
                    (
                        vec![
                            "--literal-pathspecs".into(),
                            "diff".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            "--no-color".into(),
                            FULL_CONTEXT.into(),
                            SRC_PREFIX.into(),
                            DST_PREFIX.into(),
                            "--".into(),
                            path.as_os_str().to_owned(),
                        ],
                        false,
                    )
                } else {
                    let absolute = request.repo.worktree.join(path);
                    let parent =
                        fs::canonicalize(absolute.parent().ok_or_else(|| error("无效文件路径"))?)?;
                    if !parent.starts_with(&request.repo.worktree) {
                        return Err(error("未跟踪文件位于工作树之外"));
                    }
                    let kind = fs::symlink_metadata(&absolute)?.file_type();
                    if !kind.is_file() && !kind.is_symlink() {
                        return Err(error("Diff 仅支持文件与符号链接"));
                    }
                    (
                        vec![
                            "diff".into(),
                            "--no-index".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            "--no-color".into(),
                            FULL_CONTEXT.into(),
                            SRC_PREFIX.into(),
                            DST_PREFIX.into(),
                            "--".into(),
                            "/dev/null".into(),
                            path.as_os_str().to_owned(),
                        ],
                        true,
                    )
                }
            }
            Operation::Status => (
                vec![
                    "status".into(),
                    "--porcelain=v2".into(),
                    "-z".into(),
                    "--branch".into(),
                    "--untracked-files=all".into(),
                ],
                false,
            ),
            Operation::Diff {
                side,
                path,
                original_path,
            } => {
                validate_relative_path(path)?;
                let mut args: Vec<OsString> = vec![
                    "--literal-pathspecs".into(),
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-color".into(),
                    FULL_CONTEXT.into(),
                    SRC_PREFIX.into(),
                    DST_PREFIX.into(),
                ];
                // A staged rename shows as one entry even with `diff.renames=false`.
                args.push("-M".into());
                if matches!(side, DiffSide::Staged) {
                    args.push("--cached".into());
                }
                args.extend(["--".into(), path.as_os_str().to_owned()]);
                if let Some(original) = original_path {
                    validate_relative_path(original)?;
                    args.push(original.as_os_str().to_owned());
                }
                (args, false)
            }
            Operation::CommitDiff {
                commit,
                parent,
                path,
                original_path,
            } => {
                validate_relative_path(path)?;
                if !graph::is_hash(commit) || parent.as_deref().is_some_and(|p| !graph::is_hash(p))
                {
                    return Err(error("无效的提交"));
                }
                let base = match parent {
                    Some(parent) => parent.clone(),
                    None => self.empty_tree(&request.repo, cancel)?,
                };
                let mut args: Vec<OsString> = vec![
                    "--literal-pathspecs".into(),
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "--no-color".into(),
                    "-M".into(),
                    FULL_CONTEXT.into(),
                    SRC_PREFIX.into(),
                    DST_PREFIX.into(),
                    base.into(),
                    commit.into(),
                    "--".into(),
                    path.as_os_str().to_owned(),
                ];
                if let Some(original) = original_path {
                    validate_relative_path(original)?;
                    args.push(original.as_os_str().to_owned());
                }
                (args, false)
            }
        };
        let output = self.run_command(&request.repo.worktree, &args, cancel, allow_difference)?;
        Ok(Reply {
            repo: request.repo.id.clone(),
            generation: request.generation,
            output,
        })
    }

    pub fn status(
        &self,
        repo: &Repository,
        generation: u64,
        cancel: &AtomicBool,
    ) -> io::Result<Status> {
        let lock = self.repo_lock(&repo.id);
        let _serial = self.lock_repo(&lock, cancel)?;
        self.status_locked(repo, generation, cancel)
    }

    fn status_locked(
        &self,
        repo: &Repository,
        generation: u64,
        cancel: &AtomicBool,
    ) -> io::Result<Status> {
        let before = write::snapshot_version(repo)?;
        let reply = self.execute_locked(
            &Request {
                repo: repo.clone(),
                generation,
                operation: Operation::Status,
            },
            cancel,
        )?;
        let mut status = parse_status(&reply.output)?;
        let after = write::snapshot_version(repo)?;
        if before != after {
            return Err(error("Git 索引在查询期间发生变化，请刷新"));
        }
        status.version = write::worktree_version(repo, &status, after)?;
        Ok(status)
    }

    /// Local and remote-tracking branches, most recently committed first.
    pub fn branches(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<Branch>> {
        let output = self.run(
            &repo.worktree,
            &[
                "for-each-ref".into(),
                "--sort=-committerdate".into(),
                refs::BRANCH_FORMAT.into(),
                "refs/heads".into(),
                "refs/remotes".into(),
            ],
            cancel,
        )?;
        parse_branches(&output)
    }

    /// A page of the commit graph, children before parents, newest first.
    pub fn graph(
        &self,
        repo: &Repository,
        scope: &GraphScope,
        skip: usize,
        limit: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<GraphCommit>> {
        let mut args: Vec<OsString> = vec![
            "log".into(),
            "-z".into(),
            "--no-color".into(),
            "--no-show-signature".into(),
            "--date-order".into(),
            "--decorate=full".into(),
            graph::GRAPH_FORMAT.into(),
            format!("--skip={skip}").into(),
            format!("--max-count={limit}").into(),
        ];
        match scope {
            GraphScope::All => {
                args.extend(["--branches".into(), "--remotes".into(), "--tags".into()])
            }
            GraphScope::Local => args.extend(["--branches".into(), "--tags".into()]),
            GraphScope::Branch(name) => {
                if !(name.starts_with("refs/heads/") || name.starts_with("refs/remotes/")) {
                    return Err(error("无效的分支"));
                }
                args.push(name.into());
            }
        }
        // A detached HEAD is on no branch; an unborn one has nothing to list.
        if !matches!(scope, GraphScope::Branch(_))
            && self
                .run(
                    &repo.worktree,
                    &[
                        "rev-parse".into(),
                        "--verify".into(),
                        "--quiet".into(),
                        "HEAD".into(),
                    ],
                    cancel,
                )
                .is_ok()
        {
            args.push("HEAD".into());
        }
        args.push("--".into());
        parse_graph(&self.run(&repo.worktree, &args, cancel)?)
    }

    /// The whole message, both identities and the files changed against the first parent.
    pub fn commit_details(
        &self,
        repo: &Repository,
        hash: &str,
        cancel: &AtomicBool,
    ) -> io::Result<CommitDetails> {
        if !graph::is_hash(hash) {
            return Err(error("无效的提交"));
        }
        let show = self.run(
            &repo.worktree,
            &[
                "show".into(),
                "-s".into(),
                "--no-color".into(),
                "--no-show-signature".into(),
                graph::DETAILS_FORMAT.into(),
                hash.into(),
            ],
            cancel,
        )?;
        let mut details = graph::parse_details(&show)?;
        let base = match details.parents.first() {
            Some(parent) => parent.clone(),
            None => self.empty_tree(repo, cancel)?,
        };
        let files = self.run(
            &repo.worktree,
            &[
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                "-z".into(),
                "-M".into(),
                "--name-status".into(),
                base.into(),
                details.hash.clone().into(),
                "--".into(),
            ],
            cancel,
        )?;
        details.files = parse_name_status(&files)?;
        Ok(details)
    }

    /// The empty tree's name in this repository's hash (SHA-1 or SHA-256).
    fn empty_tree(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<String> {
        let output = self.run_with_input(
            &repo.worktree,
            &[
                "hash-object".into(),
                "-t".into(),
                "tree".into(),
                "--stdin".into(),
            ],
            cancel,
            false,
            Some(b""),
        )?;
        let hash = String::from_utf8_lossy(&output).trim().to_string();
        if !graph::is_hash(&hash) {
            return Err(error("无法计算空树"));
        }
        Ok(hash)
    }

    /// Commits on HEAD that its upstream does not have, newest first, at most `limit`.
    pub fn outgoing(
        &self,
        repo: &Repository,
        limit: usize,
        cancel: &AtomicBool,
    ) -> io::Result<Vec<Commit>> {
        let output = self.run(
            &repo.worktree,
            &[
                "log".into(),
                "-z".into(),
                "--no-color".into(),
                "--no-show-signature".into(),
                format!("--max-count={limit}").into(),
                "--format=%h%x00%at%x00%s".into(),
                "@{upstream}..HEAD".into(),
                "--".into(),
            ],
            cancel,
        )?;
        parse_log(&output)
    }

    /// Lists tracked and non-ignored untracked files below `dir`, relative to `dir`.
    ///
    /// Fails outside a Git worktree; callers fall back to a bounded directory walk.
    /// Which of `paths` (relative to the worktree; directories end with `/`) Git ignores.
    /// One process for the whole batch, so file watching never spawns Git per event.
    pub fn check_ignore(
        &self,
        repo: &Repository,
        paths: &[PathBuf],
        cancel: &AtomicBool,
    ) -> io::Result<Vec<PathBuf>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let mut input = Vec::new();
        for path in paths {
            validate_relative_path(path)?;
            input.extend_from_slice(path.as_os_str().as_encoded_bytes());
            input.push(0);
        }
        let output = self.run_with_input(
            &repo.worktree,
            &["check-ignore".into(), "--stdin".into(), "-z".into()],
            cancel,
            true,
            Some(&input),
        )?;
        Ok(output
            .split(|b| *b == 0)
            .filter(|item| !item.is_empty())
            .map(|item| PathBuf::from(OsString::from_vec(item.to_vec())))
            .collect())
    }

    pub fn list_files(&self, dir: &Path, cancel: &AtomicBool) -> io::Result<Vec<u8>> {
        self.run(
            dir,
            &[
                "ls-files".into(),
                "-z".into(),
                "--stage".into(),
                "--cached".into(),
                "--others".into(),
                "--exclude-standard".into(),
            ],
            cancel,
        )
    }

    /// Directories (absolute) that `repo`'s ignore rules exclude, collapsed to the topmost
    /// ignored directory, so discovery never walks `build/`, caches or virtualenvs.
    pub fn ignored_dirs(&self, repo: &Repository, cancel: &AtomicBool) -> io::Result<Vec<PathBuf>> {
        let output = self.run(
            &repo.worktree,
            &[
                "ls-files".into(),
                "-z".into(),
                "--others".into(),
                "--ignored".into(),
                "--exclude-standard".into(),
                "--directory".into(),
                "--no-empty-directory".into(),
            ],
            cancel,
        )?;
        Ok(output
            .split(|b| *b == 0)
            .filter_map(|item| item.strip_suffix(b"/"))
            .filter(|item| !item.is_empty())
            .map(|item| repo.worktree.join(OsString::from_vec(item.to_vec())))
            .collect())
    }

    /// Breadth-first, at most [`DISCOVERY_DEPTH`] levels below each root. Nested repositories are
    /// found below repositories, but directories the enclosing repository ignores, and
    /// [`EXCLUDED_DIRS`](workspace_editor_core::EXCLUDED_DIRS), are only checked for being a
    /// repository themselves, never walked. A `.git` entry Git would reject (tool caches leave
    /// such files) is skipped silently.
    pub fn discover(
        &self,
        roots: &[PathBuf],
        cancel: &AtomicBool,
        mut event: impl FnMut(Discovery),
    ) {
        let started = Instant::now();
        let mut visited = HashSet::new();
        let mut repos = HashSet::new();
        let mut invalid = 0usize;
        // (directory, depth, ignored directories of the enclosing repository)
        let mut queue: VecDeque<(PathBuf, usize, Arc<HashSet<PathBuf>>)> = roots
            .iter()
            .map(|root| (root.clone(), 0, Arc::default()))
            .collect();
        while let Some((path, depth, ignored)) = queue.pop_front() {
            if cancel.load(Ordering::Relaxed) {
                event(Discovery::Cancelled);
                return;
            }
            let path = match fs::canonicalize(&path) {
                Ok(path) => path,
                Err(e) => {
                    event(Discovery::Issue(path, e.to_string()));
                    continue;
                }
            };
            if !visited.insert(path.clone()) {
                continue;
            }
            if visited.len() > DISCOVERY_DIRS {
                event(Discovery::Issue(
                    path,
                    format!("扫描超过 {DISCOVERY_DIRS} 个目录，更深的仓库未发现"),
                ));
                break;
            }
            let mut ignored = ignored;
            let mut descend = true;
            match git_marker(&path) {
                GitMarker::Valid => match self.identify(&path, cancel) {
                    Ok(repo) => {
                        if repos.len() >= DISCOVERY_REPOS {
                            event(Discovery::Issue(
                                path,
                                format!("仓库超过 {DISCOVERY_REPOS} 个，其余未显示"),
                            ));
                            break;
                        }
                        if repos.insert(repo.id.clone()) {
                            // The repository's own ignore rules decide what is walked below it.
                            ignored = Arc::new(
                                self.ignored_dirs(&repo, cancel)
                                    .unwrap_or_default()
                                    .into_iter()
                                    .collect(),
                            );
                            event(Discovery::Repository(repo));
                        }
                    }
                    Err(e) => event(Discovery::Issue(path.clone(), e.to_string())),
                },
                GitMarker::Invalid => {
                    invalid += 1;
                    // Not a repository; whatever left it there is not walked either.
                    descend = false;
                }
                GitMarker::Missing => {}
            }
            if !descend || depth >= DISCOVERY_DEPTH {
                continue;
            }
            let entries = match fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(e) => {
                    event(Discovery::Issue(path, e.to_string()));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        event(Discovery::Issue(path.clone(), e.to_string()));
                        continue;
                    }
                };
                match entry.file_type() {
                    Ok(t) if t.is_dir() => {
                        let child = entry.path();
                        if is_excluded_dir(&entry.file_name()) {
                            event(Discovery::Excluded(child));
                        } else if ignored.contains(&child) {
                            // An ignored directory may itself be a repository (workspaces that
                            // ignore their sub-repositories); it is checked but not walked.
                            if git_marker(&child) == GitMarker::Valid {
                                queue.push_back((child, depth + 1, Arc::default()));
                            } else {
                                event(Discovery::Excluded(child));
                            }
                        } else {
                            queue.push_back((child, depth + 1, ignored.clone()));
                        }
                    }
                    Ok(_) => {} // Do not follow directory symlinks.
                    Err(e) => event(Discovery::Issue(entry.path(), e.to_string())),
                }
            }
        }
        eprintln!(
            "event=discovery_finished repos={} dirs={} invalid_git={invalid} seconds={:.3}",
            repos.len(),
            visited.len(),
            started.elapsed().as_secs_f64()
        );
    }
}

/// Discovery walks at most this many levels below a workspace root.
pub const DISCOVERY_DEPTH: usize = 4;
/// Bounds on one discovery pass; reaching one is reported, not hidden.
pub const DISCOVERY_DIRS: usize = 50_000;
pub const DISCOVERY_REPOS: usize = 256;

#[derive(Debug)]
pub enum Discovery {
    Repository(Repository),
    Excluded(PathBuf),
    Issue(PathBuf, String),
    Cancelled,
}

fn validate_relative_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(error("Git 文件路径必须是仓库内的相对路径"));
    }
    Ok(())
}

fn nonblocking(fd: std::os::fd::RawFd) -> io::Result<()> {
    // SAFETY: fd is a live pipe owned by this function's caller. fcntl does not take ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: same live descriptor as above; F_SETFL only changes its status flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Output {
    bytes: Vec<u8>,
    limit: usize,
    overflow: bool,
    closed: bool,
}
impl Output {
    fn new(limit: usize) -> Self {
        Self {
            bytes: vec![],
            limit,
            overflow: false,
            closed: false,
        }
    }
    fn read_available(&mut self, reader: &mut impl Read) -> io::Result<()> {
        let mut buffer = [0; 8192];
        // Bound each pass so a continuous stdout stream cannot starve stderr or cancellation.
        for _ in 0..8 {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    self.closed = true;
                    return Ok(());
                }
                Ok(count) => {
                    let keep = count.min(self.limit.saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&buffer[..keep]);
                    self.overflow |= keep < count;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn only_routing_variables_are_cleared() {
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_CONFIG_KEY_0",
        ] {
            assert!(is_routing_variable(key), "{key}");
        }
        for key in [
            "GIT_SSH_COMMAND",
            "GIT_ASKPASS",
            "GIT_CONFIG_GLOBAL",
            "GIT_EXEC_PATH",
        ] {
            assert!(!is_routing_variable(key), "{key}");
        }
    }

    #[test]
    fn waiter_wakes_on_release_and_honours_cancel() {
        let service = GitService::new(1, Duration::from_secs(5)).unwrap();
        let lock = Arc::new(RepoLock::default());
        let guard = service.lock_repo(&lock, &AtomicBool::new(false)).unwrap();
        let waiter = {
            let (service, lock) = (service.clone(), lock.clone());
            thread::spawn(move || {
                let started = Instant::now();
                let _guard = service.lock_repo(&lock, &AtomicBool::new(false)).unwrap();
                started.elapsed()
            })
        };
        thread::sleep(Duration::from_millis(150));
        drop(guard);
        let waited = waiter.join().unwrap();
        // Woken by the release, not by a polling interval after it.
        assert!(waited < Duration::from_millis(400), "{waited:?}");

        let _held = service.lock_repo(&lock, &AtomicBool::new(false)).unwrap();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            service.lock_repo(&lock, &cancel).err().unwrap().kind(),
            io::ErrorKind::Interrupted
        );
    }
}
