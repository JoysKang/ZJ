//! Restricted viewing (开发说明「大文件与降级」, A18): files the editor does not open — over
//! 8 MB, with a line over 256 KiB, or not UTF-8 — are shown read-only, without highlighting,
//! wrapping or search, at a memory cost that does not grow with the file.
//!
//! One pass builds a sparse line index (the offset of every `STRIDE`th line: 80 KB for ten
//! million lines); the viewer then reads only the lines on screen. No GPUI here.

use crate::files::FileStamp;
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    ops::Range,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

/// Lines between two recorded offsets.
pub const STRIDE: usize = 1024;
/// Bytes of a line that are shown; the rest of a longer line is cut with `…`.
pub const SHOWN_LINE_BYTES: usize = 4096;
/// Lines read at most per request (a screen is well under this).
pub const MAX_READ_LINES: usize = 4096;
/// The prefix checked for NUL bytes: a file with one is binary and not shown.
pub const BINARY_PROBE: usize = 64 * 1024;
const CHUNK: usize = 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct LineIndex {
    pub bytes: u64,
    pub lines: usize,
    /// `checkpoints[k]` is the byte offset where line `k * STRIDE` starts.
    checkpoints: Vec<u64>,
    /// The file as indexed; a read that finds it different reports `Changed`.
    pub stamp: FileStamp,
    /// Line breaks seen, and the last bytes indexed: a file that only grew (a log) is indexed
    /// on from here, once those bytes are found unchanged.
    newlines: usize,
    tail: Vec<u8>,
}

/// Bytes kept from the end of an index to recognize a file that was only appended to.
const TAIL: usize = 256;

/// A line as shown: lossily decoded, tabs expanded, cut at `SHOWN_LINE_BYTES`.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub cut: bool,
}

#[derive(Debug)]
pub enum ReadError {
    /// The file changed since it was indexed: index it again.
    Changed,
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        ReadError::Io(error)
    }
}

/// Indexes `path` in one pass. `cancel` stops it (the viewer was closed or reopened).
pub fn index(path: &Path, cancel: &AtomicBool) -> io::Result<LineIndex> {
    index_from(path, None, cancel)
}

/// Indexes `path` again after a change: from where `old` ended when the file is the same one
/// and only grew (its last indexed bytes unchanged), otherwise from the start.
pub fn reindex(path: &Path, old: &LineIndex, cancel: &AtomicBool) -> io::Result<LineIndex> {
    index_from(path, Some(old), cancel)
}

fn index_from(path: &Path, old: Option<&LineIndex>, cancel: &AtomicBool) -> io::Result<LineIndex> {
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("不是普通文件"));
    }
    let stamp = FileStamp::of(&metadata);
    let appended = old.filter(|old| {
        old.stamp.device == stamp.device
            && old.stamp.inode == stamp.inode
            && stamp.len >= old.bytes
            && {
                let mut tail = vec![0; old.tail.len()];
                file.seek(SeekFrom::Start(old.bytes - old.tail.len() as u64))
                    .and_then(|_| file.read_exact(&mut tail))
                    .is_ok_and(|_| tail == old.tail)
            }
    });
    let mut buffer = vec![0; CHUNK];
    let (mut checkpoints, mut offset, mut newlines, mut tail) = match appended {
        Some(old) => (
            old.checkpoints.clone(),
            old.bytes,
            old.newlines,
            old.tail.clone(),
        ),
        None => (vec![0], 0u64, 0usize, Vec::new()),
    };
    file.seek(SeekFrom::Start(offset))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "已取消"));
        }
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        if offset < BINARY_PROBE as u64 {
            let probe = (BINARY_PROBE as u64 - offset).min(read as u64) as usize;
            if chunk[..probe].contains(&0) {
                return Err(io::Error::other("二进制文件，不显示"));
            }
        }
        for (at, _) in chunk.iter().enumerate().filter(|(_, byte)| **byte == b'\n') {
            newlines += 1;
            if newlines % STRIDE == 0 {
                checkpoints.push(offset + at as u64 + 1);
            }
        }
        tail.extend_from_slice(&chunk[read.saturating_sub(TAIL)..]);
        let excess = tail.len().saturating_sub(TAIL);
        tail.drain(..excess);
        offset += read as u64;
    }
    let ends_with_break = tail.last().is_none_or(|byte| *byte == b'\n');
    Ok(LineIndex {
        bytes: offset,
        // A final line without a newline still counts; an empty file shows one empty line.
        lines: (newlines + usize::from(!ends_with_break)).max(1),
        checkpoints,
        stamp,
        newlines,
        tail,
    })
}

