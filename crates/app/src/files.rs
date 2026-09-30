//! Bounded, read-only filesystem work. Display strings never become paths.
use crate::fuzzy;
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
use workspace_editor_core::{DocumentId, is_excluded_dir};
use workspace_editor_git::{ListedKind, parse_ls_files};

pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OPEN_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_RESULTS: usize = 300;
const MAX_DIRECTORY_ENTRIES: usize = 20_000;
const MAX_INDEX_ENTRIES: usize = 200_000;
const INDEX_DEADLINE: Duration = Duration::from_secs(15);

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

struct IndexEntry {
    relative: PathBuf,
    /// Lowercased lossy display form, precomputed so matching allocates nothing per keystroke.
    key: Box<str>,
    name_start: usize,
}

/// Every file path of a workspace, built once in the background and matched in memory.
///
/// Git worktrees contribute `git ls-files` output (tracked plus non-ignored untracked files);
/// other directories use a bounded walk that skips [`EXCLUDED_DIRS`] and directory symlinks.
pub struct PathIndex {
    root: PathBuf,
    entries: Vec<IndexEntry>,
    pub incomplete: bool,
    pub errors: usize,
}

type ListFiles<'a> = dyn Fn(&Path, &AtomicBool) -> io::Result<Vec<u8>> + 'a;

impl PathIndex {
    /// `list_files` runs `git ls-files` for a directory; an error selects the directory walk.
    pub fn build(root: &Path, cancel: &AtomicBool, list_files: &ListFiles<'_>) -> Self {
        let mut builder = Builder {
            root,
            cancel,
            list_files,
            started: Instant::now(),
            index: PathIndex {
                root: root.to_path_buf(),
                entries: Vec::new(),
                incomplete: false,
                errors: 0,
            },
        };
        // The root may sit inside a worktree without its own `.git`, so always try Git first.
        if !builder.list_git(Path::new("")) {
            builder.walk(Path::new(""));
        }
        builder.index
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Ranks all entries against `query` and returns the best [`MAX_RESULTS`] as absolute paths.
    pub fn search(&self, query: &str) -> SearchResults {
        let query = fuzzy::query_chars(query);
        let mut result = SearchResults {
            errors: self.errors,
            incomplete: self.incomplete,
            ..Default::default()
        };
        if query.is_empty() {
            return result;
        }
        let mut scored: Vec<(i32, &IndexEntry)> = self
            .entries
            .iter()
            .filter_map(|e| fuzzy::score(&query, &e.key, e.name_start).map(|s| (s, e)))
            .collect();
        let order = |a: &(i32, &IndexEntry), b: &(i32, &IndexEntry)| {
            b.0.cmp(&a.0)
                .then(a.1.key.len().cmp(&b.1.key.len()))
                .then(a.1.key.cmp(&b.1.key))
        };
        if scored.len() > MAX_RESULTS {
            scored.select_nth_unstable_by(MAX_RESULTS, order);
            scored.truncate(MAX_RESULTS);
            result.incomplete = true;
        }
        scored.sort_unstable_by(order);
        result.paths = scored
            .into_iter()
            .map(|(_, e)| self.root.join(&e.relative))
            .collect();
        result
    }
}

struct Builder<'a> {
    root: &'a Path,
    cancel: &'a AtomicBool,
    list_files: &'a ListFiles<'a>,
    started: Instant,
    index: PathIndex,
}

