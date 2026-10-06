//! Saving documents, as VS Code does it but small:
//! - **Line endings:** written as the document says (LF / CRLF, switchable in the status
//!   bar); new lines typed into a CRLF document get CRLF too.
//! - **Encoding:** a UTF-8 byte order mark is kept if the file had one; the final newline is
//!   left as it is.
//! - **Atomic writes:** a temporary file in the same folder, fsync, then rename, keeping the
//!   file's mode.
//! - **Symlinks** are written through to their target. **Hard-linked files** are written in
//!   place, so the other links keep seeing the same file.
//! - **Conflicts:** a save refuses to overwrite a file that changed on disk since it was
//!   loaded or last saved (device, inode, size, mtime, then content hash), unless told to.
//!
//! Also here: the small state machines the workbench drives for 关闭 / 退出 with unsaved
//! changes and for auto-save, so they can be tested without a window.

use crate::files::FileStamp;
use std::{
    collections::VecDeque,
    fs,
    hash::{Hash, Hasher},
    io::{self, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// What a document's file looked like when it was loaded or last saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskState {
    pub stamp: FileStamp,
    pub hash: u64,
}

impl DiskState {
    pub fn of(stamp: FileStamp, bytes: &[u8]) -> Self {
        Self {
            stamp,
            hash: content_hash(bytes),
        }
    }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// How the file on disk compares with what the document was loaded from.
#[derive(Debug, PartialEq, Eq)]
pub enum OnDisk {
    Same,
    /// Touched, but the content is the same (an editor saved without changes, a checkout).
    Touched(DiskState),
    Changed {
        state: DiskState,
        bytes: Vec<u8>,
    },
    /// Changed, and now larger than the editor opens: not read.
    TooLarge,
    Deleted,
}

/// Compares the file at `path` with `known`, reading it only when its stamp moved.
pub fn check(path: &Path, known: &DiskState) -> io::Result<OnDisk> {
    let stamp = match FileStamp::read(path) {
        Ok(stamp) => stamp,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(OnDisk::Deleted),
        Err(error) => return Err(error),
    };
    if stamp == known.stamp {
        return Ok(OnDisk::Same);
    }
    if stamp.len > crate::files::MAX_FILE_BYTES as u64 {
        return Ok(OnDisk::TooLarge);
    }
    let bytes = fs::read(path)?;
    let state = DiskState::of(stamp, &bytes);
    if state.hash == known.hash {
        Ok(OnDisk::Touched(state))
    } else {
        Ok(OnDisk::Changed { state, bytes })
    }
}

/// The bytes to write for a buffer: CRLF documents get `\r\n` for every line break (lines
/// typed in the editor come in as `\n`), LF documents are written as they are, and the byte
/// order mark comes back if the file had one.
pub fn encode(text: &str, crlf: bool, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3 + if crlf { text.len() / 32 } else { 0 });
    if bom {
        out.extend_from_slice("\u{feff}".as_bytes());
    }
    if crlf {
        let mut previous = 0u8;
        for &byte in text.as_bytes() {
            if byte == b'\n' && previous != b'\r' {
                out.push(b'\r');
            }
            out.push(byte);
            previous = byte;
        }
    } else {
        out.extend_from_slice(text.as_bytes());
    }
    out
}

/// The buffer text with every line break turned into `\r\n` (`crlf`) or `\n`, for the
/// status bar's LF / CRLF switch.
pub fn convert_line_endings(text: &str, crlf: bool) -> String {
    let lf = text.replace("\r\n", "\n");
    if crlf { lf.replace('\n', "\r\n") } else { lf }
}

#[derive(Debug)]
pub enum SaveError {
    /// No permission to write the file (or its folder): offer 另存为.
    ReadOnly(String),
    /// The file changed on disk since it was loaded or saved.
    Conflict,
    Io(io::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::ReadOnly(why) => write!(f, "{why}"),
            SaveError::Conflict => write!(f, "文件已在磁盘上更改"),
            SaveError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::PermissionDenied {
            SaveError::ReadOnly(format!("没有写入权限：{error}"))
        } else {
            SaveError::Io(error)
        }
    }
}

/// Writes `bytes` to `path`. With `expected`, refuses when the file changed on disk since
/// (`SaveError::Conflict`); a file that was deleted meanwhile is simply created again.
pub fn write(
    path: &Path,
    bytes: &[u8],
    expected: Option<&DiskState>,
) -> Result<DiskState, SaveError> {
    // Symlinks are written through: the link stays, its target gets the new contents.
    let target = match fs::canonicalize(path) {
        Ok(target) => target,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A dangling link writes its target; a missing file is created.
            match fs::read_link(path) {
                Ok(link) => path.parent().unwrap_or(Path::new("/")).join(link),
                Err(_) => path.to_path_buf(),
            }
        }
        Err(error) => return Err(error.into()),
    };
    let metadata = match fs::metadata(&target) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if let (Some(expected), Some(_)) = (expected, &metadata) {
        match check(&target, expected)? {
            OnDisk::Same | OnDisk::Touched(_) | OnDisk::Deleted => {}
            OnDisk::Changed { .. } | OnDisk::TooLarge => return Err(SaveError::Conflict),
        }
    }
    if let Some(metadata) = &metadata
        && metadata.permissions().mode() & 0o222 == 0
    {
        return Err(SaveError::ReadOnly("文件是只读的".into()));
    }
    if metadata.is_none()
        && let Some(parent) = target.parent()
    {
        // A file deleted together with its folder is recreated with it (as VS Code does).
        fs::create_dir_all(parent)?;
    }
    match &metadata {
        // Several names for one file: rewrite it in place so they all stay the same file.
        Some(metadata) if metadata.nlink() > 1 => {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&target)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        _ => write_atomically(&target, bytes, metadata.as_ref())?,
    }
    let stamp = FileStamp::read(&target)?;
    Ok(DiskState::of(stamp, bytes))
}