/// Reads the lines in `range` (clamped to the file and to `MAX_READ_LINES`).
pub fn read_lines(
    path: &Path,
    index: &LineIndex,
    range: Range<usize>,
) -> Result<Vec<Line>, ReadError> {
    let end = range.end.min(index.lines).min(range.start + MAX_READ_LINES);
    if range.start >= end {
        // Still a read: a changed file is noticed here too.
        reader_at(path, index, 0)?;
        return Ok(Vec::new());
    }
    let mut reader = reader_at(path, index, range.start)?;
    let mut lines = Vec::with_capacity(end - range.start);
    for _ in range.start..end {
        lines.push(read_line(&mut reader)?);
    }
    Ok(lines)
}

/// Most text copied out of a large file at once.
pub const MAX_COPY_BYTES: usize = 16 * 1024 * 1024;

/// A reader at the start of line `line`, after checking the file is the one indexed.
fn reader_at(
    path: &Path,
    index: &LineIndex,
    line: usize,
) -> Result<BufReader<fs::File>, ReadError> {
    let mut file = fs::File::open(path)?;
    if FileStamp::of(&file.metadata()?) != index.stamp {
        return Err(ReadError::Changed);
    }
    let checkpoint = (line / STRIDE).min(index.checkpoints.len() - 1);
    file.seek(SeekFrom::Start(index.checkpoints[checkpoint]))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    for _ in checkpoint * STRIDE..line {
        skip_line(&mut reader)?;
    }
    Ok(reader)
}

/// The full text of the lines in `range` (lossily decoded), for copying; refused over
/// `MAX_COPY_BYTES`.
pub fn read_text(path: &Path, index: &LineIndex, range: Range<usize>) -> Result<String, ReadError> {
    let end = range.end.min(index.lines);
    let mut reader = reader_at(path, index, range.start)?;
    let mut bytes = Vec::new();
    let mut left = end.saturating_sub(range.start);
    // Chunk by chunk, so one huge line stops at the limit instead of being read whole.
    while left > 0 {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let (take, line_ends) = match buffer.iter().position(|b| *b == b'\n') {
            Some(at) => (at + 1, true),
            None => (buffer.len(), false),
        };
        if bytes.len() + take > MAX_COPY_BYTES {
            return Err(ReadError::Io(io::Error::other(format!(
                "选中的行超过 {} MB，没有复制",
                MAX_COPY_BYTES / 1024 / 1024
            ))));
        }
        bytes.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if line_ends {
            left -= 1;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Whether `line` contains `query`: case-sensitive when the query has an uppercase letter,
/// otherwise ASCII case-insensitive (VS Code's smart case).
fn contains(line: &[u8], query: &[u8], sensitive: bool) -> bool {
    !query.is_empty()
        && line.windows(query.len()).any(|window| {
            if sensitive {
                window == query
            } else {
                window.eq_ignore_ascii_case(query)
            }
        })
}

/// The next (`forward`) or previous line after / before `from` that contains `query`,
/// wrapping around the file; `from` itself is checked last. `cancel` stops the scan.
pub fn find(
    path: &Path,
    index: &LineIndex,
    query: &str,
    from: usize,
    forward: bool,
    cancel: &AtomicBool,
) -> Result<Option<usize>, ReadError> {
    let sensitive = query.chars().any(char::is_uppercase);
    let query = query.as_bytes();
    let lines = index.lines;
    let from = from.min(lines.saturating_sub(1));
    // One pass from the top keeps the scan sequential; it remembers the matches it needs.
    let mut reader = reader_at(path, index, 0)?;
    let (mut first, mut last_before, mut first_after, mut last) = (None, None, None, None);
    for number in 0..lines {
        if number % STRIDE == 0 && cancel.load(Ordering::Relaxed) {
            return Err(ReadError::Io(io::Error::new(
                io::ErrorKind::Interrupted,
                "已取消",
            )));
        }
        if !line_contains(&mut reader, query, sensitive, cancel)? {
            continue;
        }
        first.get_or_insert(number);
        last = Some(number);
        if number < from {
            last_before = Some(number);
        } else if number > from && first_after.is_none() {
            first_after = Some(number);
            if forward {
                break;
            }
        }
    }
    Ok(if forward {
        first_after.or(first)
    } else {
        last_before.or(last)
    })
}

/// Consumes the next line and says whether it contains `query`, holding at most a buffer
/// and `query.len() - 1` carried bytes however long the line is.
fn line_contains(
    reader: &mut impl BufRead,
    query: &[u8],
    sensitive: bool,
    cancel: &AtomicBool,
) -> io::Result<bool> {
    let keep = query.len().saturating_sub(1);
    let mut window: Vec<u8> = Vec::new();
    let mut found = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(found);
        }
        let (take, line_ends) = match buffer.iter().position(|b| *b == b'\n') {
            Some(at) => (at, true),
            None => (buffer.len(), false),
        };
        if !found {
            window.extend_from_slice(&buffer[..take]);
            found = contains(&window, query, sensitive);
            let drop = window.len().saturating_sub(keep);
            window.drain(..drop);
        }
        reader.consume(if line_ends { take + 1 } else { take });
        if line_ends {
            return Ok(found);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "已取消"));
        }
    }
}

