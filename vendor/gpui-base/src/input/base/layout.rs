use std::{ops::Range, rc::Rc};

use gpui::{Bounds, Half, Pixels, ShapedLine, TextAlign, px};

use super::{WrappingIndent, display_map::LineLayout};

#[derive(Clone, Default)]
pub(crate) struct WhitespaceIndicators {
    pub(crate) space: ShapedLine,
    pub(crate) tab: ShapedLine,
}

#[derive(Clone)]
pub(super) struct LastLayout {
    // ZJ patch: before-line controls use separate content-space positions and text anchors.
    pub(super) document_revision: u64,
    pub(super) visible_range: Range<usize>,
    pub(super) visible_buffer_lines: Vec<usize>,
    pub(super) visible_line_byte_offsets: Vec<usize>,
    pub(super) visible_top: Pixels,
    pub(super) visible_line_y: Vec<Pixels>,
    pub(super) height_projection: HeightProjection,
    pub(super) visible_range_offset: Range<usize>,
    pub(super) lines: Rc<Vec<LineLayout>>,
    pub(super) line_height: Pixels,
    pub(super) wrap_width: Option<Pixels>,
    pub(super) wrapping_indent: WrappingIndent,
    pub(super) line_number_width: Pixels,
    /// Width of one space in the editor font.
    ///
    /// Past the end of a line there are no glyphs to hit-test against, so this is the
    /// step used to measure how far past the end a pointer sits.
    pub(super) space_width: Pixels,
    pub(super) cursor_bounds: Option<Bounds<Pixels>>,
    pub(super) text_align: TextAlign,
    pub(super) content_width: Pixels,
}

impl LastLayout {
    pub(crate) fn line(&self, row: usize) -> Option<&LineLayout> {
        let pos = self.visible_buffer_lines.binary_search(&row).ok()?;
        self.lines.get(pos)
    }

    pub(super) fn alignment_offset(&self, line_width: Pixels) -> Pixels {
        match self.text_align {
            TextAlign::Left => px(0.),
            TextAlign::Center => (self.content_width - line_width).half().max(px(0.)),
            TextAlign::Right => (self.content_width - line_width).max(px(0.)),
        }
    }
}

/// ZJ patch: maps pure display rows into content space without changing text layout coordinates.
#[derive(Clone, Default, PartialEq)]
pub(super) struct HeightProjection {
    // (display row, widget index, cumulative slot height).
    pub(super) slots: Vec<(usize, usize, Pixels)>,
}

impl HeightProjection {
    pub(super) fn new<M: super::InputModeKind>(state: &super::InputBaseState<M>) -> Self {
        use super::{InputExtras as _, RopeExt as _};
        let mut slots = Vec::new();
        let mut height = px(0.);
        for (index, widget) in state.extras.before_line_widgets().iter().enumerate() {
            let line = state
                .text
                .offset_to_point(widget.anchor_byte.min(state.text.len()))
                .row;
            if let Some(rows) = state.display_map.buffer_line_to_display_row_range(line) {
                height += widget.height;
                slots.push((rows.start, index, height));
            }
        }
        Self { slots }
    }

    pub(super) fn row_y(&self, row: usize, line_height: Pixels) -> Pixels {
        let count = self.slots.partition_point(|slot| slot.0 <= row);
        line_height * row + count.checked_sub(1).map_or(px(0.), |ix| self.slots[ix].2)
    }

    pub(super) fn total_height(&self) -> Pixels {
        self.slots.last().map_or(px(0.), |slot| slot.2)
    }

    pub(super) fn row_at_y(&self, y: Pixels, line_height: Pixels, rows: usize) -> usize {
        // Include a line even when only its preceding control is visible.
        let mut low = 0;
        let mut high = rows;
        while low < high {
            let middle = (low + high) / 2;
            if self.row_y(middle, line_height) + line_height <= y {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        low.min(rows.saturating_sub(1))
    }

    pub(super) fn slot_at_y(&self, y: Pixels, line_height: Pixels) -> Option<usize> {
        self.slots.iter().enumerate().find_map(|(ix, slot)| {
            let end = line_height * slot.0 + slot.2;
            let start =
                line_height * slot.0 + ix.checked_sub(1).map_or(px(0.), |ix| self.slots[ix].2);
            (y >= start && y < end).then_some(slot.1)
        })
    }
}

impl HeightProjection {
    pub(super) fn contains_slot(&self, y: Pixels, line_height: Pixels) -> bool {
        self.slot_at_y(y, line_height).is_some()
    }
}
