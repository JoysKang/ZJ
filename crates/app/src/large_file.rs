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
}

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
    let mut file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("不是普通文件"));
    }
    let stamp = FileStamp::of(&metadata);
    let mut checkpoints = vec![0];
    let mut buffer = vec![0; CHUNK];
    let (mut offset, mut newlines, mut last) = (0u64, 0usize, b'\n');
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
        last = chunk[read - 1];
        offset += read as u64;
    }
    Ok(LineIndex {
        bytes: offset,
        // A final line without a newline still counts; an empty file shows one empty line.
        lines: (newlines + usize::from(last != b'\n')).max(1),
        checkpoints,
        stamp,
    })
}

/// Reads the lines in `range` (clamped to the file and to `MAX_READ_LINES`).
pub fn read_lines(
    path: &Path,
    index: &LineIndex,
    range: Range<usize>,
) -> Result<Vec<Line>, ReadError> {
    let mut file = fs::File::open(path)?;
    if FileStamp::of(&file.metadata()?) != index.stamp {
        return Err(ReadError::Changed);
    }
    let end = range.end.min(index.lines).min(range.start + MAX_READ_LINES);
    if range.start >= end {
        return Ok(Vec::new());
    }
    let checkpoint = range.start / STRIDE;
    file.seek(SeekFrom::Start(index.checkpoints[checkpoint]))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    for _ in checkpoint * STRIDE..range.start {
        skip_line(&mut reader)?;
    }
    let mut lines = Vec::with_capacity(end - range.start);
    for _ in range.start..end {
        lines.push(read_line(&mut reader)?);
    }
    Ok(lines)
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