/// Consumes up to and including the next newline; returns the bytes consumed.
fn skip_line(reader: &mut impl BufRead) -> io::Result<usize> {
    let mut consumed = 0;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return Ok(consumed);
        }
        match buffer.iter().position(|byte| *byte == b'\n') {
            Some(at) => {
                reader.consume(at + 1);
                return Ok(consumed + at + 1);
            }
            None => {
                let len = buffer.len();
                reader.consume(len);
                consumed += len;
            }
        }
    }
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Line> {
    let mut bytes = Vec::new();
    let mut cut = false;
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        let (take, done) = match buffer.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at, true),
            None => (buffer.len(), false),
        };
        let room = SHOWN_LINE_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..take.min(room)]);
        cut |= take > room;
        reader.consume(if done { take + 1 } else { take });
        if done {
            break;
        }
        if cut {
            skip_line(reader)?;
            break;
        }
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let text = String::from_utf8_lossy(&bytes);
    // A cut may split a character: drop the replacement character it leaves at the end.
    let text = if cut {
        text.trim_end_matches('\u{fffd}')
    } else {
        &text
    };
    Ok(Line {
        text: text.replace('\t', "    "),
        cut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("zj-large-{name}-{}", std::process::id()));
        fs::write(&path, contents).unwrap();
        path
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|line| line.text.as_str()).collect()
    }

    #[test]
    fn indexes_sparsely_and_reads_any_range() {
        let mut contents = String::new();
        for n in 0..(STRIDE * 4 + 10) {
            contents.push_str(&format!("line {n}\r\n"));
        }
        let path = temp("ranges", contents.as_bytes());
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.lines, STRIDE * 4 + 10);
        assert_eq!(index.checkpoints.len(), 5);
        assert_eq!(index.bytes, contents.len() as u64);
        let read = |range| read_lines(&path, &index, range).unwrap();
        assert_eq!(texts(&read(0..2)), ["line 0", "line 1"]);
        let across = read(STRIDE - 1..STRIDE + 1);
        assert_eq!(
            texts(&across),
            [format!("line {}", STRIDE - 1), format!("line {STRIDE}")]
        );
        let tail = read(STRIDE * 4 + 8..usize::MAX);
        assert_eq!(
            texts(&tail),
            [
                format!("line {}", STRIDE * 4 + 8),
                format!("line {}", STRIDE * 4 + 9)
            ]
        );
        assert!(read(index.lines..index.lines + 5).is_empty());
        assert_eq!(read(0..usize::MAX).len(), MAX_READ_LINES);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn copies_whole_lines_and_finds_with_smart_case_wrapping_around() {
        let mut contents = String::new();
        for n in 0..(STRIDE * 2) {
            contents.push_str(&format!(
                "{}\n",
                if n % 700 == 5 { "Error here" } else { "fine" }
            ));
        }
        let path = temp("find", contents.as_bytes());
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            read_text(&path, &index, 4..7).unwrap(),
            "fine\nError here\nfine\n"
        );
        let find = |query, from, forward| {
            find(&path, &index, query, from, forward, &AtomicBool::new(false)).unwrap()
        };
        // Matches at 5, 705, 1405.
        assert_eq!(find("error", 0, true), Some(5));
        assert_eq!(find("error", 5, true), Some(705));
        assert_eq!(find("error", 1405, true), Some(5));
        assert_eq!(find("error", 705, false), Some(5));
        assert_eq!(find("error", 5, false), Some(1405));
        assert_eq!(find("ERROR", 0, true), None);
        assert_eq!(find("Error", 0, true), Some(5));
        assert_eq!(find("missing", 0, true), None);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn long_lines_are_searched_and_copied_without_holding_them_whole() {
        // The match straddles the 64 KiB buffer boundary of a long line.
        let mut contents = "a".repeat(64 * 1024 - 3);
        contents.push_str("needle");
        contents.push_str(&"a".repeat(100_000));
        contents.push_str("\nshort\n");
        let path = temp("long-find", contents.as_bytes());
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(
            find(&path, &index, "needle", 1, true, &cancel).unwrap(),
            Some(0)
        );
        assert_eq!(
            find(&path, &index, "short", 0, true, &cancel).unwrap(),
            Some(1)
        );
        assert_eq!(read_text(&path, &index, 1..2).unwrap(), "short\n");
        // A line over the copy limit is refused, not read whole.
        let huge = temp("long-copy", &vec![b'x'; MAX_COPY_BYTES + 1]);
        let huge_index = super::index(&huge, &AtomicBool::new(false)).unwrap();
        assert!(read_text(&huge, &huge_index, 0..1).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(huge).unwrap();
    }

    #[test]
    fn a_file_that_only_grew_is_indexed_on_and_a_rewritten_one_from_the_start() {
        let mut contents = String::new();
        for n in 0..(STRIDE + 5) {
            contents.push_str(&format!("line {n}\n"));
        }
        contents.push_str("partial");
        let path = temp("grow", contents.as_bytes());
        let cancel = AtomicBool::new(false);
        let old = index(&path, &cancel).unwrap();
        // Appended: the partial line gets its end, and more lines follow.
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        for n in 0..(STRIDE + 3) {
            writeln!(file, " end {n}").unwrap();
        }
        drop(file);
        let grown = reindex(&path, &old, &cancel).unwrap();
        assert_eq!(grown, index(&path, &cancel).unwrap());
        assert_eq!(grown.lines, 2 * STRIDE + 8);
        // Rewritten with the same length: indexed from the start, not "appended".
        let bytes = fs::read(&path).unwrap();
        let rewritten: Vec<u8> = bytes
            .iter()
            .map(|b| if *b == b'e' { b'\n' } else { *b })
            .collect();
        fs::write(&path, &rewritten).unwrap();
        assert_eq!(
            reindex(&path, &grown, &cancel).unwrap(),
            index(&path, &cancel).unwrap()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn last_line_without_newline_and_empty_files_count() {
        let path = temp("tail", b"a\nb");
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.lines, 2);
        assert_eq!(texts(&read_lines(&path, &index, 0..9).unwrap()), ["a", "b"]);
        fs::write(&path, b"").unwrap();
        let index = super::index(&path, &AtomicBool::new(false)).unwrap();
        assert_eq!(index.lines, 1);
        assert_eq!(texts(&read_lines(&path, &index, 0..1).unwrap()), [""]);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn long_lines_are_cut_and_the_next_line_still_reads() {
        let mut contents = "é".repeat(SHOWN_LINE_BYTES); // two bytes each
        contents.push_str("\tafter\nnext\n");
        let path = temp("long", contents.as_bytes());
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        let lines = read_lines(&path, &index, 0..2).unwrap();
        assert!(lines[0].cut);
        assert_eq!(lines[0].text, "é".repeat(SHOWN_LINE_BYTES / 2));
        assert_eq!(
            lines[1],
            Line {
                text: "next".into(),
                cut: false
            }
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn non_utf8_shows_lossily_binary_is_refused_and_changes_are_noticed() {
        let path = temp("gbk", b"\xc4\xe3\xba\xc3\tok\n");
        let index = index(&path, &AtomicBool::new(false)).unwrap();
        let line = &read_lines(&path, &index, 0..1).unwrap()[0];
        assert!(line.text.ends_with("    ok"));
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"more\n").unwrap();
        drop(file);
        assert!(matches!(
            read_lines(&path, &index, 0..1),
            Err(ReadError::Changed)
        ));
        fs::write(&path, b"text\0binary").unwrap();
        assert!(super::index(&path, &AtomicBool::new(false)).is_err());
        assert!(super::index(&path, &AtomicBool::new(true)).is_err());
        fs::remove_file(path).unwrap();
    }
}
