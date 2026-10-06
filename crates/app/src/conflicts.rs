//! Merge conflict markers in a buffer (`<<<<<<<`, optional `|||||||` base, `=======`,
//! `>>>>>>>`) and their resolutions, as VS Code's 采用当前更改 / 采用传入更改 / 保留双方更改.
//! Byte offsets into the text; no GPUI here.

use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// From the start of the `<<<<<<<` line to the end of the `>>>>>>>` line (with its newline).
    pub whole: Range<usize>,
    /// The `<<<<<<<` line, without its newline.
    pub ours_marker: Range<usize>,
    /// The lines between the markers, each with its newline.
    pub ours: Range<usize>,
    /// From the `|||||||` line (diff3 style), if any, up to `=======`.
    pub base: Option<Range<usize>>,
    /// The `=======` line.
    pub separator: Range<usize>,
    pub theirs: Range<usize>,
    /// The `>>>>>>>` line, without its newline.
    pub theirs_marker: Range<usize>,
    /// 0-based line of `<<<<<<<`.
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    /// 采用当前更改 (HEAD, the `<<<<<<<` side).
    Ours,
    /// 采用传入更改.
    Theirs,
    /// 保留双方更改: ours, then theirs.
    Both,
}

fn is_marker(line: &str, marker: &str) -> bool {
    line.strip_prefix(marker)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

/// Every complete conflict, in order. Markers that do not close are left alone.
pub fn find(text: &str) -> Vec<Conflict> {
    #[derive(Clone)]
    enum State {
        Outside,
        Ours {
            start: usize,
            line: usize,
            marker: Range<usize>,
            body: usize,
        },
        Base {
            open: (usize, usize, Range<usize>, Range<usize>),
            base: usize,
        },
        Theirs {
            open: (usize, usize, Range<usize>, Range<usize>),
            base: Option<Range<usize>>,
            separator: Range<usize>,
            body: usize,
        },
    }
    let mut conflicts = Vec::new();
    let mut state = State::Outside;
    let mut offset = 0;
    for (number, raw) in text.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += raw.len();
        let line = raw.trim_end_matches('\n').trim_end_matches('\r');
        let content = start..start + line.len();
        state = match state {
            _ if is_marker(line, "<<<<<<<") => State::Ours {
                start,
                line: number,
                marker: content,
                body: offset,
            },
            State::Ours {
                start: s,
                line: l,
                marker,
                body,
            } if is_marker(line, "|||||||") => State::Base {
                open: (s, l, marker, body..start),
                base: start,
            },
            State::Ours {
                start: s,
                line: l,
                marker,
                body,
            } if line == "=======" => State::Theirs {
                open: (s, l, marker, body..start),
                base: None,
                separator: content,
                body: offset,
            },
            State::Base { open, base } if line == "=======" => State::Theirs {
                open,
                base: Some(base..start),
                separator: content,
                body: offset,
            },
            State::Theirs {
                open: (s, l, marker, ours),
                base,
                separator,
                body,
            } if is_marker(line, ">>>>>>>") => {
                conflicts.push(Conflict {
                    whole: s..offset,
                    ours_marker: marker,
                    ours,
                    base,
                    separator,
                    theirs: body..start,
                    theirs_marker: content,
                    line: l,
                });
                State::Outside
            }
            other => other,
        };
    }
    conflicts
}

/// The text that replaces `conflict.whole` for `choice`.
pub fn resolve(text: &str, conflict: &Conflict, choice: Choice) -> String {
    let ours = &text[conflict.ours.clone()];
    let theirs = &text[conflict.theirs.clone()];
    match choice {
        Choice::Ours => ours.to_string(),
        Choice::Theirs => theirs.to_string(),
        Choice::Both => format!("{ours}{theirs}"),
    }
}

/// The conflict the cursor (a byte offset) is in, else the next one after it, else the last.
pub fn current(conflicts: &[Conflict], cursor: usize) -> Option<usize> {
    conflicts
        .iter()
        .position(|c| cursor < c.whole.end)
        .or(conflicts.len().checked_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a\n<<<<<<< HEAD\nours 1\nours 2\n=======\ntheirs\n>>>>>>> feature\nb\n<<<<<<< HEAD\nx\r\n||||||| base\nold\n=======\ny\r\n>>>>>>> other\n";

    #[test]
    fn finds_two_way_and_diff3_conflicts() {
        let conflicts = find(TEXT);
        assert_eq!(conflicts.len(), 2);
        let first = &conflicts[0];
        assert_eq!(first.line, 1);
        assert_eq!(&TEXT[first.ours_marker.clone()], "<<<<<<< HEAD");
        assert_eq!(&TEXT[first.ours.clone()], "ours 1\nours 2\n");
        assert_eq!(&TEXT[first.theirs.clone()], "theirs\n");
        assert_eq!(&TEXT[first.theirs_marker.clone()], ">>>>>>> feature");
        assert!(first.base.is_none());
        let second = &conflicts[1];
        assert_eq!(second.line, 8);
        assert_eq!(&TEXT[second.ours.clone()], "x\r\n");
        assert_eq!(&TEXT[second.base.clone().unwrap()], "||||||| base\nold\n");
        assert_eq!(&TEXT[second.theirs.clone()], "y\r\n");
        assert!(TEXT[second.whole.clone()].ends_with(">>>>>>> other\n"));
    }

    #[test]
    fn resolves_each_way() {
        let conflicts = find(TEXT);
        let apply = |choice| {
            let c = &conflicts[0];
            format!(
                "{}{}{}",
                &TEXT[..c.whole.start],
                resolve(TEXT, c, choice),
                &TEXT[c.whole.end..]
            )
        };
        assert!(apply(Choice::Ours).starts_with("a\nours 1\nours 2\nb\n"));
        assert!(apply(Choice::Theirs).starts_with("a\ntheirs\nb\n"));
        assert!(apply(Choice::Both).starts_with("a\nours 1\nours 2\ntheirs\nb\n"));
    }

    #[test]
    fn unclosed_or_lookalike_markers_are_not_conflicts() {
        assert!(find("<<<<<<< HEAD\nx\n=======\ny\n").is_empty());
        assert!(find("<<<<<<<<< not\n========\n>>>>>>>>\n").is_empty());
        assert_eq!(find("<<<<<<<\n=======\n>>>>>>>").len(), 1);
        // A marker at the end of the text without a newline still closes.
        assert_eq!(find("<<<<<<<\n=======\n>>>>>>>")[0].whole.end, 23);
    }

    #[test]
    fn the_current_conflict_follows_the_cursor() {
        let conflicts = find(TEXT);
        assert_eq!(current(&conflicts, 0), Some(0));
        assert_eq!(current(&conflicts, conflicts[0].whole.end), Some(1));
        assert_eq!(current(&conflicts, TEXT.len()), Some(1));
        assert_eq!(current(&[], 0), None);
    }
}
