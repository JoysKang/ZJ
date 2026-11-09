//! Local reading records. These contain identities and fingerprints, never buffer text.
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const MAX_REVIEWED: usize = 512;
pub const MAX_POSITIONS: usize = 20;
pub const MAX_PINNED: usize = 8;
pub const MAX_PATH_BYTES: usize = 4096;
pub const MAX_VERIFY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STORE_BYTES: u64 = 8 * 1024 * 1024;
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Repository checkpoints outlive workspace windows. Call these helpers on a background executor.
pub fn load_store(path: &Path) -> io::Result<Vec<ReviewCheckpoint>> {
    Ok(ReadingSession::from_json(&read_store(path)?).reviewed)
}

fn read_store(path: &Path) -> io::Result<Value> {
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("审阅记录不是普通文件"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_STORE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(io::Error::other("审阅记录超过 8 MiB"));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if !value.is_object() {
        return Err(io::Error::other("审阅记录不是 JSON 对象"));
    }
    Ok(value)
}

/// Each window serializes its actions; the process lock merges updates from other windows.
pub fn update_store(path: &Path, record: &ReviewCheckpoint, reviewed: bool) -> io::Result<()> {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let _lock = STORE_LOCK
        .lock()
        .map_err(|_| io::Error::other("审阅记录锁失效"))?;
    let mut value = read_store(path)?;
    let mut records = ReadingSession::from_json(&value).reviewed;
    if reviewed {
        records.retain(|saved| saved.repo != record.repo || saved.path != record.path);
        if records.len() == MAX_REVIEWED {
            records.remove(0);
        }
        records.push(record.clone());
    } else {
        // An old clear action cannot delete a newer checkpoint from another window.
        records.retain(|saved| saved != record);
    }
    value["reviewed"] = Value::Array(records.iter().map(ReviewCheckpoint::to_json).collect());
    let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(io::Error::other("审阅记录超过 8 MiB"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub device: u64,
    pub inode: u64,
    pub len: u64,
    /// FNV-1a over the bytes, with a fixed algorithm across restarts/builds.
    pub hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewCheckpoint {
    pub repo: PathBuf,
    pub repo_device: u64,
    pub repo_inode: u64,
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
}

/// A file is considered reviewed only after a successful content check in this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewStatus {
    Reviewed,
    Pending,
    Unavailable(String),
}

impl ReviewCheckpoint {
    /// Runs on a background executor. The caller may supply the last displayed disk version;
    /// a file changed since that version must not be marked as read without seeing it.
    pub fn capture(
        repo: &Path,
        path: &Path,
        displayed: Option<crate::save::DiskState>,
    ) -> io::Result<Self> {
        let repo = fs::canonicalize(repo)?;
        let path = fs::canonicalize(path)?;
        if !valid_absolute(&repo) {
            return Err(io::Error::other("仓库路径无法保存为审阅记录"));
        }
        let relative = path
            .strip_prefix(&repo)
            .map_err(|_| io::Error::other("文件不在选定仓库内"))?;
        if !relative_path(relative) {
            return Err(io::Error::other("无效的审阅路径"));
        }
        let metadata = fs::metadata(&repo)?;
        if !metadata.is_dir() {
            return Err(io::Error::other("仓库目录已失效"));
        }
        let (fingerprint, disk) = read_fingerprint(&path)?;
        if displayed.is_some_and(|expected| expected != disk) {
            return Err(io::Error::other("文件已在外部变化，请重新读取后标记已读"));
        }
        Ok(Self {
            repo,
            repo_device: metadata.dev(),
            repo_inode: metadata.ino(),
            path: relative.to_path_buf(),
            fingerprint,
        })
    }

    pub fn verify(&self) -> ReviewStatus {
        let result = (|| {
            let current = Self::capture(&self.repo, &self.repo.join(&self.path), None)?;
            Ok::<_, io::Error>(if &current == self {
                ReviewStatus::Reviewed
            } else {
                ReviewStatus::Pending
            })
        })();
        result.unwrap_or_else(|error| ReviewStatus::Unavailable(error.to_string()))
    }

    fn to_json(&self) -> Value {
        json!({"repo":self.repo, "repo_device":self.repo_device, "repo_inode":self.repo_inode,
            "path":self.path,"fingerprint":{"device":self.fingerprint.device,"inode":self.fingerprint.inode,
            "len":self.fingerprint.len,"fnv1a64":format!("{:016x}", self.fingerprint.hash)}})
    }

    fn from_json(value: &Value) -> Option<Self> {
        let fingerprint = value.get("fingerprint")?;
        let len = fingerprint.get("len")?.as_u64()?;
        if len > crate::files::MAX_FILE_BYTES as u64 {
            return None;
        }
        let hash = fingerprint.get("fnv1a64")?.as_str()?;
        if hash.len() != 16 {
            return None;
        }
        Some(Self {
            repo: absolute(value.get("repo")?)?,
            path: relative(value.get("path")?)?,
            repo_device: value.get("repo_device")?.as_u64()?,
            repo_inode: value.get("repo_inode")?.as_u64()?,
            fingerprint: Fingerprint {
                device: fingerprint.get("device")?.as_u64()?,
                inode: fingerprint.get("inode")?.as_u64()?,
                len,
                hash: u64::from_str_radix(hash, 16).ok()?,
            },
        })
    }
}

fn read_fingerprint(path: &Path) -> io::Result<(Fingerprint, crate::save::DiskState)> {
    // A file replaced by a FIFO or symlink must not block the review worker or escape the repo.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)?;
    let before = file.metadata()?;
    if !before.is_file() || before.len() > crate::files::MAX_FILE_BYTES as u64 {
        return Err(io::Error::other("审阅指纹仅支持不超过 8 MiB 的普通文件"));
    }
    let mut bytes = Vec::with_capacity(before.len() as usize);
    (&file)
        .take(crate::files::MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if bytes.len() > crate::files::MAX_FILE_BYTES
        || crate::files::FileStamp::of(&before) != crate::files::FileStamp::of(&after)
        || crate::files::FileStamp::of(&after) != crate::files::FileStamp::read(path)?
        || after.len() != bytes.len() as u64
    {
        return Err(io::Error::other("读取期间文件发生变化，请重试"));
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in &bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    Ok((
        Fingerprint {
            device: after.dev(),
            inode: after.ino(),
            len: bytes.len() as u64,
            hash,
        },
        crate::save::DiskState::of(crate::files::FileStamp::of(&after), &bytes),
    ))
}

/// Checks are serial and bounded in file count, bytes and memory. No timer or Git writes.
pub fn verify_checkpoints(records: &[ReviewCheckpoint], cancel: &AtomicBool) -> Vec<ReviewStatus> {
    let mut remaining = MAX_VERIFY_BYTES;
    records
        .iter()
        .take(MAX_REVIEWED)
        .map(|record| {
            let size = fs::metadata(record.repo.join(&record.path))
                .map(|m| m.len())
                .unwrap_or(0);
            if cancel.load(Ordering::Relaxed) || size > remaining {
                ReviewStatus::Pending
            } else {
                // A concurrent growth may cost one extra bounded file read.
                remaining -= size;
                record.verify()
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComparisonSource {
    Commit {
        repo: PathBuf,
        commit: String,
        parent: Option<String>,
        path: PathBuf,
        original_path: Option<PathBuf>,
    },
    History {
        repo: PathBuf,
        commit: String,
        path: PathBuf,
        current_path: Option<PathBuf>,
    },
    Files {
        left: PathBuf,
        right: PathBuf,
    },
    GitChange {
        repo: PathBuf,
        path: PathBuf,
        original_path: Option<PathBuf>,
        staged: bool,
        untracked: bool,
    },
    Git {
        repo: PathBuf,
        base: String,
        target: String,
        path: PathBuf,
        original_path: Option<PathBuf>,
    },
}

impl ComparisonSource {
    pub fn valid(&self) -> bool {
        match self {
            Self::Commit {
                repo,
                commit,
                parent,
                path,
                original_path,
            } => {
                valid_absolute(repo)
                    && valid_ref(commit)
                    && parent.as_deref().is_none_or(valid_ref)
                    && relative_path(path)
                    && original_path.as_deref().is_none_or(relative_path)
            }
            Self::History {
                repo,
                commit,
                path,
                current_path,
            } => {
                valid_absolute(repo)
                    && valid_ref(commit)
                    && relative_path(path)
                    && current_path.as_deref().is_none_or(relative_path)
            }
            Self::Files { left, right } => valid_absolute(left) && valid_absolute(right),
            Self::GitChange {
                repo,
                path,
                original_path,
                staged,
                untracked,
            } => {
                valid_absolute(repo)
                    && relative_path(path)
                    && original_path.as_deref().is_none_or(relative_path)
                    && !(*staged && *untracked)
            }
            Self::Git {
                repo,
                base,
                target,
                path,
                original_path,
            } => {
                valid_absolute(repo)
                    && valid_ref(base)
                    && valid_ref(target)
                    && relative_path(path)
                    && original_path.as_deref().is_none_or(relative_path)
            }
        }
    }
    pub fn path(&self) -> PathBuf {
        match self {
            Self::Files { right, .. } => right.clone(),
            Self::History {
                repo,
                path,
                current_path,
                ..
            } => repo.join(current_path.as_ref().unwrap_or(path)),
            Self::Commit { repo, path, .. }
            | Self::Git { repo, path, .. }
            | Self::GitChange { repo, path, .. } => repo.join(path),
        }
    }
    fn to_json(&self) -> Value {
        match self {
            Self::Commit {
                repo,
                commit,
                parent,
                path,
                original_path,
            } => {
                json!({"kind":"commit","repo":repo,"commit":commit,"parent":parent,"path":path,"original_path":original_path})
            }
            Self::History {
                repo,
                commit,
                path,
                current_path,
            } => {
                json!({"kind":"history","repo":repo,"commit":commit,"path":path,"current_path":current_path})
            }
            Self::Files { left, right } => json!({"kind":"files","left":left,"right":right}),
            Self::GitChange {
                repo,
                path,
                original_path,
                staged,
                untracked,
            } => {
                json!({"kind":"git_change","repo":repo,"path":path,"original_path":original_path,"staged":staged,"untracked":untracked})
            }
            Self::Git {
                repo,
                base,
                target,
                path,
                original_path,
            } => {
                json!({"kind":"git","repo":repo,"base":base,"target":target,"path":path,"original_path":original_path})
            }
        }
    }
    fn from_json(value: &Value) -> Option<Self> {
        let optional_path = |key| match value.get(key) {
            None | Some(Value::Null) => Some(None),
            Some(path) => Some(Some(relative(path)?)),
        };
        let source = match value.get("kind")?.as_str()? {
            "commit" => Self::Commit {
                repo: absolute(value.get("repo")?)?,
                commit: value.get("commit")?.as_str()?.into(),
                parent: match value.get("parent") {
                    None | Some(Value::Null) => None,
                    Some(parent) => Some(parent.as_str()?.into()),
                },
                path: relative(value.get("path")?)?,
                original_path: optional_path("original_path")?,
            },
            "history" => Self::History {
                repo: absolute(value.get("repo")?)?,
                commit: value.get("commit")?.as_str()?.into(),
                path: relative(value.get("path")?)?,
                current_path: optional_path("current_path")?,
            },
            "files" => Self::Files {
                left: absolute(value.get("left")?)?,
                right: absolute(value.get("right")?)?,
            },
            "git_change" => Self::GitChange {
                repo: absolute(value.get("repo")?)?,
                path: relative(value.get("path")?)?,
                original_path: match value.get("original_path") {
                    None | Some(Value::Null) => None,
                    Some(path) => Some(relative(path)?),
                },
                staged: value.get("staged")?.as_bool()?,
                untracked: value.get("untracked")?.as_bool()?,
            },
            "git" => Self::Git {
                repo: absolute(value.get("repo")?)?,
                base: value.get("base")?.as_str()?.into(),
                target: value.get("target")?.as_str()?.into(),
                path: relative(value.get("path")?)?,
                original_path: match value.get("original_path") {
                    None | Some(Value::Null) => None,
                    Some(path) => Some(relative(path)?),
                },
            },
            _ => return None,
        };
        source.valid().then_some(source)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilePosition {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    /// Negative Kit offsets, in screen points.
    pub scroll_x: f32,
    pub scroll_y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PinnedComparison {
    pub source: ComparisonSource,
    pub top_row: usize,
    /// Older sessions use the global default when no per-comparison layout was saved.
    pub inline: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingSession {
    pub positions: Vec<FilePosition>,
    pub pinned: Vec<PinnedComparison>,
    pub reviewed: Vec<ReviewCheckpoint>,
    pub extra: Map<String, Value>,
}

impl ReadingSession {
    pub fn from_json(value: &Value) -> Self {
        let mut extra = value.as_object().cloned().unwrap_or_default();
        let list = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
        };
        let positions = list("positions")
            .iter()
            .take(MAX_POSITIONS)
            .filter_map(|value| {
                let coordinate = |key| {
                    value
                        .get(key)?
                        .as_f64()
                        .map(|n| n as f32)
                        .filter(|n| n.is_finite() && *n <= 0. && *n >= -100_000_000.)
                };
                Some(FilePosition {
                    path: absolute(value.get("path")?)?,
                    line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
                    column: u32::try_from(value.get("column")?.as_u64()?).ok()?,
                    scroll_x: coordinate("scroll_x")?,
                    scroll_y: coordinate("scroll_y")?,
                })
            })
            .collect();
        let pinned = list("pinned")
            .iter()
            .take(MAX_PINNED)
            .filter_map(|value| {
                Some(PinnedComparison {
                    inline: value.get("inline").and_then(Value::as_bool),
                    source: ComparisonSource::from_json(value.get("source")?)?,
                    top_row: usize::try_from(value.get("top_row")?.as_u64()?)
                        .ok()?
                        .min(1_000_000),
                })
            })
            .collect();
        let reviewed = list("reviewed")
            .iter()
            .take(MAX_REVIEWED)
            .filter_map(ReviewCheckpoint::from_json)
            .collect();
        for key in ["positions", "pinned", "reviewed"] {
            extra.remove(key);
        }
        Self {
            positions,
            pinned,
            reviewed,
            extra,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut value = Value::Object(self.extra.clone());
        value["positions"] = Value::Array(self.positions.iter().take(MAX_POSITIONS).filter(|p| valid_absolute(&p.path) && p.scroll_x.is_finite() && p.scroll_y.is_finite()).map(|p| json!({"path":p.path,"line":p.line,"column":p.column,"scroll_x":p.scroll_x,"scroll_y":p.scroll_y})).collect());
        value["pinned"] = Value::Array(
            self.pinned
                .iter()
                .take(MAX_PINNED)
                .filter(|p| p.source.valid())
                .map(|p| json!({"source":p.source.to_json(),"top_row":p.top_row,"inline":p.inline}))
                .collect(),
        );
        value["reviewed"] = Value::Array(
            self.reviewed
                .iter()
                .take(MAX_REVIEWED)
                .map(ReviewCheckpoint::to_json)
                .collect(),
        );
        value
    }
}

fn valid_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('-')
        && !value.chars().any(char::is_control)
}
fn valid_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .to_str()
            .is_some_and(|path| path.len() <= MAX_PATH_BYTES && !path.contains('\0'))
}
fn absolute(value: &Value) -> Option<PathBuf> {
    let path = PathBuf::from(value.as_str()?);
    valid_absolute(&path).then_some(path)
}
fn relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .to_str()
            .is_some_and(|path| path.len() <= MAX_PATH_BYTES && !path.contains('\0'))
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
fn relative(value: &Value) -> Option<PathBuf> {
    let path = PathBuf::from(value.as_str()?);
    relative_path(&path).then_some(path)
}

#[cfg(test)]
#[path = "review_progress_tests.rs"]
mod tests;
