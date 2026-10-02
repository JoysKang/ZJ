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
use workspace_editor_core::{DocumentId, GitMarker, git_marker, is_excluded_dir};
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

/// Dot files and folders that stay visible while hidden files are hidden: project
/// configuration people edit, as opposed to tool state (`.venv`, `.pytest_cache`, `.claude`).
const VISIBLE_DOT_NAMES: &[&str] = &[
    ".github",
    ".gitlab",
    ".cargo",
    ".gitignore",
    ".gitattributes",
    ".gitmodules",
    ".gitlab-ci.yml",
    ".editorconfig",
    ".dockerignore",
];
const VISIBLE_DOT_PREFIXES: &[&str] = &[".env", ".prettierrc", ".eslintrc"];

/// Whether a file or folder name is hidden unless the user shows hidden files.
pub fn hidden_by_default(name: &std::ffi::OsStr) -> bool {
    let name = name.as_bytes();
    name.first() == Some(&b'.')
        && !VISIBLE_DOT_NAMES.iter().any(|v| name == v.as_bytes())
        && !VISIBLE_DOT_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix.as_bytes()))
}

/// Whether any component of a relative path is hidden by default.
pub fn path_hidden_by_default(relative: &Path) -> bool {
    relative
        .components()
        .any(|c| hidden_by_default(c.as_os_str()))
}

