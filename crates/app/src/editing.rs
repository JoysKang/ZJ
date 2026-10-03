//! Line editing commands as pure functions of the buffer text and the selection, following
//! VS Code: toggle line comment (⌘/), move and copy lines (⌥↑↓, ⇧⌥↑↓), and the word and
//! next match for ⌘D. Offsets are byte offsets; line breaks are `\n` or `\r\n` (CRLF files
//! keep `\r\n` in the buffer).

use std::ops::Range;

/// How a language comments out lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comment {
    /// Each line gets the token (`//`, `#`, `--`).
    Line(&'static str),
    /// No line comment: the lines are wrapped in one block comment, as VS Code does.
    Block(&'static str, &'static str),
}

/// Replace `range` of the old text with `text`, then select `selection` (new-text offsets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Line {
    start: usize,
    /// End of the content, before `\n` / `\r\n`.
    end: usize,
    /// Start of the next line; `end` for a last line without a break.
    next: usize,
}

fn line_at(text: &str, offset: usize) -> Line {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    match text[start..].find('\n') {
        Some(i) => {
            let newline = start + i;
            let cr = newline > start && text.as_bytes()[newline - 1] == b'\r';
            Line {
                start,
                end: newline - usize::from(cr),
                next: newline + 1,
            }
        }
        None => Line {
            start,
            end: text.len(),
            next: text.len(),
        },
    }
}

/// The lines the selection touches. A selection ending at the start of a line does not
/// include that line (VS Code), unless it is the only one.
fn selected_lines(text: &str, selection: &Range<usize>) -> Vec<Line> {
    let first = line_at(text, selection.start);
    let mut last = line_at(text, selection.end);
    if selection.end > selection.start && selection.end == last.start && last.start > first.start {
        last = line_at(text, last.start - 1);
    }
    let mut lines = vec![first];
    while let Some(line) = lines.last().filter(|line| line.start < last.start) {
        let next = line_at(text, line.next);
        lines.push(next);
    }
    lines
}

/// Bytes of leading spaces and tabs.
fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The column after `c` at `column`: a tab goes to the next tab stop.
fn advance(column: usize, c: char, tab_size: usize) -> usize {
    match c {
        '\t' => (column / tab_size + 1) * tab_size,
        _ => column + 1,
    }
}

fn columns(whitespace: &str, tab_size: usize) -> usize {
    whitespace
        .chars()
        .fold(0, |column, c| advance(column, c, tab_size))
}

/// Byte offset in `line` where its indentation reaches `column`.
fn byte_at_column(line: &str, column: usize, tab_size: usize) -> usize {
    let mut at = 0;
    for (i, c) in line.char_indices() {
        if at >= column || !matches!(c, ' ' | '\t') {
            return i;
        }
        at = advance(at, c, tab_size);
    }
    line.len()
}

/// One change inside an edit: replace `len` bytes at `at` with `insert`.
#[derive(Debug)]
struct Splice {
    at: usize,
    len: usize,
    insert: String,
    /// Where an offset exactly at an insertion goes: `Some(true)` stays before it,
    /// `Some(false)` moves after it, `None` follows the selection (see [`map_offset`]).
    before: Option<bool>,
}

impl Splice {
    fn insert(at: usize, insert: String, before: Option<bool>) -> Self {
        Self {
            at,
            len: 0,
            insert,
            before,
        }
    }

    fn remove(at: usize, len: usize) -> Self {
        Self {
            at,
            len,
            insert: String::new(),
            before: None,
        }
    }
}

/// Moves `offset` through ascending, non-overlapping splices, the way text follows an edit.
fn map_offset(offset: usize, splices: &[Splice], before: bool) -> usize {
    let mut delta = 0isize;
    for splice in splices {
        let stays = splice.before.unwrap_or(before);
        if offset < splice.at || (offset == splice.at && (splice.len > 0 || stays)) {
            break;
        }
        if offset >= splice.at + splice.len {
            delta += splice.insert.len() as isize - splice.len as isize;
            continue;
        }
        return (splice.at as isize + delta) as usize;
    }
    (offset as isize + delta) as usize
}