fn write_atomically(
    target: &Path,
    bytes: &[u8],
    metadata: Option<&fs::Metadata>,
) -> io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temporary = dir.join(format!(".{name}.zj-save-{}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        if let Some(metadata) = metadata {
            file.set_permissions(fs::Permissions::from_mode(metadata.permissions().mode()))?;
        }
        file.sync_all()?;
        fs::rename(&temporary, target)?;
        // The rename itself is durable once the folder is synced.
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The answer to 「要保存对 X 的更改吗？」.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Save,
    DontSave,
    Cancel,
}

impl Answer {
    /// From the prompt's button index (保存 / 不保存 / 取消, or 全部保存 / 全部不保存 / 取消).
    pub fn from_button(index: Option<usize>) -> Self {
        match index {
            Some(0) => Answer::Save,
            Some(1) => Answer::DontSave,
            _ => Answer::Cancel,
        }
    }
}

/// Quitting with unsaved changes: the windows that have some are asked one after another;
/// a 取消 (or a failed save) stops the quit, and once all are answered the app quits.
#[derive(Debug)]
pub struct QuitFlow<T> {
    pending: VecDeque<T>,
    pub done: bool,
    pub cancelled: bool,
}

impl<T> QuitFlow<T> {
    pub fn new(windows: impl IntoIterator<Item = T>) -> Self {
        let pending: VecDeque<T> = windows.into_iter().collect();
        Self {
            done: pending.is_empty(),
            pending,
            cancelled: false,
        }
    }

    /// The next window to ask, if the quit is still going.
    pub fn next(&mut self) -> Option<T> {
        if self.cancelled || self.done {
            return None;
        }
        let next = self.pending.pop_front();
        if next.is_none() {
            self.done = true;
        }
        next
    }

    /// How the window that was asked ended: `resolved` when its changes were saved or
    /// discarded, `false` when the user cancelled or a save failed.
    pub fn answered(&mut self, resolved: bool) {
        if !resolved {
            self.cancelled = true;
        } else if self.pending.is_empty() {
            self.done = true;
        }
    }

    /// Whether the app should quit now.
    pub fn should_quit(&self) -> bool {
        self.done && !self.cancelled
    }
}

/// files.autoSave: off, afterDelay (1000 ms), onFocusChange.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoSave {
    #[default]
    Off,
    AfterDelay,
    OnFocusChange,
}

pub const AUTO_SAVE_DELAY: Duration = Duration::from_millis(1000);

impl AutoSave {
    pub fn parse(value: &str) -> Self {
        match value {
            "afterDelay" => AutoSave::AfterDelay,
            "onFocusChange" => AutoSave::OnFocusChange,
            _ => AutoSave::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AutoSave::Off => "off",
            AutoSave::AfterDelay => "afterDelay",
            AutoSave::OnFocusChange => "onFocusChange",
        }
    }
}

/// Debounces auto-save: each edit restarts the delay; a timer that fires for an older edit
/// does nothing.
#[derive(Debug, Default)]
pub struct Debounce {
    generation: u64,
}

