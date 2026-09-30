//! Bounded, read-only filesystem work. Display strings never become paths.
use std::{
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use workspace_editor_core::DocumentId;

pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OPEN_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_RESULTS: usize = 300;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;
const MAX_SEARCH_ENTRIES: usize = 100_000;

#[derive(Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub symlink: bool,
}

pub fn directory(path: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in fs::read_dir(path)? {
        let item = item?;
        if item.file_name() == ".git" {
            continue;
        }
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err(io::Error::other("目录超过 20,000 项，请选择更小的工作区"));
        }
        let kind = item.file_type()?;
        entries.push(Entry {
            path: item.path(),
            directory: kind.is_dir(),
            symlink: kind.is_symlink(),
        });
    }
    entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.path.cmp(&b.path)));
    Ok(entries)
}

#[derive(Default)]
pub struct SearchResults {
    pub paths: Vec<PathBuf>,
    pub incomplete: bool,
    pub errors: usize,
}

pub fn search(root: &Path, query: &str, cancel: &AtomicBool) -> SearchResults {
    let mut result = SearchResults::default();
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return result;
    }
    let started = Instant::now();
    let mut pending = vec![root.to_path_buf()];
    let mut inspected = 0;
    while let Some(path) = pending.pop() {
        let entries = match directory(&path) {
            Ok(entries) => entries,
            Err(_) => {
                result.errors += 1;
                continue;
            }
        };
        for entry in entries {
            inspected += 1;
            if cancel.load(Ordering::Relaxed)
                || inspected > MAX_SEARCH_ENTRIES
                || started.elapsed() > Duration::from_secs(10)
            {
                result.incomplete = true;
                return result;
            }
            if entry.symlink {
                continue;
            }
            if entry.directory {
                let name = entry.path.file_name().unwrap_or_default();
                if !["target", "node_modules", ".venv", "__pycache__"]
                    .iter()
                    .any(|excluded| name == *excluded)
                {
                    pending.push(entry.path);
                }
            } else if entry
                .path
                .strip_prefix(root)
                .unwrap_or(&entry.path)
                .to_string_lossy()
                .to_lowercase()
                .contains(&query)
            {
                result.paths.push(entry.path);
                if result.paths.len() == MAX_RESULTS {
                    result.incomplete = true;
                    return result;
                }
            }
        }
    }
    result
}

pub struct TextFile {
    pub id: DocumentId,
    pub path: PathBuf,
    pub text: String,
    pub bytes: usize,
    pub readonly: bool,
}

// Anchor every component at an open directory; a replaced ancestor cannot redirect the read.
fn open_beneath(root: &Path, relative: &Path) -> io::Result<fs::File> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::other("文件路径必须位于工作区内"));
        };
        let name = std::ffi::CString::new(name.as_bytes()).map_err(io::Error::other)?;
        let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        if components.peek().is_some() {
            flags |= libc::O_DIRECTORY;
        }
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // openat returned a new owned descriptor, which File closes on all exit paths.
        file = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(file)
}

// None is reserved for an explicit file-picker selection, including files outside a workspace.
pub fn text_file(root: Option<&Path>, path: &Path) -> io::Result<TextFile> {
    let path = fs::canonicalize(path)?;
    let root = match root {
        Some(root) => fs::canonicalize(root)?,
        None => path
            .parent()
            .ok_or_else(|| io::Error::other("所选文件没有父目录"))?
            .to_path_buf(),
    };
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| io::Error::other("链接目标位于工作区之外，未读取"))?;
    let file = open_beneath(&root, relative)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return Err(io::Error::other("仅支持不超过 8 MiB 的普通 UTF-8 文本文件"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE_BYTES || bytes.contains(&0) {
        return Err(io::Error::other("文件过大或包含二进制内容，未打开"));
    }
    let size = bytes.len();
    let text =
        String::from_utf8(bytes).map_err(|_| io::Error::other("文件不是 UTF-8 文本，未打开"))?;
    if text.split('\n').any(|line| line.len() > 256 * 1024) {
        return Err(io::Error::other("单行超过 256 KiB，当前原型未打开"));
    }
    Ok(TextFile {
        id: DocumentId {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        path,
        text: text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned(),
        bytes: size,
        readonly: metadata.nlink() > 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsing_search_and_text_are_bounded_and_read_only() {
        let root = std::env::temp_dir().join(format!("zj-files-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("src")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        let path = root.join("src/中文 main.rs");
        fs::write(&path, "\u{feff}hello\r\n").unwrap();
        fs::write(root.join("target/main.rs"), "ignored").unwrap();
        fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        std::os::unix::fs::symlink("src", root.join("alias")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", root.join("outside")).unwrap();
        let entries = directory(&root).unwrap();
        assert!(entries[0].directory);
        assert!(
            !entries
                .iter()
                .any(|entry| entry.path.file_name().unwrap() == ".git")
        );
        let result = search(&root, "MAIN.RS", &AtomicBool::new(false));
        assert_eq!(result.paths, vec![path.clone()]);
        assert!(!result.incomplete);
        assert!(search(&root, "main", &AtomicBool::new(true)).incomplete);
        let loaded = text_file(Some(&root), &path).unwrap();
        assert_eq!(loaded.text, "hello\r\n");
        assert!(!loaded.readonly);
        fs::hard_link(&path, root.join("hard")).unwrap();
        assert!(text_file(Some(&root), &path).unwrap().readonly);
        assert!(text_file(Some(&root), &root.join("outside")).is_err());
        let outside = root.with_extension("selected.txt");
        fs::write(&outside, "explicitly selected\n").unwrap();
        assert!(text_file(Some(&root), &outside).is_err());
        assert_eq!(
            text_file(None, &outside).unwrap().text,
            "explicitly selected\n"
        );
        std::os::unix::fs::symlink(&outside, root.join("selected-link")).unwrap();
        assert_eq!(
            text_file(None, &root.join("selected-link")).unwrap().id,
            text_file(None, &outside).unwrap().id
        );
        assert!(open_beneath(&root, Path::new("alias/中文 main.rs")).is_err());
        assert!(text_file(Some(&root), &root.join("binary")).is_err());
        assert!(text_file(None, &root.join("binary")).is_err());
        fs::write(root.join("other-encoding"), [0xff, 0xfe]).unwrap();
        assert!(text_file(Some(&root), &root.join("other-encoding")).is_err());
        let fifo = std::ffi::CString::new(root.join("pipe").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(text_file(Some(&root), &root.join("pipe")).is_err());
        assert!(text_file(None, &root.join("pipe")).is_err());
        let large = fs::File::create(root.join("large")).unwrap();
        large.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        assert!(text_file(Some(&root), &root.join("large")).is_err());
        fs::write(root.join("long-line"), "x".repeat(256 * 1024 + 1)).unwrap();
        assert!(text_file(Some(&root), &root.join("long-line")).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "\u{feff}hello\r\n");
        assert_eq!(
            fs::read_to_string(&outside).unwrap(),
            "explicitly selected\n"
        );
        fs::remove_file(outside).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
}