/// Applies the splices as one replacement of `region`. A non-empty selection grows over text
/// inserted at its start; a cursor moves past text inserted where it is.
fn splice(text: &str, region: Range<usize>, splices: &[Splice], selection: &Range<usize>) -> Edit {
    let mut out = String::with_capacity(region.len() + 16 * splices.len());
    let mut at = region.start;
    for splice in splices {
        out.push_str(&text[at..splice.at]);
        out.push_str(&splice.insert);
        at = splice.at + splice.len;
    }
    out.push_str(&text[at..region.end]);
    let cursor = selection.is_empty();
    let start = map_offset(selection.start, splices, !cursor);
    let end = if cursor {
        start
    } else {
        map_offset(selection.end, splices, false)
    };
    Edit {
        range: region,
        text: out,
        selection: start..end,
    }
}

/// ⌘/: comments out the selected lines, or uncomments them when every non-blank one already
/// is. Line comments go at the smallest indentation (in columns, `tab_size` per tab) followed
/// by a space; blank lines are skipped unless all lines are blank. Languages without a line
/// comment wrap the lines in one block comment, from the first line's indentation to the end
/// of the last line. The replaced range covers whole lines.
pub fn toggle_comment(
    text: &str,
    selection: Range<usize>,
    comment: Comment,
    tab_size: usize,
) -> Option<Edit> {
    let lines = selected_lines(text, &selection);
    let splices = match comment {
        Comment::Line(token) => line_comment(text, &lines, token, tab_size.max(1)),
        Comment::Block(open, close) => block_comment(text, &lines, open, close),
    };
    let (first, last) = (splices.first()?, splices.last()?);
    let region = lines[0].start.min(first.at)..lines[lines.len() - 1].end.max(last.at + last.len);
    Some(splice(text, region, &splices, &selection))
}

fn line_comment(text: &str, lines: &[Line], token: &'static str, tab_size: usize) -> Vec<Splice> {
    let content = |line: &Line| &text[line.start..line.end];
    let mut targets: Vec<&Line> = lines
        .iter()
        .filter(|line| indent_len(content(line)) < content(line).len())
        .collect();
    let blank = targets.is_empty();
    if blank {
        targets = lines.iter().collect();
    }
    let commented = !blank
        && targets.iter().all(|line| {
            let line = content(line);
            line[indent_len(line)..].starts_with(token)
        });
    if commented {
        return targets
            .iter()
            .map(|line| {
                let at = line.start + indent_len(content(line));
                let space = text[at + token.len()..line.end].starts_with(' ');
                Splice::remove(at, token.len() + usize::from(space))
            })
            .collect();
    }
    let column = targets
        .iter()
        .map(|line| {
            let line = content(line);
            columns(&line[..indent_len(line)], tab_size)
        })
        .min()
        .unwrap_or(0);
    targets
        .iter()
        .map(|line| {
            let at = line.start + byte_at_column(content(line), column, tab_size);
            Splice::insert(at, format!("{token} "), None)
        })
        .collect()
}

fn block_comment(text: &str, lines: &[Line], open: &str, close: &str) -> Vec<Splice> {
    let (first, last) = (lines[0], lines[lines.len() - 1]);
    let start = first.start + indent_len(&text[first.start..first.end]);
    let end = last.end.max(start);
    let inner = text[start..end].trim_end_matches([' ', '\t']);
    if inner.len() >= open.len() + close.len() && inner.starts_with(open) && inner.ends_with(close)
    {
        let body = &inner[open.len()..inner.len() - close.len()];
        let lead = usize::from(body.starts_with(' '));
        let trail = usize::from(body.len() > lead && body.ends_with(' '));
        let close_at = start + inner.len() - close.len() - trail;
        return vec![
            Splice::remove(start, open.len() + lead),
            Splice::remove(close_at, close.len() + trail),
        ];
    }
    // The selection stays on the commented text, not on the markers.
    vec![
        Splice::insert(start, format!("{open} "), Some(false)),
        Splice::insert(end, format!(" {close}"), Some(true)),
    ]
}