impl Debounce {
    /// An edit happened: returns the ticket the timer started for it must present.
    pub fn poke(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// The timer for `ticket` fired: true only if no edit came after it.
    pub fn fire(&self, ticket: u64) -> bool {
        ticket == self.generation
    }
}

/// A full-context unified patch from `old` to `new`, for 比较 (the file on disk ↔ the buffer)
/// in the diff editor. Lines are matched with an LCS between the common prefix and suffix;
/// a middle part too large for that is shown as replaced.
pub fn text_patch(old: &str, new: &str) -> String {
    const MAX_CELLS: usize = 4_000_000;
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    // The middle as (' ' | '-' | '+', line).
    let mut middle: Vec<(char, &str)> = Vec::new();
    if am.len().saturating_mul(bm.len()) <= MAX_CELLS {
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                middle.push((' ', am[i]));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                // Deletions before insertions, as git prints them.
                middle.push(('-', am[i]));
                i += 1;
            } else {
                middle.push(('+', bm[j]));
                j += 1;
            }
        }
    } else {
        middle.extend(am.iter().map(|line| ('-', *line)));
        middle.extend(bm.iter().map(|line| ('+', *line)));
    }
    let lines = a[..prefix]
        .iter()
        .map(|line| (' ', *line))
        .chain(middle)
        .chain(a[a.len() - suffix..].iter().map(|line| (' ', *line)));
    let (mut old_count, mut new_count) = (0, 0);
    let mut body = String::new();
    for (kind, line) in lines {
        match kind {
            ' ' => {
                old_count += 1;
                new_count += 1;
            }
            '-' => old_count += 1,
            _ => new_count += 1,
        }
        body.push(kind);
        body.push_str(line);
        if !line.ends_with('\n') {
            body.push_str("\n\\ No newline at end of file\n");
        }
    }
    format!("@@ -1,{old_count} +1,{new_count} @@\n{body}")
}

