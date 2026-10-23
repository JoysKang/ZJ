//! The client side of ACP `fs/*`: workspace confinement, bounded reads, atomic writes.

use std::{
    fs, io,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

/// Larger files are refused rather than streamed into the agent's context.
pub const READ_LIMIT: u64 = 16 * 1024 * 1024;

/// Unsaved editor contents, so the agent sees what the user sees.
///
/// Asynchronous because the editor answers from its UI thread; the client waits a bounded
/// time and gives up when it shuts down, so the provider never has to be answered.
pub trait BufferProvider: Send + Sync {
    /// Full text of an open buffer with unsaved edits at this absolute, canonical path;
    /// `None` when there is none (the file on disk is used).
    fn buffer_text(&self, path: &Path) -> futures::future::BoxFuture<'static, Option<String>>;
}

/// Canonical workspace root plus the checks every agent path goes through.
#[derive(Clone, Debug)]
pub(crate) struct Workspace {
    root: PathBuf,
}

fn outside(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("{} 不在工作区内", path.display()),
    )
}

impl Workspace {
    pub fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            root: fs::canonicalize(root)?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path inside the workspace with symlinks resolved. A path that does not exist
    /// yet (a new file) is resolved through its nearest existing ancestor; `..` is refused.
    pub fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} 不是绝对路径", path.display()),
            ));
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(outside(path));
        }
        let mut existing = path.to_path_buf();
        let mut rest: Vec<std::ffi::OsString> = Vec::new();
        let canonical = loop {
            match fs::canonicalize(&existing) {
                Ok(c) => break c,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let Some(name) = existing.file_name() else {
                        return Err(e);
                    };
                    rest.push(name.to_os_string());
                    if !existing.pop() {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            }
        };
        let mut resolved = canonical;
        for name in rest.into_iter().rev() {
            resolved.push(name);
        }
        if !resolved.starts_with(&self.root) {
            return Err(outside(path));
        }
        Ok(resolved)
    }
}

fn too_large(path: &Path) -> io::Error {
    io::Error::other(format!(
        "{} 超过 {} MB，不读取",
        path.display(),
        READ_LIMIT / 1024 / 1024
    ))
}

/// Reads only regular text files, checking the opened file and bounding the actual bytes.
pub(crate) fn read_disk(path: &Path) -> io::Result<String> {
    // Opening first avoids trusting metadata for a path that may have been replaced. A FIFO
    // must not block in open before we can inspect the handle and reject it.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} 不是普通文件，不读取", path.display()),
        ));
    }
    if meta.len() > READ_LIMIT {
        return Err(too_large(path));
    }
    read_bounded(file, path)
}

fn read_bounded(reader: impl Read, path: &Path) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.take(READ_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > READ_LIMIT {
        return Err(too_large(path));
    }
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} 不是 UTF-8 文本", path.display()),
        )
    })
}

pub(crate) fn window(text: String, line: Option<u32>, limit: Option<u32>) -> String {
    if line.is_none() && limit.is_none() {
        return text;
    }
    let skip = line.map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
    let take = limit.map(|l| l as usize).unwrap_or(usize::MAX);
    text.split_inclusive('\n').skip(skip).take(take).collect()
}

/// Write to a temporary sibling, then rename; keeps the original file's permissions.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("路径没有父目录"))?;
    fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("路径没有文件名"))?
        .to_string_lossy();
    // Unique per write: two sessions (or a session and a review) writing the same file at
    // once must not truncate each other's temporary file.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = parent.join(format!(
        ".{name}.zj-agent-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    write_atomic_to(path, text, &tmp)
}

fn write_atomic_to(path: &Path, text: &str, tmp: &Path) -> io::Result<()> {
    // A failed create does not give this write ownership of an existing temporary file.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(tmp, fs::Permissions::from_mode(meta.permissions().mode()))?;
        }
        fs::rename(tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confinement() {
        let root = std::env::temp_dir().join(format!("zj-ws-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "x").unwrap();
        let ws = Workspace::new(&root).unwrap();
        let canon = ws.root().to_path_buf();
        assert_eq!(
            ws.resolve(&root.join("src/a.rs")).unwrap(),
            canon.join("src/a.rs")
        );
        assert_eq!(
            ws.resolve(&root.join("new/dir/b.rs")).unwrap(),
            canon.join("new/dir/b.rs")
        );
        assert!(ws.resolve(Path::new("/etc/passwd")).is_err());
        assert!(ws.resolve(&root.join("src/../../x")).is_err());
        assert!(ws.resolve(Path::new("relative.rs")).is_err());
        std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
        assert!(ws.resolve(&root.join("escape/passwd")).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_reject_special_files_and_enforce_the_byte_limit() {
        let root = std::env::temp_dir().join(format!("zj-read-guard-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert_eq!(
            read_disk(&root).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let pipe = root.join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            read_disk(&pipe).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let path = root.join("large.txt");
        let file = fs::File::create(&path).unwrap();
        file.set_len(READ_LIMIT + 1).unwrap();
        assert!(read_disk(&path).unwrap_err().to_string().contains("超过"));
        // The reader can produce more bytes than metadata promised (a file grew after open).
        let bytes = io::repeat(b'x').take(READ_LIMIT + 2);
        assert!(
            read_bounded(bytes, &path)
                .unwrap_err()
                .to_string()
                .contains("超过")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_temp_creation_preserves_existing_files() {
        let root = std::env::temp_dir().join(format!("zj-write-guard-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("target.txt");
        let tmp = root.join("existing-temp");
        fs::write(&path, "original").unwrap();
        fs::write(&tmp, "owned by another writer").unwrap();
        assert_eq!(
            write_atomic_to(&path, "new", &tmp).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(fs::read_to_string(&tmp).unwrap(), "owned by another writer");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn line_window_is_one_based() {
        let text = "1\n2\n3\n4\n".to_string();
        assert_eq!(window(text.clone(), Some(2), Some(2)), "2\n3\n");
        assert_eq!(window(text.clone(), None, Some(1)), "1\n");
        assert_eq!(window(text, Some(4), None), "4\n");
    }
}