/// ⌥↑ / ⌥↓: swaps the selected lines with the line above or below; `None` at the first /
/// last line. Line breaks stay where they were, so a last line without one stays without
/// one, and CRLF stays CRLF. The selection moves with the lines.
pub fn move_lines(text: &str, selection: Range<usize>, down: bool) -> Option<Edit> {
    let block = selected_lines(text, &selection);
    let count = block.len();
    let (first, last) = (block[0], block[count - 1]);
    let mut lines = block;
    // `order[slot]` is the line (index into `lines`) that ends up in `slot`.
    let order: Vec<usize> = if down {
        if last.next == last.end {
            return None;
        }
        lines.push(line_at(text, last.next));
        std::iter::once(count).chain(0..count).collect()
    } else {
        if first.start == 0 {
            return None;
        }
        lines.insert(0, line_at(text, first.start - 1));
        (1..=count).chain(std::iter::once(0)).collect()
    };
    let region = lines[0].start..lines[lines.len() - 1].next;
    let mut out = String::with_capacity(region.len());
    let mut moved_to = vec![(0, 0); lines.len()];
    for (slot, &index) in order.iter().enumerate() {
        let start = region.start + out.len();
        out.push_str(&text[lines[index].start..lines[index].end]);
        out.push_str(&text[lines[slot].end..lines[slot].next]);
        moved_to[index] = (start, region.start + out.len());
    }
    let block = if down { 0..count } else { 1..count + 1 };
    let map = |offset: usize| {
        for index in block.clone() {
            let line = lines[index];
            if offset <= line.end {
                return moved_to[index].0 + offset.saturating_sub(line.start);
            }
        }
        // A selection ending at the start of the next line keeps doing so.
        moved_to[block.end - 1].1
    };
    Some(Edit {
        range: region,
        text: out,
        selection: map(selection.start)..map(selection.end),
    })
}