impl Builder<'_> {
    fn stopped(&mut self) -> bool {
        if self.cancel.load(Ordering::Relaxed)
            || self.index.entries.len() >= MAX_INDEX_ENTRIES
            || self.started.elapsed() > INDEX_DEADLINE
        {
            self.index.incomplete = true;
        }
        self.index.incomplete
    }

    fn push(&mut self, relative: PathBuf) {
        if relative
            .components()
            .any(|c| is_excluded_dir(c.as_os_str()))
        {
            return;
        }
        let key: Box<str> = relative.to_string_lossy().to_lowercase().into();
        let name_start = key.rfind('/').map_or(0, |i| i + 1);
        self.index.entries.push(IndexEntry {
            relative,
            key,
            name_start,
        });
    }

    /// Returns false when `relative` is not inside a Git worktree (or Git failed).
    fn list_git(&mut self, relative: &Path) -> bool {
        let Ok(bytes) = (self.list_files)(&self.root.join(relative), self.cancel) else {
            return false;
        };
        for (path, kind) in parse_ls_files(&bytes) {
            if self.stopped() {
                return true;
            }
            let path = relative.join(path);
            match kind {
                ListedKind::File => self.push(path),
                ListedKind::Submodule | ListedKind::NestedRepository => {
                    // Uninitialized submodules are empty directories; symlinks are not followed.
                    match fs::symlink_metadata(self.root.join(&path)) {
                        Ok(meta) if meta.is_dir() => {
                            if !self.list_git(&path) {
                                self.walk(&path);
                            }
                        }
                        Ok(_) => {}
                        Err(_) => self.index.errors += 1,
                    }
                }
            }
        }
        true
    }

    fn walk(&mut self, relative: &Path) {
        let mut pending = vec![relative.to_path_buf()];
        while let Some(dir) = pending.pop() {
            if self.stopped() {
                return;
            }
            let entries = match fs::read_dir(self.root.join(&dir)) {
                Ok(entries) => entries,
                Err(_) => {
                    self.index.errors += 1;
                    continue;
                }
            };
            for item in entries {
                let Ok(item) = item else {
                    self.index.errors += 1;
                    continue;
                };
                let Ok(kind) = item.file_type() else {
                    self.index.errors += 1;
                    continue;
                };
                let path = dir.join(item.file_name());
                if kind.is_symlink() {
                    continue;
                } else if kind.is_dir() {
                    if is_excluded_dir(&item.file_name()) {
                        continue;
                    }
                    // A nested repository contributes its own Git listing and ignore rules.
                    if !(self.root.join(&path).join(".git").exists() && self.list_git(&path)) {
                        pending.push(path);
                    }
                } else {
                    self.push(path);
                }
            }
        }
    }
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
        // SAFETY: `file` is a live directory descriptor and `name` is a NUL-terminated CString
        // that outlives the call; openat does not retain either pointer.
        let fd = unsafe { libc::openat(file.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new descriptor that nothing else owns; File closes it on all
        // exit paths.
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
        let not_git = |_: &Path, _: &AtomicBool| Err(io::Error::other("not a worktree"));
        let index = PathIndex::build(&root, &AtomicBool::new(false), &not_git);
        let result = index.search("MAIN.RS");
        assert_eq!(result.paths, vec![path.clone()]);
        assert!(!result.incomplete);
        assert!(PathIndex::build(&root, &AtomicBool::new(true), &not_git).incomplete);
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
        // SAFETY: `fifo` is a valid NUL-terminated path that outlives the call.
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

    #[test]
    fn git_index_follows_ignore_rules_and_nested_repositories() {
        let root = std::env::temp_dir().join(format!("zj-index-{}", std::process::id()));
        let git = |dir: &Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
                .arg(dir)
                .args(args)
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE")
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "git {args:?}");
        };
        fs::create_dir_all(root.join("outer/src")).unwrap();
        fs::create_dir_all(root.join("outer/build")).unwrap();
        fs::create_dir_all(root.join("outer/inner")).unwrap();
        fs::create_dir_all(root.join("plain/node_modules/pkg")).unwrap();
        fs::write(root.join("outer/.gitignore"), "build/\n").unwrap();
        fs::write(root.join("outer/src/main.rs"), "").unwrap();
        fs::write(root.join("outer/build/out.rs"), "").unwrap();
        fs::write(root.join("outer/untracked.md"), "").unwrap();
        fs::write(root.join("outer/inner/nested.rs"), "").unwrap();
        fs::write(root.join("plain/readme.txt"), "").unwrap();
        fs::write(root.join("plain/node_modules/pkg/index.js"), "").unwrap();
        git(&root.join("outer"), &["init", "-q"]);
        git(&root.join("outer/inner"), &["init", "-q"]);
        git(&root.join("outer"), &["add", ".gitignore", "src/main.rs"]);
        let service = workspace_editor_git::GitService::new(1, Duration::from_secs(10)).unwrap();
        let list = |dir: &Path, cancel: &AtomicBool| service.list_files(dir, cancel);
        let index = PathIndex::build(&root, &AtomicBool::new(false), &list);
        let mut keys: Vec<_> = index.entries.iter().map(|e| e.key.to_string()).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "outer/.gitignore",
                "outer/inner/nested.rs",
                "outer/src/main.rs",
                "outer/untracked.md",
                "plain/readme.txt",
            ]
        );
        assert_eq!(
            index.search("main").paths,
            vec![root.join("outer/src/main.rs")]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
