//! Pair unified patch lines once, off the UI thread. Both panes use one virtual list.
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug)]
pub struct Cell {
    pub number: usize,
    pub text: Range<usize>,
    pub changed: bool,
}
#[derive(Clone, Debug)]
pub enum Row {
    Hunk(Range<usize>),
    Lines {
        old: Option<Cell>,
        new: Option<Cell>,
    },
}
#[derive(Clone, Debug)]
pub struct DiffModel {
    pub patch: Arc<str>,
    pub rows: Vec<Row>,
    pub columns: usize,
}

impl DiffModel {
    /// Combined/conflict and binary patches retain the native Git text fallback.
    pub fn parse(patch: Arc<str>) -> Option<Self> {
        let mut rows = Vec::new();
        let (mut old, mut new) = (0, 0);
        let mut in_hunk = false;
        let mut removed = Vec::new();
        let mut added = Vec::new();
        fn flush(rows: &mut Vec<Row>, removed: &mut Vec<Cell>, added: &mut Vec<Cell>) {
            let mut left = removed.drain(..);
            let mut right = added.drain(..);
            loop {
                let (old, new) = (left.next(), right.next());
                if old.is_none() && new.is_none() {
                    break;
                }
                rows.push(Row::Lines { old, new });
            }
        }
        let mut offset = 0;
        let mut columns = 0;
        for line in patch.split_inclusive('\n') {
            let len = line.trim_end_matches(['\n', '\r']).len();
            if len > 262_144 || rows.len() + added.len() + removed.len() > 100_000 {
                return None;
            }
            let start = offset;
            offset += line.len();
            let line = &patch[start..start + len];
            columns = columns.max(line.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum());
            if line.starts_with("@@@")
                || line.starts_with("Binary files ")
                || line.starts_with("GIT binary patch")
            {
                return None;
            }
            if line.starts_with("@@ ") {
                flush(&mut rows, &mut removed, &mut added);
                let mut parts = line.split_whitespace();
                parts.next()?;
                let position = |s: &str, prefix| {
                    s.strip_prefix(prefix)?
                        .split(',')
                        .next()?
                        .parse::<usize>()
                        .ok()
                };
                old = position(parts.next()?, '-')?;
                new = position(parts.next()?, '+')?;
                rows.push(Row::Hunk(start..start + len));
                in_hunk = true;
            } else if in_hunk {
                let range = start + 1..start + len;
                match line.as_bytes().first() {
                    Some(b'-') => {
                        removed.push(Cell {
                            number: old,
                            text: range,
                            changed: true,
                        });
                        old += 1;
                    }
                    Some(b'+') => {
                        added.push(Cell {
                            number: new,
                            text: range,
                            changed: true,
                        });
                        new += 1;
                    }
                    Some(b' ') => {
                        flush(&mut rows, &mut removed, &mut added);
                        rows.push(Row::Lines {
                            old: Some(Cell {
                                number: old,
                                text: range.clone(),
                                changed: false,
                            }),
                            new: Some(Cell {
                                number: new,
                                text: range,
                                changed: false,
                            }),
                        });
                        old += 1;
                        new += 1;
                    }
                    Some(b'\\') => {} // Git's missing-final-newline marker remains in inline view.
                    _ => in_hunk = false,
                }
            }
        }
        flush(&mut rows, &mut removed, &mut added);
        if rows.is_empty() {
            None
        } else {
            Some(Self {
                patch,
                rows,
                columns,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aligns_replacement_and_unequal_additions_with_original_numbers() {
        let model = DiffModel::parse("--- a/a\n+++ b/a\n@@ -4,3 +4,4 @@\n keep\n-old\n+new\n+extra\n tail\n\\ No newline at end of file\n".into()).unwrap();
        assert_eq!(model.rows.len(), 5);
        let Row::Lines {
            old: Some(old),
            new: Some(new),
        } = &model.rows[2]
        else {
            panic!()
        };
        assert_eq!((old.number, new.number), (5, 5));
        assert_eq!(&model.patch[old.text.clone()], "old");
        let Row::Lines {
            old: None,
            new: Some(new),
        } = &model.rows[3]
        else {
            panic!()
        };
        assert_eq!(new.number, 6);
        assert!(DiffModel::parse("Binary files a and b differ\n".into()).is_none());
        assert!(DiffModel::parse("@@@ -1,1 -1,1 +1,1 @@@\n".into()).is_none());
    }
}