/// Lists a directory for the Explorer: folders first, then files, by name. Hidden dot
/// entries are left out unless `show_hidden`; the second value counts them.
pub fn directory(path: &Path, show_hidden: bool) -> io::Result<(Vec<Entry>, usize)> {
    let mut entries = Vec::new();
    let mut hidden = 0;
    for item in fs::read_dir(path)? {
        let item = item?;
        if item.file_name() == ".git" {
            continue;
        }
        if !show_hidden && hidden_by_default(&item.file_name()) {
            hidden += 1;
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
    Ok((entries, hidden))
}

#[derive(Default)]
pub struct SearchResults {
    pub paths: Vec<PathBuf>,
    pub incomplete: bool,
}

/// Every file path of a workspace, built once in the background and matched in memory.
///
/// Git worktrees contribute `git ls-files` output (tracked plus non-ignored untracked files);
/// other directories use a bounded walk that skips [`EXCLUDED_DIRS`] and directory symlinks.
///
/// Paths are packed into one arena of raw bytes with 12 bytes of offsets per file, instead of
/// two heap allocations per file; only paths that are not already lowercase UTF-8 keep a
/// separate lowercased key for matching. A workspace of 20,000 files takes about 0.8 MB
/// instead of 4.5 MB.
pub struct PathIndex {
    root: PathBuf,
    /// Relative paths' raw bytes, concatenated in sorted order.
    paths: Vec<u8>,
    /// Lowercased lossy display forms, concatenated in the same order.
    keys: String,
    /// End offsets into `paths` and `keys`; each entry starts where the previous one ends.
    slots: Vec<Slot>,
    pub incomplete: bool,
    pub errors: usize,
}

#[derive(Clone, Copy)]
struct Slot {
    path_end: u32,
    key_end: u32,
    /// Byte offset of the file name inside the key.
    name_start: u32,
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
            found: Vec::new(),
            incomplete: false,
            errors: 0,
        };
        // The root may sit inside a worktree without its own `.git`, so always try Git first.
        if !builder.list_git(Path::new("")) {
            builder.walk(Path::new(""));
        }
        let Builder {
            found,
            incomplete,
            errors,
            ..
        } = builder;
        Self::pack(root.to_path_buf(), found, incomplete, errors)
    }

    /// Sorted by relative path (component order, so a directory's files are contiguous) so
    /// file watching can look entries up without a second set.
    fn pack(root: PathBuf, mut found: Vec<PathBuf>, incomplete: bool, errors: usize) -> Self {
        found.sort_unstable();
        found.dedup();
        let bytes: usize = found.iter().map(|p| p.as_os_str().len()).sum();
        let mut index = PathIndex {
            root,
            paths: Vec::with_capacity(bytes),
            keys: String::new(),
            slots: Vec::with_capacity(found.len()),
            incomplete,
            errors,
        };
        for relative in found {
            index.push(relative.as_path());
        }
        index
    }

    fn push(&mut self, relative: &Path) {
        let key_start = self.keys.len();
        let bytes = relative.as_os_str().as_bytes();
        self.paths.extend_from_slice(bytes);
        // Most paths are already lowercase UTF-8; those share their bytes with the key.
        let same = std::str::from_utf8(bytes).is_ok_and(|text| {
            !text
                .chars()
                .any(|c| c.to_lowercase().ne(std::iter::once(c)))
        });
        let key = if same {
            std::str::from_utf8(bytes).unwrap_or_default()
        } else {
            for c in relative.to_string_lossy().chars() {
                self.keys.extend(c.to_lowercase());
            }
            &self.keys[key_start..]
        };
        let name_start = key.rfind('/').map_or(0, |i| i + 1);
        self.slots.push(Slot {
            path_end: self.paths.len() as u32,
            key_end: self.keys.len() as u32,
            name_start: name_start as u32,
        });
    }

    fn relative(&self, index: usize) -> &Path {
        let start = index
            .checked_sub(1)
            .map_or(0, |prev| self.slots[prev].path_end as usize);
        let end = self.slots[index].path_end as usize;
        Path::new(std::ffi::OsStr::from_bytes(&self.paths[start..end]))
    }

    /// The lowercased display form; an empty key range means the path itself is the key.
    fn key(&self, index: usize) -> (&str, usize) {
        let start = index
            .checked_sub(1)
            .map_or(0, |prev| self.slots[prev].key_end as usize);
        let slot = self.slots[index];
        let key = match &self.keys[start..slot.key_end as usize] {
            "" => {
                std::str::from_utf8(self.relative(index).as_os_str().as_bytes()).unwrap_or_default()
            }
            key => key,
        };
        (key, slot.name_start as usize)
    }

    fn relatives(&self) -> impl Iterator<Item = &Path> {
        (0..self.slots.len()).map(|i| self.relative(i))
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Bytes held by the index, for resource reports.
    pub fn heap_bytes(&self) -> usize {
        self.paths.capacity()
            + self.keys.capacity()
            + self.slots.capacity() * std::mem::size_of::<Slot>()
    }

    /// Absolute paths of all indexed files.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.relatives().map(|r| self.root.join(r)).collect()
    }

    fn position(&self, relative: &Path) -> Result<usize, usize> {
        let (mut low, mut high) = (0, self.slots.len());
        while low < high {
            let mid = (low + high) / 2;
            match self.relative(mid).cmp(relative) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Ok(mid),
            }
        }
        Err(low)
    }

    /// Whether `path` (absolute) is an indexed file, or a directory containing indexed files.
    pub fn covers(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        match self.position(relative) {
            Ok(_) => true,
            Err(next) => next < self.slots.len() && self.relative(next).starts_with(relative),
        }
    }

    /// A copy with files added and paths (files or whole directories) removed; used for
    /// watch events instead of rebuilding the index.
    pub fn with_changes(&self, added: &[PathBuf], removed: &[PathBuf]) -> PathIndex {
        let removed: Vec<&Path> = removed
            .iter()
            .filter_map(|path| path.strip_prefix(&self.root).ok())
            .collect();
        let mut found: Vec<PathBuf> = self
            .relatives()
            .filter(|relative| !removed.iter().any(|gone| relative.starts_with(gone)))
            .map(Path::to_path_buf)
            .collect();
        for path in added {
            if let Ok(relative) = path.strip_prefix(&self.root)
                && !relative
                    .components()
                    .any(|c| is_excluded_dir(c.as_os_str()))
            {
                found.push(relative.to_path_buf());
            }
        }
        Self::pack(self.root.clone(), found, self.incomplete, self.errors)
    }

    /// Ranks all entries against `query` and returns the best [`MAX_RESULTS`] as absolute paths.
    /// Paths through hidden dot folders are skipped unless `show_hidden`.
    pub fn search(&self, query: &str, show_hidden: bool) -> SearchResults {
        let query = fuzzy::query_chars(query);
        let mut result = SearchResults {
            incomplete: self.incomplete,
            ..Default::default()
        };
        if query.is_empty() {
            return result;
        }
        let mut scored: Vec<(i32, usize)> = (0..self.slots.len())
            .filter_map(|i| {
                let (key, name_start) = self.key(i);
                fuzzy::score(&query, key, name_start)
                    .filter(|_| show_hidden || !path_hidden_by_default(self.relative(i)))
                    .map(|s| (s, i))
            })
            .collect();
        let order = |a: &(i32, usize), b: &(i32, usize)| {
            let (ka, kb) = (self.key(a.1).0, self.key(b.1).0);
            b.0.cmp(&a.0).then(ka.len().cmp(&kb.len())).then(ka.cmp(kb))
        };
        if scored.len() > MAX_RESULTS {
            scored.select_nth_unstable_by(MAX_RESULTS, order);
            scored.truncate(MAX_RESULTS);
            result.incomplete = true;
        }
        scored.sort_unstable_by(order);
        result.paths = scored
            .into_iter()
            .map(|(_, i)| self.root.join(self.relative(i)))
            .collect();
        result
    }
}

