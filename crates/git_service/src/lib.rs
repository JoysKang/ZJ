//! Bounded, cancellable system Git operations, serialized by worktree identity.

mod ls_files;
mod status;
mod write;
pub use ls_files::{ListedKind, parse_ls_files};
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
use workspace_editor_core::{RepoId, Repository, is_excluded_dir};

const OUTPUT_LIMIT: usize = 16_000_000;
/// Diffs carry the whole file so the diff editor can rebuild both sides.
const FULL_CONTEXT: &str = "--unified=1000000";
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
        // A desktop launch must not inherit another shell's repository/index routing.
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(|key| key.starts_with("GIT_")) {
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
            // SAFETY: process_group(0) gave this child its own group; a negative PID targets that group.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
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
        let resolve = |flag: &str| -> io::Result<PathBuf> {
            let mut bytes = self.run(
                path,
                &[
                    "rev-parse".into(),
                    "--path-format=absolute".into(),
                    flag.into(),
                ],
                cancel,
            )?;
            if bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            fs::canonicalize(PathBuf::from(OsString::from_vec(bytes)))
        };
        Ok(Repository {
            id: RepoId(resolve("--git-dir")?),
            worktree: resolve("--show-toplevel")?,
            common_dir: resolve("--git-common-dir")?,
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
                ];
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

    /// Lists tracked and non-ignored untracked files below `dir`, relative to `dir`.
    ///
    /// Fails outside a Git worktree; callers fall back to a bounded directory walk.
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

    /// Streaming discovery continues below repositories; ignore rules do not hide nested repos.
    pub fn discover(
        &self,
        roots: &[PathBuf],
        cancel: &AtomicBool,
        mut event: impl FnMut(Discovery),
    ) {
        let mut visited = HashSet::new();
        let mut repos = HashSet::new();
        let mut queue = VecDeque::from(roots.to_vec());
        while let Some(path) = queue.pop_front() {
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
            if path.join(".git").exists() {
                match self.identify(&path, cancel) {
                    Ok(repo) if repos.insert(repo.id.clone()) => event(Discovery::Repository(repo)),
                    Ok(_) => {}
                    Err(e) => event(Discovery::Issue(path.clone(), e.to_string())),
                }
            }
            match fs::read_dir(&path) {
                Ok(entries) => {
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
                                let name = entry.file_name();
                                if is_excluded_dir(&name) {
                                    event(Discovery::Excluded(entry.path()));
                                } else {
                                    queue.push_back(entry.path());
                                }
                            }
                            Ok(_) => {} // Do not follow directory symlinks.
                            Err(e) => event(Discovery::Issue(entry.path(), e.to_string())),
                        }
                    }
                }
                Err(e) => event(Discovery::Issue(path, e.to_string())),
            }
        }
    }
}

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