/// Name of the next untitled buffer (Untitled-1, Untitled-2, …) given the ones in use.
pub fn untitled_name(in_use: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    let used: std::collections::HashSet<PathBuf> = in_use.into_iter().collect();
    (1..)
        .map(|n| PathBuf::from(format!("Untitled-{n}")))
        .find(|name| !used.contains(name))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zj-save-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn state(path: &Path) -> DiskState {
        DiskState::of(FileStamp::read(path).unwrap(), &fs::read(path).unwrap())
    }

    #[test]
    fn a_file_grown_past_the_limit_is_not_read_and_saving_over_it_is_a_conflict() {
        let root = temp("too-large");
        let path = root.join("log.txt");
        fs::write(&path, "small\n").unwrap();
        let known = state(&path);
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(crate::files::MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        assert!(matches!(check(&path, &known).unwrap(), OnDisk::TooLarge));
        assert!(matches!(
            write(&path, b"mine\n", Some(&known)),
            Err(SaveError::Conflict)
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn atomic_write_keeps_mode_and_replaces_the_file() {
        let root = temp("atomic");
        let path = root.join("a.sh");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        let before = state(&path);
        let after = write(&path, b"new\n", Some(&before)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
        // A new inode: the old one was replaced, not truncated.
        assert_ne!(after.stamp.inode, before.stamp.inode);
        assert_eq!(after, state(&path));
        // No temporary file left behind.
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn line_endings_and_byte_order_mark() {
        assert_eq!(encode("a\r\nb\nc", true, false), b"a\r\nb\r\nc");
        assert_eq!(encode("a\r\nb\n", false, false), b"a\r\nb\n");
        assert_eq!(encode("x", false, true), "\u{feff}x".as_bytes());
        assert_eq!(encode("", true, false), b"");
        // The final newline is kept as it is, present or not.
        assert_eq!(encode("a\n", true, false), b"a\r\n");
        assert_eq!(encode("a", true, false), b"a");
        assert_eq!(convert_line_endings("a\r\nb\nc", true), "a\r\nb\r\nc");
        assert_eq!(convert_line_endings("a\r\nb\nc", false), "a\nb\nc");
        // Round trip through a real file.
        let root = temp("crlf");
        let path = root.join("w.txt");
        fs::write(&path, "\u{feff}一\r\n二\r\n").unwrap();
        let loaded = crate::files::text_file(Some(&root), &path).unwrap();
        assert!(loaded.crlf && loaded.bom);
        let edited = format!("{}三\n", loaded.text);
        write(&path, &encode(&edited, loaded.crlf, loaded.bom), None).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            "\u{feff}一\r\n二\r\n三\r\n".as_bytes()
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlinks_are_written_through_and_hard_links_in_place() {
        let root = temp("links");
        let target = root.join("target.txt");
        let link = root.join("link.txt");
        fs::write(&target, "t").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write(&link, b"through", Some(&state(&link))).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "through");
        let other = root.join("hard.txt");
        fs::hard_link(&target, &other).unwrap();
        let inode = fs::metadata(&target).unwrap().ino();
        write(&other, b"both", Some(&state(&other))).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "both");
        assert_eq!(fs::metadata(&other).unwrap().ino(), inode);
        assert_eq!(fs::metadata(&other).unwrap().nlink(), 2);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_only_files_are_refused() {
        let root = temp("readonly");
        let path = root.join("r.txt");
        fs::write(&path, "r").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(
            write(&path, b"x", None),
            Err(SaveError::ReadOnly(_))
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "r");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn external_changes_are_detected() {
        let root = temp("external");
        let path = root.join("e.txt");
        fs::write(&path, "one").unwrap();
        let known = state(&path);
        assert_eq!(check(&path, &known).unwrap(), OnDisk::Same);
        // Rewritten with the same content: only touched.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&path, "one").unwrap();
        assert!(matches!(check(&path, &known).unwrap(), OnDisk::Touched(_)));
        // Really changed: a save refuses, unless it overwrites (no expected state).
        fs::write(&path, "other").unwrap();
        assert!(
            matches!(check(&path, &known).unwrap(), OnDisk::Changed { ref bytes, .. } if bytes == b"other")
        );
        assert!(matches!(
            write(&path, b"mine", Some(&known)),
            Err(SaveError::Conflict)
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "other");
        write(&path, b"mine", None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        // Deleted meanwhile: saving creates it again.
        let known = state(&path);
        fs::remove_file(&path).unwrap();
        assert_eq!(check(&path, &known).unwrap(), OnDisk::Deleted);
        write(&path, b"back", Some(&known)).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "back");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn compare_patches_align_lines() {
        assert_eq!(
            text_patch("a\nb\nc\n", "a\nB\nc\nd\n"),
            "@@ -1,3 +1,4 @@\n a\n-b\n+B\n c\n+d\n"
        );
        assert_eq!(
            text_patch("x", "x"),
            "@@ -1,1 +1,1 @@\n x\n\\ No newline at end of file\n"
        );
        // The patch rebuilds both sides in the diff editor.
        let patch = text_patch("一\n二\n三\n", "一\n三\n四\n");
        let doc = crate::diff_doc::DiffDoc::parse(
            &patch,
            None,
            &gpui_kit::component::highlighter::HighlightTheme::default_dark(),
            crate::diff_doc::ChangeColors {
                inserted_text: gpui_kit::Hsla::default(),
                removed_text: gpui_kit::Hsla::default(),
            },
        )
        .unwrap();
        assert_eq!(doc.old.text, "一\n二\n三\n");
        assert_eq!(doc.new.text, "一\n三\n四\n");
    }

    #[test]
    fn untitled_buffers_are_saved_as_a_new_file() {
        assert_eq!(untitled_name(vec![]), PathBuf::from("Untitled-1"));
        assert_eq!(
            untitled_name(vec![
                PathBuf::from("Untitled-1"),
                PathBuf::from("Untitled-3")
            ]),
            PathBuf::from("Untitled-2")
        );
        let root = temp("untitled");
        let path = root.join("新文件.md");
        let saved = write(&path, &encode("# 标题\n", false, false), None).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "# 标题\n");
        assert_eq!(saved, state(&path));
        // A buffer whose file was deleted with its folder recreates both.
        let gone = root.join("已删除/a.txt");
        write(&gone, b"kept\n", None).unwrap();
        assert_eq!(fs::read(&gone).unwrap(), b"kept\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn auto_save_debounces_edits() {
        assert_eq!(AutoSave::parse("afterDelay"), AutoSave::AfterDelay);
        assert_eq!(AutoSave::parse("nonsense"), AutoSave::Off);
        assert_eq!(
            AutoSave::parse(AutoSave::OnFocusChange.as_str()),
            AutoSave::OnFocusChange
        );
        let mut debounce = Debounce::default();
        let first = debounce.poke();
        let second = debounce.poke();
        // The first timer fires after the second edit: nothing happens.
        assert!(!debounce.fire(first));
        assert!(debounce.fire(second));
    }

    #[test]
    fn quitting_asks_each_window_and_stops_on_cancel() {
        assert!(QuitFlow::<u8>::new([]).should_quit());
        let mut flow = QuitFlow::new([1, 2, 3]);
        assert_eq!(flow.next(), Some(1));
        flow.answered(true);
        assert_eq!(flow.next(), Some(2));
        flow.answered(false);
        assert_eq!(flow.next(), None);
        assert!(!flow.should_quit());
        let mut flow = QuitFlow::new(["a", "b"]);
        for _ in 0..2 {
            flow.next().unwrap();
            flow.answered(true);
        }
        assert_eq!(flow.next(), None);
        assert!(flow.should_quit());
        assert_eq!(Answer::from_button(Some(0)), Answer::Save);
        assert_eq!(Answer::from_button(Some(1)), Answer::DontSave);
        assert_eq!(Answer::from_button(None), Answer::Cancel);
    }
}