struct Builder<'a> {
    root: &'a Path,
    cancel: &'a AtomicBool,
    list_files: &'a ListFiles<'a>,
    started: Instant,
    found: Vec<PathBuf>,
    incomplete: bool,
    errors: usize,
}

impl Builder<'_> {
    fn stopped(&mut self) -> bool {
        if self.cancel.load(Ordering::Relaxed)
            || self.found.len() >= MAX_INDEX_ENTRIES
            || self.started.elapsed() > INDEX_DEADLINE
        {
            self.incomplete = true;
        }
        self.incomplete
    }

    fn push(&mut self, relative: PathBuf) {
        if relative
            .components()
            .any(|c| is_excluded_dir(c.as_os_str()))
        {
            return;
        }
        self.found.push(relative);
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
                        Err(_) => self.errors += 1,
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
                    self.errors += 1;
                    continue;
                }
            };
            for item in entries {
                let Ok(item) = item else {
                    self.errors += 1;
                    continue;
                };
                let Ok(kind) = item.file_type() else {
                    self.errors += 1;
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
                    if !(git_marker(&self.root.join(&path)) == GitMarker::Valid
                        && self.list_git(&path))
                    {
                        pending.push(path);
                    }
                } else {
                    self.push(path);
                }
            }
        }
    }
}

/// What a file looked like when it was read: a later write checks it is still the same
/// file, unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub device: u64,
    pub inode: u64,
    pub len: u64,
    pub modified_ns: i128,
}

impl FileStamp {
    pub fn of(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            modified_ns: metadata.mtime() as i128 * 1_000_000_000 + metadata.mtime_nsec() as i128,
        }
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        Ok(Self::of(&fs::metadata(path)?))
    }
}

