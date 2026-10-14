//! Version-aware navigation in the virtualized Diff editor.
use super::diff_ops::DiffSelection;
use super::diff_view::DiffList;
use super::navigation::{self, Target};
use super::*;
use crate::diff_doc::{DiffDoc, LineKind, Side};
use crate::partial_patch::RawPatch;
use std::sync::Weak;

/// A byte column in the displayed text, independent of inline / side-by-side layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Caret {
    pub old: bool,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug)]
pub(super) struct Origin {
    generation: u64,
    doc: Weak<DiffDoc>,
    old: bool,
}

impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation && self.old == other.old && self.doc.ptr_eq(&other.doc)
    }
}

impl Origin {
    pub(super) fn valid(&self, this: &Workbench) -> bool {
        self.generation == this.diff.generation && self.valid_history(this)
    }

    pub(super) fn valid_history(&self, this: &Workbench) -> bool {
        this.diff
            .doc
            .as_ref()
            .is_some_and(|doc| self.doc.ptr_eq(&Arc::downgrade(doc)))
    }
}

pub(super) fn row_caret(doc: &DiffDoc, list: DiffList, row: usize) -> Option<Caret> {
    let (old, line) = match list {
        DiffList::Original => (true, doc.rows.get(row)?.old?),
        DiffList::Modified => (false, doc.rows.get(row)?.new?),
        DiffList::Inline => {
            let row = doc.inline.get(row)?;
            match row.kind {
                LineKind::Removed => (true, row.line),
                LineKind::Added => (false, row.line),
                LineKind::Same => row.other.map_or((true, row.line), |line| (false, line)),
            }
        }
    };
    Some(Caret {
        old,
        line: line as usize,
        column: 0,
    })
}

/// Convert an expanded-tab display column back to an original UTF-8 byte offset.
fn source_column(text: &str, column: usize) -> usize {
    let mut displayed = 0;
    for (offset, ch) in text.char_indices() {
        let width = if ch == '\t' { 4 } else { ch.len_utf8() };
        if displayed + width > column {
            return offset;
        }
        displayed += width;
    }
    text.len()
}

fn display_column(text: &str, column: usize) -> usize {
    text.char_indices()
        .take_while(|(offset, _)| *offset < column)
        .map(|(_, ch)| if ch == '\t' { 4 } else { ch.len_utf8() })
        .sum()
}

/// Rebuild only for a navigation request; expanded tabs must not change Python scope.
fn source(side: &Side, raw: &RawPatch, caret: Caret) -> Option<(String, usize)> {
    let mut source = String::with_capacity(side.text.len());
    let mut offset = None;
    for (index, line) in side.lines.iter().enumerate() {
        if line.number as usize != index + 1 {
            return None;
        }
        let text = raw.text(caret.old, line.number)?;
        if index == caret.line {
            offset = Some(source.len() + source_column(text, caret.column));
        }
        source.push_str(text);
        source.push('\n');
    }
    Some((source, offset?))
}

impl Workbench {
    pub(super) fn diff_origin(&self, old: bool) -> Option<Origin> {
        Some(Origin {
            generation: self.diff.generation,
            doc: Arc::downgrade(self.diff.doc.as_ref()?),
            old,
        })
    }

    pub(super) fn diff_navigation_caret(&self) -> Option<Caret> {
        self.diff.caret.or_else(|| {
            let selection = self.diff.selection?;
            row_caret(self.diff.doc.as_ref()?, selection.list, selection.head)
        })
    }

    /// Original text for one navigation request, without parsing a second full patch.
    fn diff_original_line(&self, old: bool, index: usize) -> Option<&str> {
        let doc = self.diff.doc.as_ref()?;
        let side = if old { &doc.old } else { &doc.new };
        let line = side.lines.get(index)?;
        if let Some(raw) = &self.diff.raw {
            return raw.text(old, line.number);
        }
        if let Some((patch, _)) = &self.diff.source {
            return patch
                .split_terminator('\n')
                .skip_while(|line| !line.starts_with("@@ "))
                .filter_map(|line| {
                    let prefix = *line.as_bytes().first()?;
                    (prefix == b' ' || prefix == if old { b'-' } else { b'+' })
                        .then(|| line[1..].strip_suffix('\r').unwrap_or(&line[1..]))
                })
                .nth(index);
        }
        Some(side.line_text(index))
    }

    pub(super) fn diff_line_info(&self) -> Option<(u32, u32, u32)> {
        let doc = self.diff.doc.as_ref()?;
        let caret = self.diff_navigation_caret();
        let old = caret.map_or(doc.new.lines.is_empty(), |caret| caret.old);
        let side = if old { &doc.old } else { &doc.new };
        let (line, column) = caret
            .and_then(|caret| {
                let line = side.lines.get(caret.line)?;
                let text = self.diff_original_line(old, caret.line)?;
                let offset = source_column(text, caret.column);
                let column = text[..offset].chars().count();
                Some((line.number - 1, column as u32))
            })
            .unwrap_or((0, 0));
        Some((side.lines.last()?.number, line, column))
    }

