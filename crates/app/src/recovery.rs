//! Edit recovery (开发说明 R11 / A16): a buffer with unsaved edits is snapshotted to
//! `recovery/` next to `settings.json`, 2 s after the last edit, and the snapshot goes away
//! once the buffer is saved, reloaded or discarded. A launch that finds snapshots left by an
//! abnormal exit restores them into their tabs. Recovery goes back to the last snapshot that
//! reached the disk; the last keystrokes before a crash may be missing.
//!
//! One JSON file per buffer, versioned and written through a temporary file and a rename, so a
//! crash mid-write leaves the previous snapshot. No GPUI here: the workbench feeds `Op`s to a
//! single background queue (`apply`), which keeps writes and removals in order.

use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

/// Bumped when the record format changes; older ZJ versions skip newer files.
pub const VERSION: u64 = 1;
/// How long after the last edit a snapshot is written (开发说明: 2 s).
pub const SNAPSHOT_DELAY: Duration = Duration::from_secs(2);
/// Buffers larger than this are not snapshotted (a file over 8 MB does not open anyway).
pub const MAX_TEXT: usize = 8 * 1024 * 1024;

/// The snapshot of one buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// Names the snapshot file; stays the same for the buffer's lifetime (and across a restore).
    pub key: String,
    /// The file, or the untitled buffer's name (`Untitled-1`).
    pub path: PathBuf,
    pub untitled: bool,
    /// The folder of the window the buffer was open in.
    pub root: Option<PathBuf>,
    pub text: String,
    /// Milliseconds since the Unix epoch.
    pub written_at: i64,
}

/// What the background queue does, in order.
#[derive(Debug)]
pub enum Op {
    Write(Record),
    Remove(String),
}

/// `recovery/` next to the settings file (a temporary directory in tests).
pub fn dir() -> Option<PathBuf> {
    crate::settings::Settings::path().map(|path| path.with_file_name("recovery"))
}

/// The snapshot key of a file.
pub fn file_key(path: &Path) -> String {
    format!("file:{}", path.display())
}

/// A key for an untitled buffer, unique across launches.
pub fn untitled_key(inode: u64) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("untitled:{}:{nanos}:{inode}", std::process::id())
}

/// The snapshot file for `key` (FNV-1a of the key: stable, and safe as a file name).
pub fn file_name(key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}.json")
}

pub fn encode(record: &Record) -> Vec<u8> {
    let value = json!({
        "version": VERSION,
        "key": record.key,
        "path": record.path.to_string_lossy(),
        "untitled": record.untitled,
        "root": record.root.as_ref().map(|root| root.to_string_lossy()),
        "text": record.text,
        "written_at": record.written_at,
    });
    serde_json::to_vec(&value).unwrap_or_default()
}

pub fn decode(bytes: &[u8]) -> Result<Record, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version != VERSION {
        return Err(format!("不支持的版本 {version}"));
    }
    let text = |name: &str| {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("缺少 {name}"))
    };
    Ok(Record {
        key: text("key")?,
        path: PathBuf::from(text("path")?),
        untitled: value
            .get("untitled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        root: value.get("root").and_then(Value::as_str).map(PathBuf::from),
        text: text("text")?,
        written_at: value.get("written_at").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// Runs one queued operation.
pub fn apply(dir: &Path, op: &Op) -> io::Result<()> {
    match op {
        Op::Write(record) => write(dir, record),
        Op::Remove(key) => match fs::remove_file(dir.join(file_name(key))) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

fn write(dir: &Path, record: &Record) -> io::Result<()> {
    if record.text.len() > MAX_TEXT {
        return Err(io::Error::other(format!(
            "{} 超过 {} MB，不写恢复记录",
            record.path.display(),
            MAX_TEXT / 1024 / 1024
        )));
    }
    create_private_dir(dir)?;
    let target = dir.join(file_name(&record.key));
    let temporary = target.with_extension("json.tmp");
    let result = (|| {
        let mut file = private_file(&temporary)?;
        file.write_all(&encode(record))?;
        file.sync_all()?;
        fs::rename(&temporary, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Snapshots hold unsaved text: only the user may read them.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

fn private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Every snapshot in `dir`, oldest first, and the files that could not be read (reported,
/// left in place).
pub fn load_all(dir: &Path) -> (Vec<Record>, Vec<String>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return (records, errors);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| decode(&bytes))
        {
            Ok(record) => records.push(record),
            Err(error) => errors.push(format!("{}：{error}", path.display())),
        }
    }
    records.sort_by_key(|r| r.written_at);
    (records, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, text: &str) -> Record {
        Record {
            key: key.into(),
            path: "/w/src/main.rs".into(),
            untitled: false,
            root: Some("/w".into()),
            text: text.into(),
            written_at: 1,
        }
    }

    #[test]
    fn records_round_trip_and_reject_other_versions() {
        let r = record("file:/w/src/main.rs", "fn main() {}\n中文");
        assert_eq!(decode(&encode(&r)).unwrap(), r);
        let newer = String::from_utf8(encode(&r))
            .unwrap()
            .replace("\"version\":1", "\"version\":2");
        assert!(decode(newer.as_bytes()).is_err());
        assert!(decode(b"{ not json").is_err());
    }

    #[test]
    fn writes_replace_removes_clean_up_and_bad_files_are_reported() {
        let dir = std::env::temp_dir().join(format!("zj-recovery-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let key = file_key(Path::new("/w/src/main.rs"));
        apply(&dir, &Op::Write(record(&key, "one"))).unwrap();
        apply(&dir, &Op::Write(record(&key, "two"))).unwrap();
        fs::write(dir.join("broken.json"), "{").unwrap();
        let (records, errors) = load_all(&dir);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].text, "two");
        assert_eq!(errors.len(), 1);
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.join(file_name(&key)))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        apply(&dir, &Op::Remove(key.clone())).unwrap();
        // Removing again (already gone) is fine.
        apply(&dir, &Op::Remove(key)).unwrap();
        assert!(load_all(&dir).0.is_empty());
        // Too large: refused, nothing written.
        let big = record("file:/big", &"x".repeat(MAX_TEXT + 1));
        assert!(apply(&dir, &Op::Write(big)).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn untitled_keys_differ_and_file_names_are_stable() {
        assert_ne!(untitled_key(1), untitled_key(1));
        assert_eq!(file_name("file:/a"), file_name("file:/a"));
        assert_ne!(file_name("file:/a"), file_name("file:/b"));
    }
}