pub struct TextFile {
    pub id: DocumentId,
    pub path: PathBuf,
    pub text: String,
    pub bytes: usize,
    pub readonly: bool,
    /// The file uses CRLF line endings (first line ending decides).
    pub crlf: bool,
    /// A UTF-8 byte order mark was present and stripped for editing.
    pub bom: bool,
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
        crlf: text
            .find('\n')
            .is_some_and(|newline| text[..newline].ends_with('\r')),
        bom: text.starts_with('\u{feff}'),
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
        let (entries, _) = directory(&root, true).unwrap();
        assert!(entries[0].directory);
        assert!(
            !entries
                .iter()
                .any(|entry| entry.path.file_name().unwrap() == ".git")
        );
        let not_git = |_: &Path, _: &AtomicBool| Err(io::Error::other("not a worktree"));
        let index = PathIndex::build(&root, &AtomicBool::new(false), &not_git);
        let result = index.search("MAIN.RS", true);
        assert_eq!(result.paths, vec![path.clone()]);
        assert!(!result.incomplete);
        assert!(PathIndex::build(&root, &AtomicBool::new(true), &not_git).incomplete);
        // Incremental updates from file watching.
        assert!(index.covers(&path) && index.covers(&root.join("src")));
        assert!(!index.covers(&root.join("target/main.rs")));
        let added = root.join("src/added.rs");
        let next = index.with_changes(&[added.clone(), root.join("target/x.rs")], &[]);
        assert!(next.covers(&added) && !next.covers(&root.join("target/x.rs")));
        let gone = next.with_changes(&[], &[root.join("src")]);
        assert!(!gone.covers(&path) && !gone.covers(&added));
        let loaded = text_file(Some(&root), &path).unwrap();
        assert_eq!(loaded.text, "hello\r\n");
        assert!(loaded.crlf && loaded.bom);
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
    fn dot_entries_hide_except_project_configuration() {
        use std::ffi::OsStr;
        for hidden in [
            ".venv",
            ".claude",
            ".pytest_cache",
            ".vscode",
            ".DS_Store",
            ".ruff_cache",
        ] {
            assert!(hidden_by_default(OsStr::new(hidden)), "{hidden}");
        }
        for shown in [
            ".github",
            ".gitignore",
            ".env",
            ".env.local",
            ".eslintrc.json",
            ".cargo",
            "src",
        ] {
            assert!(!hidden_by_default(OsStr::new(shown)), "{shown}");
        }
        assert!(path_hidden_by_default(Path::new("a/.claude/settings.json")));
        assert!(!path_hidden_by_default(Path::new(
            ".github/workflows/ci.yml"
        )));
        let root = std::env::temp_dir().join(format!("zj-hidden-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [".venv", ".github", "src"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(root.join(".gitignore"), "").unwrap();
        fs::write(root.join(".DS_Store"), "").unwrap();
        let names = |show| {
            let (entries, hidden) = directory(&root, show).unwrap();
            let names: Vec<String> = entries
                .iter()
                .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            (names, hidden)
        };
        assert_eq!(
            names(false),
            (vec![".github".into(), "src".into(), ".gitignore".into()], 2)
        );
        assert_eq!(names(true).0.len(), 5);
        let index = PathIndex::pack(
            root.clone(),
            vec![".venv/lib/site.py".into(), "src/site.py".into()],
            false,
            0,
        );
        assert_eq!(
            index.search("site", false).paths,
            vec![root.join("src/site.py")]
        );
        assert_eq!(index.search("site", true).paths.len(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn packed_index_matches_mixed_case_and_stays_small() {
        let paths: Vec<PathBuf> = (0..1000)
            .map(|i| PathBuf::from(format!("src/m{:02}/file_{i:04}.rs", i / 40)))
            .chain(["Docs/README.md".into(), "a-b/x.rs".into(), "a/b.rs".into()])
            .collect();
        let index = PathIndex::pack(PathBuf::from("/w"), paths, false, 0);
        assert_eq!(index.len(), 1003);
        // Lowercase paths share their bytes with the key: ~23 bytes + 12 per entry.
        assert!(index.heap_bytes() < 1003 * 40, "{}", index.heap_bytes());
        assert_eq!(
            index.search("readme", true).paths,
            vec![PathBuf::from("/w/Docs/README.md")]
        );
        assert_eq!(index.key(0).0, "Docs/README.md".to_lowercase());
        assert!(index.covers(Path::new("/w/a")) && index.covers(Path::new("/w/a-b/x.rs")));
        assert!(index.covers(Path::new("/w/src/m03")) && !index.covers(Path::new("/w/src/m99")));
        let next = index.with_changes(&[PathBuf::from("/w/a/c.rs")], &[PathBuf::from("/w/src")]);
        assert_eq!(next.len(), 4);
        assert!(next.covers(Path::new("/w/a/c.rs")) && !next.covers(Path::new("/w/src")));
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
        let mut keys: Vec<_> = (0..index.len())
            .map(|i| index.key(i).0.to_string())
            .collect();
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
            index.search("main", true).paths,
            vec![root.join("outer/src/main.rs")]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
