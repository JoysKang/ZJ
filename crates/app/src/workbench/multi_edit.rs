//! Group overlapping/adjacent selected lines once, retaining every cursor's position.
use crate::editing::{self, Comment, Edit};
use std::ops::Range;

type Selection = (Range<usize>, bool);
pub(super) type Batch = (Vec<(Range<usize>, String)>, Vec<Selection>);

#[derive(Clone, Copy)]
pub(super) enum Command {
    Comment(Comment, usize),
    Move(bool),
    Copy(bool),
}

fn line_span(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let end_at = if range.end > range.start && text.as_bytes().get(range.end - 1) == Some(&b'\n') {
        range.end - 1
    } else {
        range.end
    };
    let end = text[end_at..].find('\n').map_or(text.len(), |i| end_at + i);
    start..end
}

fn shifted(at: usize, delta: isize) -> usize {
    at.checked_add_signed(delta)
        .expect("mapped selection remains in the document")
}

// A line comment only inserts/removes a prefix; block comments additionally change the last suffix.
fn comment_offset(old: &str, new: &str, at: usize, before: bool, comment: Comment) -> usize {
    if let Comment::Block(open, close) = comment {
        let start = old.len() - old.trim_start_matches([' ', '\t']).len();
        if new.len() > old.len() {
            return at + if at >= start { open.len() + 1 } else { 0 };
        }
        let inner = old[start..].trim_end_matches([' ', '\t']);
        let body = &inner[open.len()..inner.len() - close.len()];
        let lead = usize::from(body.starts_with(' '));
        let trail = usize::from(body.len() > lead && body.ends_with(' '));
        let close_at = start + inner.len() - close.len() - trail;
        return at
            - at.saturating_sub(start).min(open.len() + lead)
            - at.saturating_sub(close_at).min(close.len() + trail);
    }
    let line_start = old[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_number = old[..line_start].bytes().filter(|b| *b == b'\n').count();
    let new_start = if line_number == 0 {
        0
    } else {
        new.match_indices('\n')
            .nth(line_number - 1)
            .map_or(new.len(), |(i, _)| i + 1)
    };
    let old_line = old[line_start..].split('\n').next().unwrap_or("");
    let new_line = new[new_start..].split('\n').next().unwrap_or("");
    let column = at - line_start;
    let mapped = if new_line.len() > old_line.len() {
        let insert_at = new_line.len() - new_line.trim_start_matches([' ', '\t']).len();
        column
            + if column > insert_at || (column == insert_at && !before) {
                new_line.len() - old_line.len()
            } else {
                0
            }
    } else {
        let remove_at = old_line.len() - old_line.trim_start_matches([' ', '\t']).len();
        column
            - column
                .saturating_sub(remove_at)
                .min(old_line.len() - new_line.len())
    };
    new_start + mapped
}

fn move_offset(text: &str, edit: &Edit, block_start: usize, at: usize) -> usize {
    let old_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let row = text[block_start..old_start]
        .bytes()
        .filter(|b| *b == b'\n')
        .count();
    let base = edit.selection.start - edit.range.start;
    let moved = &edit.text[base..];
    let new_start = if row == 0 {
        0
    } else {
        moved
            .match_indices('\n')
            .nth(row - 1)
            .map_or(moved.len(), |(i, _)| i + 1)
    };
    let old_line = text[old_start..]
        .split('\n')
        .next()
        .unwrap_or("")
        .trim_end_matches('\r');
    let column = at - old_start;
    let offset = if column <= old_line.len() {
        new_start + column
    } else {
        moved[new_start..]
            .find('\n')
            .map_or(moved.len(), |i| new_start + i + 1)
    };
    edit.selection.start + offset
}

pub(super) fn compute(text: &str, selections: &[Selection], command: Command) -> Batch {
    let mut groups: Vec<(Range<usize>, Vec<usize>)> = Vec::new();
    let mut ordered: Vec<_> = selections
        .iter()
        .enumerate()
        .map(|(i, (range, _))| (line_span(text, range), i))
        .collect();
    ordered.sort_by_key(|(range, _)| range.start);
    for (range, i) in ordered {
        if let Some((last, members)) = groups
            .last_mut()
            .filter(|(last, _)| range.start <= last.end.saturating_add(1))
        {
            last.end = last.end.max(range.end);
            members.push(i);
        } else {
            groups.push((range, vec![i]));
        }
    }
    let mut edits = Vec::new();
    let mut after = selections.to_vec();
    let mut delta = 0isize;
    for (span, members) in groups {
        let edit: Option<Edit> = match command {
            Command::Comment(comment, width) => {
                editing::toggle_comment(text, span.clone(), comment, width)
            }
            Command::Move(down) => editing::move_lines(text, span.clone(), down),
            Command::Copy(down) => Some(editing::copy_lines(text, span.clone(), down)),
        };
        for i in members {
            let range = &selections[i].0;
            let map = |at, before| match (&edit, command) {
                (Some(edit), Command::Comment(comment, _)) => {
                    if at > edit.range.end {
                        shifted(at, edit.text.len() as isize - edit.range.len() as isize)
                    } else {
                        edit.range.start
                            + comment_offset(
                                &text[edit.range.clone()],
                                &edit.text,
                                at - edit.range.start,
                                before,
                                comment,
                            )
                    }
                }
                (Some(edit), Command::Move(_)) => move_offset(text, edit, span.start, at),
                (Some(edit), Command::Copy(_)) => {
                    shifted(at, edit.selection.start as isize - span.start as isize)
                }
                (None, _) => at,
            };
            after[i].0 = shifted(map(range.start, !range.is_empty()), delta)
                ..shifted(map(range.end, false), delta);
        }
        if let Some(edit) = edit {
            delta += edit.text.len() as isize - edit.range.len() as isize;
            edits.push((edit.range, edit.text));
        }
    }
    (edits, after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(source: &str, selections: &[Selection], command: Command) -> (String, Vec<Selection>) {
        let (edits, selections) = compute(source, selections, command);
        let mut text = source.to_owned();
        for (range, replacement) in edits.into_iter().rev() {
            text.replace_range(range, &replacement);
        }
        (text, selections)
    }

    #[test]
    fn multi_edit_mapping_preserves_single_command_semantics_at_all_character_boundaries() {
        for source in [
            "/\n  α\nend",
            "α\r\n  bb\r\ncc",
            "α\r\n  bb\ncc",
            "<p>α</p>\n  β\n",
            "// α\n// β\n",
            " \n  \n",
        ] {
            let offsets: Vec<_> = source
                .char_indices()
                .map(|(i, _)| i)
                .chain(std::iter::once(source.len()))
                .collect();
            for &start in &offsets {
                for &end in offsets.iter().filter(|end| **end >= start) {
                    let range = start..end;
                    for command in [
                        Command::Comment(Comment::Line("//"), 4),
                        Command::Comment(Comment::Block("<!--", "-->"), 4),
                        Command::Move(true),
                        Command::Move(false),
                        Command::Copy(true),
                        Command::Copy(false),
                    ] {
                        let expected = match command {
                            Command::Comment(comment, width) => {
                                editing::toggle_comment(source, range.clone(), comment, width)
                            }
                            Command::Move(down) => editing::move_lines(source, range.clone(), down),
                            Command::Copy(down) => {
                                Some(editing::copy_lines(source, range.clone(), down))
                            }
                        };
                        let (actual_text, actual_selections) =
                            apply(source, &[(range.clone(), false)], command);
                        let mut expected_text = source.to_owned();
                        let mut expected_range = range.clone();
                        if let Some(edit) = expected {
                            expected_text.replace_range(edit.range, &edit.text);
                            expected_range = edit.selection;
                        }
                        assert_eq!(
                            actual_text, expected_text,
                            "source={source:?}, range={range:?}"
                        );
                        assert_eq!(
                            actual_selections,
                            vec![(expected_range, false)],
                            "source={source:?}, range={range:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn multi_edit_groups_same_and_adjacent_lines_without_losing_cursors() {
        let source = "α\r\n  bb\r\ncc\r\ndd";
        let selections = vec![(0..0, false), (2..2, true), (12..12, false)];
        let (text, after) = apply(
            source,
            &selections,
            Command::Comment(Comment::Line("//"), 4),
        );
        assert_eq!(text, "// α\r\n  bb\r\n// cc\r\ndd");
        assert_eq!(after, vec![(3..3, false), (5..5, true), (18..18, false)]);
        let (text, after) = apply(
            "a\nb\nc\nd",
            &[(0..0, false), (2..2, true)],
            Command::Move(true),
        );
        assert_eq!(text, "c\na\nb\nd");
        assert_eq!(after, vec![(2..2, false), (4..4, true)]);
        let (text, after) = apply(
            "a\nb\nc\nd",
            &[(0..0, false), (4..4, true)],
            Command::Copy(true),
        );
        assert_eq!(text, "a\na\nb\nc\nc\nd");
        assert_eq!(after, vec![(2..2, false), (8..8, true)]);
    }
}