/// ⇧⌥↑ / ⇧⌥↓: duplicates the selected lines above or below and selects the new copy (above,
/// the copy is the upper one). A last line without a break gets the document's line break
/// between the two copies.
pub fn copy_lines(text: &str, selection: Range<usize>, down: bool) -> Edit {
    let lines = selected_lines(text, &selection);
    let (first, last) = (lines[0], lines[lines.len() - 1]);
    let region = first.start..last.next;
    let block = &text[region.clone()];
    let between = if last.next > last.end {
        ""
    } else {
        crate::replace::eol_of(text)
    };
    let shift = if down { block.len() + between.len() } else { 0 };
    Edit {
        range: region,
        text: format!("{block}{between}{block}"),
        selection: selection.start + shift..selection.end + shift,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// ⌘D with no selection: the word the cursor is in or touches (the one after it first).
pub fn word_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    (start < end).then_some(start..end)
}

/// ⌘D with a selection: the next occurrence of the selected text after it, wrapping around
/// at the end; case-sensitive, and only whole words when `whole_word` (the selection came
/// from the word under the cursor). `None` when the selection is the only one.
pub fn next_occurrence(
    text: &str,
    selection: Range<usize>,
    whole_word: bool,
) -> Option<Range<usize>> {
    let needle = &text[selection.clone()];
    if needle.is_empty() {
        return None;
    }
    let bounded = |start: usize| {
        let end = start + needle.len();
        !whole_word
            || (!text[..start].chars().next_back().is_some_and(is_word)
                && !text[end..].chars().next().is_some_and(is_word))
    };
    let after = text[selection.end..]
        .match_indices(needle)
        .map(|(i, _)| selection.end + i);
    let wrapped = text[..selection.end].match_indices(needle).map(|(i, _)| i);
    after
        .chain(wrapped)
        .find(|&start| start != selection.start && bounded(start))
        .map(|start| start..start + needle.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLASHES: Comment = Comment::Line("//");
    const HTML: Comment = Comment::Block("<!--", "-->");

    /// Applies `edit` and marks the new selection with `[` `]` (or `|` for a cursor).
    fn show(text: &str, edit: Option<Edit>) -> String {
        let Some(edit) = edit else {
            return "(none)".into();
        };
        let mut new = text.to_string();
        new.replace_range(edit.range.clone(), &edit.text);
        let Range { start, end } = edit.selection;
        if start == end {
            new.insert(start, '|');
        } else {
            new.insert(end, ']');
            new.insert(start, '[');
        }
        new
    }

    /// `text` with `|` for a cursor or `[` `]` around a selection, the markers removed.
    fn parse(marked: &str) -> (String, Range<usize>) {
        if let Some(at) = marked.find('|') {
            return (marked.replacen('|', "", 1), at..at);
        }
        let start = marked.find('[').unwrap();
        let end = marked.find(']').unwrap() - 1;
        (marked.replacen('[', "", 1).replacen(']', "", 1), start..end)
    }

    fn comment(marked: &str, comment: Comment) -> String {
        let (text, selection) = parse(marked);
        show(&text, toggle_comment(&text, selection, comment, 4))
    }

    fn moved(marked: &str, down: bool) -> String {
        let (text, selection) = parse(marked);
        show(&text, move_lines(&text, selection, down))
    }

    fn copied(marked: &str, down: bool) -> String {
        let (text, selection) = parse(marked);
        show(&text, Some(copy_lines(&text, selection, down)))
    }

    #[test]
    fn moves_a_line_and_stops_at_the_ends() {
        assert_eq!(moved("a\nb|b\nc\n", true), "a\nc\nb|b\n");
        assert_eq!(moved("a\nb|b\nc\n", false), "b|b\na\nc\n");
        assert_eq!(moved("a|a\nb\n", false), "(none)");
        assert_eq!(moved("a\nb|b", true), "(none)");
        // The empty line after a final break is a line too (VS Code).
        assert_eq!(moved("a\nb|b\n", true), "a\n\nb|b");
    }

    #[test]
    fn moves_a_block_of_lines() {
        assert_eq!(moved("a\n[b\nc]\nd\n", true), "a\nd\n[b\nc]\n");
        assert_eq!(moved("a\n[b\nc]\nd\n", false), "[b\nc]\na\nd\n");
        assert_eq!(
            moved("a\n[b\nc\n]d\n", false),
            "[b\nc\n]a\nd\n",
            "a selection ending at a line start moves the lines above it"
        );
        assert_eq!(moved("a\n[b\nc\n]d\n", true), "a\nd\n[b\nc\n]");
    }

    #[test]
    fn moving_keeps_the_last_line_without_a_break() {
        assert_eq!(moved("a|a\nb", true), "b\na|a");
        assert_eq!(moved("a\nb|b", false), "b|b\na");
        assert_eq!(moved("x\r\na|a\r\nb", true), "x\r\nb\r\na|a");
        assert_eq!(moved("x\r\na\r\nb|", false), "x\r\nb|\r\na");
    }

    #[test]
    fn copies_lines_up_and_down() {
        assert_eq!(copied("a\nb|b\nc\n", true), "a\nbb\nb|b\nc\n");
        assert_eq!(copied("a\nb|b\nc\n", false), "a\nb|b\nbb\nc\n");
        assert_eq!(copied("[a\nb]\nc", true), "a\nb\n[a\nb]\nc");
        assert_eq!(copied("[a\nb\n]c", false), "[a\nb\n]a\nb\nc");
        assert_eq!(copied("a\nb|", true), "a\nb\nb|");
        assert_eq!(copied("a\r\nb|", false), "a\r\nb|\r\nb");
        assert_eq!(copied("|", true), "\n|");
    }

    #[test]
    fn word_under_the_cursor() {
        let word = |marked: &str| {
            let (text, selection) = parse(marked);
            word_at(&text, selection.start).map(|range| text[range].to_string())
        };
        assert_eq!(word("let fo|o = 1;").as_deref(), Some("foo"));
        assert_eq!(word("let |foo = 1;").as_deref(), Some("foo"));
        assert_eq!(word("let foo| = 1;").as_deref(), Some("foo"));
        assert_eq!(word("a.b|_c(d)").as_deref(), Some("b_c"));
        assert_eq!(word("变量|名 = 1").as_deref(), Some("变量名"));
        assert_eq!(word("a = | 1"), None);
        assert_eq!(word("|"), None);
    }

    #[test]
    fn next_occurrence_wraps_and_matches_case_and_words() {
        let next = |marked: &str, whole_word: bool| {
            let (text, selection) = parse(marked);
            let found = next_occurrence(&text, selection, whole_word);
            show(
                &text,
                found.map(|selection| Edit {
                    range: 0..0,
                    text: String::new(),
                    selection,
                }),
            )
        };
        assert_eq!(next("[a] b a", false), "a b [a]");
        assert_eq!(next("a b [a]", false), "[a] b a", "wraps to the start");
        assert_eq!(
            next("[Foo] foo Foo", false),
            "Foo foo [Foo]",
            "case-sensitive"
        );
        assert_eq!(next("[foo] foobar foo", true), "foo foobar [foo]");
        assert_eq!(next("[foo] foobar foo", false), "foo [foo]bar foo");
        assert_eq!(next("[x] y", false), "(none)");
        assert_eq!(next("[a\nb] a\nb", false), "a\nb [a\nb]");
    }

    #[test]
    fn comments_and_uncomments_lines() {
        assert_eq!(comment("fn a() {}|\n", SLASHES), "// fn a() {}|\n");
        assert_eq!(comment("|fn a() {}\n", SLASHES), "// |fn a() {}\n");
        assert_eq!(comment("// fn a() {}|\n", SLASHES), "fn a() {}|\n");
        // Without a space after the token, only the token goes.
        assert_eq!(comment("//x|\n", SLASHES), "x|\n");
        assert_eq!(
            comment("[a\nb\n]c\n", Comment::Line("#")),
            "[# a\n# b\n]c\n",
            "a selection ending at a line start leaves that line alone"
        );
        assert_eq!(comment("[# a\n# b]\n", Comment::Line("#")), "[a\nb]\n");
        assert_eq!(comment("se|lect 1;", Comment::Line("--")), "-- se|lect 1;");
    }

    #[test]
    fn mixed_lines_get_commented() {
        assert_eq!(comment("[// a\nb]\n", SLASHES), "[// // a\n// b]\n");
    }

    #[test]
    fn blank_lines_are_skipped_unless_all_are_blank() {
        assert_eq!(
            comment("[  a\n\n   \n  b]\n", SLASHES),
            "[  // a\n\n   \n  // b]\n"
        );
        assert_eq!(comment("[// a\n\n// b]", SLASHES), "[a\n\nb]");
        assert_eq!(comment("x\n|\ny", SLASHES), "x\n// |\ny");
        assert_eq!(comment("|", SLASHES), "// |");
    }

    #[test]
    fn inserts_at_the_smallest_indentation() {
        assert_eq!(
            comment("[    a\n  b\n      c]", SLASHES),
            "[  //   a\n  // b\n  //     c]"
        );
        // A tab is four columns here, so the tab line and the four-space line line up.
        assert_eq!(
            comment("[\ta\n    b\n        c]", SLASHES),
            "[\t// a\n    // b\n    //     c]"
        );
        assert_eq!(comment("[\t// a\n  // b]", SLASHES), "[\ta\n  b]");
    }

    #[test]
    fn keeps_crlf() {
        assert_eq!(
            comment("[a\r\nb]\r\n", Comment::Line("#")),
            "[# a\r\n# b]\r\n"
        );
        assert_eq!(
            comment("[# a\r\n# b]\r\n", Comment::Line("#")),
            "[a\r\nb]\r\n"
        );
    }

    #[test]
    fn block_comment_wraps_the_lines() {
        assert_eq!(comment("  <p>|hi</p>\n", HTML), "  <!-- <p>|hi</p> -->\n");
        assert_eq!(
            comment("[<a>\n  <b>]\n", HTML),
            "<!-- [<a>\n  <b>] -->\n",
            "the selection stays on the text, not the markers"
        );
        assert_eq!(comment("  <!-- <p>|hi</p> -->\n", HTML), "  <p>|hi</p>\n");
        assert_eq!(comment("[<!--<a>\n<b>-->]", HTML), "[<a>\n<b>]");
        assert_eq!(comment("<!-- | -->", HTML), "|");
        assert_eq!(comment("|", HTML), "<!-- | -->");
        assert_eq!(
            comment("a { color: red; }|", Comment::Block("/*", "*/")),
            "/* a { color: red; }| */"
        );
    }
}