    pub(super) fn diff_go_to_line(
        &mut self,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = &self.diff.doc else { return };
        let old = self
            .diff_navigation_caret()
            .map_or(doc.new.lines.is_empty(), |caret| caret.old);
        let side = if old { &doc.old } else { &doc.new };
        let Ok(index) = side
            .lines
            .binary_search_by_key(&(line + 1), |line| line.number)
        else {
            return;
        };
        let Some(text) = self.diff_original_line(old, index) else {
            return;
        };
        let column = text
            .char_indices()
            .nth(column as usize)
            .map_or(text.len(), |(offset, _)| offset);
        let column = display_column(text, column);
        self.remember(cx);
        self.diff_place(
            Caret {
                old,
                line: index,
                column,
            },
            window,
            cx,
        );
    }

    pub(super) fn diff_place(&mut self, caret: Caret, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = &self.diff.doc else { return };
        let inline = self.diff_is_inline();
        let row = if inline {
            doc.inline.iter().position(|row| {
                if caret.old {
                    row.kind != LineKind::Added && row.line as usize == caret.line
                } else {
                    match row.kind {
                        LineKind::Same => row.other.is_some_and(|line| line as usize == caret.line),
                        LineKind::Added => row.line as usize == caret.line,
                        LineKind::Removed => false,
                    }
                }
            })
        } else {
            doc.rows.iter().position(|row| {
                (if caret.old { row.old } else { row.new })
                    .is_some_and(|line| line as usize == caret.line)
            })
        };
        let Some(row) = row else { return };
        let side = if caret.old { &doc.old } else { &doc.new };
        let Some(text) = side
            .lines
            .get(caret.line)
            .map(|_| side.line_text(caret.line))
        else {
            return;
        };
        let prefix = text.get(..caret.column).unwrap_or(text);
        let theme = gpui_kit::component::Theme::global(cx);
        let run = TextRun {
            len: prefix.len(),
            font: Font {
                family: theme.mono_font_family.clone(),
                ..Default::default()
            },
            color: theme.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let text_x = window
            .text_system()
            .shape_line(prefix.to_owned().into(), theme.mono_font_size, &[run], None)
            .width;
        let gutter = theme::DIFF_GUTTER * if inline { 2. } else { 1. } + theme::DIFF_INDICATOR;
        {
            let handle = &self.diff.scroll.0.borrow().base_handle;
            let offset = handle.offset();
            let width = handle.bounds().size.width;
            if width > theme::DIFF_TEXT_END {
                let left = -offset.x;
                let target = gutter + text_x;
                let next = if target < left + theme::DIFF_TEXT_END {
                    (target - gutter).max(Pixels::ZERO)
                } else if target > left + width - theme::DIFF_TEXT_END {
                    (target - width + theme::DIFF_TEXT_END).max(Pixels::ZERO)
                } else {
                    left
                };
                handle.set_offset(point(-next.min(handle.max_offset().x), offset.y));
            }
        }
        self.diff.caret = Some(caret);
        self.diff.selection = Some(DiffSelection {
            list: if inline {
                DiffList::Inline
            } else if caret.old {
                DiffList::Original
            } else {
                DiffList::Modified
            },
            anchor: row,
            head: row,
        });
        self.diff.dragging = false;
        self.diff
            .scroll
            .scroll_to_item_strict(row, ScrollStrategy::Center);
        if self.active != Pane::Diff {
            self.select_pane(Pane::Diff, window, cx);
        }
        self.diff.focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn diff_follow_target(
        &mut self,
        origin: Origin,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !origin.valid(self) {
            return;
        }
        if self
            .diff
            .tab
            .as_ref()
            .is_some_and(|tab| tab.path == target.path)
        {
            let doc = self.diff.doc.as_ref().unwrap();
            let side = if origin.old { &doc.old } else { &doc.new };
            let Ok(line) = side
                .lines
                .binary_search_by_key(&(target.line + 1), |line| line.number)
            else {
                return;
            };
            self.remember(cx);
            self.diff_place(
                Caret {
                    old: origin.old,
                    line,
                    column: target.column as usize,
                },
                window,
                cx,
            );
        } else {
            self.jump_to(target, true, window, cx);
        }
    }

    pub(super) fn diff_go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(caret), Some(doc), Some(tab)) = (
            self.diff.caret,
            self.diff.doc.clone(),
            self.diff.tab.as_ref(),
        ) else {
            return;
        };
        if self.diff.loading || self.diff.stale {
            return;
        }
        let path = tab.path.clone();
        let language_path = if caret.old {
            tab.request()
                .and_then(|request| match &request.operation {
                    workspace_editor_git::Operation::Diff { original_path, .. }
                    | workspace_editor_git::Operation::CommitDiff { original_path, .. } => {
                        original_path.as_deref()
                    }
                    _ => None,
                })
                .unwrap_or(&path)
        } else {
            &path
        };
        let Some(language) = crate::symbol_index::language(language_path) else {
            return;
        };
        let Some(origin) = self.diff_origin(caret.old) else {
            return;
        };
        let raw = self.diff.raw.clone();
        let patch = self.diff.source.as_ref().map(|(patch, _)| patch.clone());
        let indexer = navigation::indexing_view(cx.entity(), &path, self.root.as_deref(), cx);
        let index = if indexer.entity_id() == cx.entity_id() {
            self.symbol_index(cx)
        } else {
            indexer.update(cx, |this, cx| this.symbol_index(cx))
        };
        let indexer = indexer.downgrade();
        self.nav.generation += 1;
        let request = self.nav.generation;
        self.message = "正在查找定义…".into();
        self.nav.task = Some(cx.spawn_in(window, async move |this, cx| {
            let mut index = index;
            let deadline = std::time::Instant::now() + navigation::INDEX_WAIT;
            while index.is_none() && std::time::Instant::now() < deadline {
                let Ok((now, building)) = indexer.update(cx, |this, _| {
                    (this.nav.symbols.clone(), this.symbol_index_building())
                }) else {
                    return;
                };
                index = now;
                if index.is_some() || !building {
                    break;
                }
                cx.background_executor().timer(navigation::INDEX_POLL).await;
            }
            let old = caret.old;
            let targets = cx
                .background_spawn(async move {
                    let parsed;
                    let raw = if let Some(raw) = raw.as_deref() {
                        raw
                    } else {
                        parsed = crate::partial_patch::parse(patch.as_deref()?);
                        parsed.as_ref()?
                    };
                    let side = if old { &doc.old } else { &doc.new };
                    let (source, offset) = source(side, raw, caret)?;
                    let mut targets =
                        navigation::resolve(&path, language, &source, offset, index.as_deref());
                    for target in &mut targets {
                        if target.path == path {
                            target.column = display_column(
                                raw.text(old, target.line + 1)?,
                                target.column as usize,
                            ) as u32;
                        }
                    }
                    Some(targets)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.nav.generation != request {
                    return;
                }
                if this.active != Pane::Diff || !origin.valid(this) {
                    if this.message == "正在查找定义…" {
                        this.message.clear();
                        cx.notify();
                    }
                    return;
                }
                match targets {
                    Some(targets) if targets.len() == 1 => {
                        this.message.clear();
                        this.diff_follow_target(
                            origin,
                            targets.into_iter().next().unwrap(),
                            window,
                            cx,
                        );
                    }
                    Some(targets) if !targets.is_empty() => {
                        this.show_locations(targets, "定义", Some(origin), window, cx);
                    }
                    Some(_) => this.message = "未找到定义".into(),
                    None => this.message = "当前 Diff 缺少完整源码，无法转到定义".into(),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use core::prelude::v1::test;

    #[test]
    fn display_columns_preserve_tabs_unicode_and_character_boundaries() {
        let text = "\t图helper(1)";
        for (displayed, original) in [
            (0, 0),
            (3, 0),
            (4, 1),
            (5, 1),
            (7, 4),
            (8, 5),
            (100, text.len()),
        ] {
            assert_eq!(source_column(text, displayed), original);
        }
        assert_eq!(display_column(text, 4), 7);
        assert_eq!(display_column(text, text.len()), 16);
    }

    #[test]
    fn definition_source_keeps_original_indentation_and_rejects_partial_context() {
        let patch = "--- a/a.py\n+++ b/a.py\n@@ -1,3 +1,3 @@\n def helper():\n-\treturn 0\r\n+\treturn 1\r\n \t图helper()\n";
        let raw = crate::partial_patch::parse(patch).unwrap();
        let highlight = crate::theme::highlight_theme_for_tests(false);
        let doc = DiffDoc::parse(
            patch,
            Some("python"),
            &highlight,
            crate::diff_doc::ChangeColors {
                inserted_text: rgb(crate::theme::LIGHT.diff_green).into(),
                removed_text: rgb(crate::theme::LIGHT.diff_red).into(),
            },
        )
        .unwrap();
        let caret = Caret {
            old: false,
            line: 2,
            column: 7,
        };
        let (text, offset) = source(&doc.new, &raw, caret).unwrap();
        assert_eq!(text, "def helper():\n\treturn 1\n\t图helper()\n");
        assert_eq!(&text[offset..], "helper()\n");
        let partial = patch.replace("@@ -1,3 +1,3 @@", "@@ -5,3 +5,3 @@");
        let raw = crate::partial_patch::parse(&partial).unwrap();
        let doc = DiffDoc::parse(
            &partial,
            None,
            &highlight,
            crate::diff_doc::ChangeColors {
                inserted_text: rgb(crate::theme::LIGHT.diff_green).into(),
                removed_text: rgb(crate::theme::LIGHT.diff_red).into(),
            },
        )
        .unwrap();
        assert!(source(&doc.new, &raw, caret).is_none());
    }
}
