use super::*;
use std::fs::{File, FileTimes};
use std::sync::atomic::AtomicUsize;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zj-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn file(&self) -> PathBuf {
        self.0.join("code.rs")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn review_progress_rechecks_bytes_even_with_identical_mtime_and_size() {
    let dir = Fixture::new();
    let path = dir.file();
    fs::write(&path, b"before").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &path, None).unwrap();
    assert_eq!(record.verify(), ReviewStatus::Reviewed);
    fs::write(&path, b"after!").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(record.verify(), ReviewStatus::Pending);
    // Disk state represents the version shown in the editor, never the unseen current bytes.
    let (_, displayed) = read_fingerprint(&path).unwrap();
    fs::write(&path, b"third!").unwrap();
    assert!(ReviewCheckpoint::capture(&dir.0, &path, Some(displayed)).is_err());
}

#[test]
fn review_progress_is_bound_to_file_and_repository_identity() {
    let dir = Fixture::new();
    let path = dir.file();
    fs::write(&path, "same").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &path, None).unwrap();
    fs::write(dir.0.join("replacement"), "same").unwrap();
    fs::rename(dir.0.join("replacement"), &path).unwrap();
    assert_eq!(record.verify(), ReviewStatus::Pending);
    fs::remove_file(&path).unwrap();
    assert!(matches!(record.verify(), ReviewStatus::Unavailable(_)));
    let other = Fixture::new();
    fs::write(other.file(), "outside").unwrap();
    std::os::unix::fs::symlink(other.file(), &path).unwrap();
    assert!(ReviewCheckpoint::capture(&dir.0, &path, None).is_err());
    assert!(ReviewCheckpoint::capture(&dir.0, &other.file(), None).is_err());
}

#[test]
fn review_progress_survives_restart_without_trusting_saved_status() {
    let dir = Fixture::new();
    fs::write(dir.file(), "original").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &dir.file(), None).unwrap();
    let mut reading = ReadingSession::default();
    reading.reviewed.push(record);
    reading.positions.push(FilePosition {
        path: dir.file(),
        line: 4,
        column: 3,
        scroll_x: -12.,
        scroll_y: -256.,
    });
    reading.pinned.push(PinnedComparison {
        source: ComparisonSource::Files {
            left: dir.file(),
            right: dir.file(),
        },
        top_row: 12,
        inline: Some(false),
    });
    reading.pinned.push(PinnedComparison {
        source: ComparisonSource::Git {
            repo: dir.0.clone(),
            base: "main".into(),
            target: "HEAD".into(),
            path: "code.rs".into(),
            original_path: Some("old.rs".into()),
        },
        top_row: 24,
        inline: Some(true),
    });
    reading
        .extra
        .insert("future_reader".into(), json!({"keep":true}));
    let bytes = serde_json::to_vec(&reading.to_json()).unwrap();
    let restored = ReadingSession::from_json(&serde_json::from_slice::<Value>(&bytes).unwrap());
    assert_eq!(restored, reading);
    fs::write(dir.file(), "external").unwrap();
    assert_eq!(
        verify_checkpoints(&restored.reviewed, &AtomicBool::new(false)),
        [ReviewStatus::Pending]
    );
    assert_eq!(
        verify_checkpoints(&restored.reviewed, &AtomicBool::new(true)),
        [ReviewStatus::Pending]
    );
}

#[test]
fn review_progress_store_survives_window_session_replacement_and_merges_repositories() {
    let first = Fixture::new();
    let second = Fixture::new();
    fs::write(first.file(), "one").unwrap();
    fs::write(second.file(), "two").unwrap();
    let a = ReviewCheckpoint::capture(&first.0, &first.file(), None).unwrap();
    let b = ReviewCheckpoint::capture(&second.0, &second.file(), None).unwrap();
    let path = first.0.join("review-progress.json");
    fs::write(&path, br#"{"future_key":true}"#).unwrap();
    update_store(&path, &a, true).unwrap();
    update_store(&path, &b, true).unwrap();
    crate::session::save_to(
        &first.0.join("session.json"),
        &[crate::session::SavedWindow::default()],
    )
    .unwrap();
    assert_eq!(load_store(&path).unwrap(), [a.clone(), b.clone()]);
    fs::write(first.file(), "new").unwrap();
    let newer = ReviewCheckpoint::capture(&first.0, &first.file(), None).unwrap();
    update_store(&path, &newer, true).unwrap();
    update_store(&path, &a, false).unwrap();
    assert_eq!(load_store(&path).unwrap(), [b.clone(), newer.clone()]);
    update_store(&path, &newer, false).unwrap();
    assert_eq!(load_store(&path).unwrap(), [b]);
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(value["future_key"], true);
}

#[test]
fn review_progress_store_does_not_overwrite_unreadable_or_invalid_records() {
    let dir = Fixture::new();
    fs::write(dir.file(), "read").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &dir.file(), None).unwrap();
    let path = dir.0.join("review-progress.json");
    fs::write(&path, "{ broken").unwrap();
    assert!(update_store(&path, &record, true).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ broken");
    let file = File::create(&path).unwrap();
    file.set_len(MAX_STORE_BYTES + 1).unwrap();
    assert!(load_store(&path).is_err());
    assert!(update_store(&path, &record, true).is_err());
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_STORE_BYTES + 1);
}

#[test]
fn review_progress_rejects_a_file_replaced_by_a_named_pipe_without_blocking() {
    let dir = Fixture::new();
    fs::write(dir.file(), "file").unwrap();
    let record = ReviewCheckpoint::capture(&dir.0, &dir.file(), None).unwrap();
    fs::remove_file(dir.file()).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(dir.file())
            .status()
            .unwrap()
            .success()
    );
    assert!(matches!(record.verify(), ReviewStatus::Unavailable(_)));
    assert!(load_store(&dir.file()).is_err());
}

#[test]
fn review_progress_rejects_invalid_sources_and_bounds_restored_lists() {
    assert!(
        !ComparisonSource::Git {
            repo: "/tmp/repo".into(),
            base: "--help".into(),
            target: "HEAD".into(),
            path: "a.rs".into(),
            original_path: None
        }
        .valid()
    );
    assert!(
        !ComparisonSource::Git {
            repo: "/tmp/repo".into(),
            base: "main".into(),
            target: "HEAD".into(),
            path: "../secret".into(),
            original_path: None
        }
        .valid()
    );
    let pin = json!({"source":{"kind":"files","left":"/tmp/a","right":"/tmp/b"},"top_row":4});
    let reading = ReadingSession::from_json(
        &json!({"positions":[{"path":"/tmp/a","line":0,"column":0,"scroll_x":1,"scroll_y":0}],"pinned":vec![pin;MAX_PINNED+10],"future":7}),
    );
    assert!(reading.positions.is_empty());
    assert_eq!(reading.pinned.len(), MAX_PINNED);
    assert_eq!(reading.extra["future"], 7);
    // Malformed individual fields do not discard the rest of a legacy session.
    assert_eq!(
        ReadingSession::from_json(&Value::Null),
        ReadingSession::default()
    );
}
