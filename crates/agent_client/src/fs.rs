//! The client side of ACP `fs/*`: workspace confinement, bounded reads, atomic writes.

use std::{
    fs, io,
    io::Write,
    os::unix::fs::PermissionsExt,
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

/// Reads a text file from disk with ACP's 1-based `line` and `limit` window.
pub(crate) fn read_disk(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    if meta.len() > READ_LIMIT {
        return Err(io::Error::other(format!(
            "{} 超过 {} MB，不读取",
            path.display(),
            READ_LIMIT / 1024 / 1024
        )));
    }
    let bytes = fs::read(path)?;
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
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(meta.permissions().mode()))?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
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
    fn line_window_is_one_based() {
        let text = "1\n2\n3\n4\n".to_string();
        assert_eq!(window(text.clone(), Some(2), Some(2)), "2\n3\n");
        assert_eq!(window(text.clone(), None, Some(1)), "1\n");
        assert_eq!(window(text, Some(4), None), "4\n");
    }
}
