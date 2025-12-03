// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

use crate::{cell::Cell, response::InsertResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrigin {
    HardBreak,
    SoftWrap,
    ScrollFill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowJoin {
    NewLogicalLine,
    ContinueLogicalLine,
}

#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    width: usize,
    pub origin: RowOrigin,
    pub join: RowJoin,
}

impl Row {
    #[must_use]
    pub const fn new(width: usize) -> Self {
        Self {
            cells: Vec::new(),
            width,
            origin: RowOrigin::ScrollFill,
            join: RowJoin::NewLogicalLine,
        }
    }

    #[must_use]
    pub const fn new_with_origin(width: usize, origin: RowOrigin, join: RowJoin) -> Self {
        Self {
            cells: Vec::new(),
            width,
            origin,
            join,
        }
    }

    #[must_use]
    pub const fn from_cells(
        width: usize,
        origin: RowOrigin,
        join: RowJoin,
        cells: Vec<Cell>,
    ) -> Self {
        Self {
            cells,
            width,
            origin,
            join,
        }
    }

    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Logical row width (number of *columns*), not number of occupied cells.
    #[must_use]
    pub const fn max_width(&self) -> usize {
        self.width
    }

    /// Update the logical width of this row (number of columns).
    /// This does *not* change the existing cells, only the max width metadata.
    pub const fn set_max_width(&mut self, new_width: usize) {
        self.width = new_width;
    }

    /// How many cells are currently occupied.
    #[must_use]
    pub fn get_row_width(&self) -> usize {
        let mut cols = 0;
        let mut idx = 0;

        while idx < self.cells.len() {
            let cell = &self.cells[idx];
            if cell.is_head() {
                cols += cell.display_width();
                idx += cell.display_width();
            } else {
                // Continuations should always follow heads,
                // but if encountered, advance by 1 cell.
                idx += 1;
            }
        }

        cols
    }

    #[must_use]
    pub fn get_char_at(&self, idx: usize) -> Option<&Cell> {
        self.cells.get(idx)
    }

    /// Return the real cell if present, otherwise an implicit blank.
    #[must_use]
    pub fn resolve_cell(&self, col: usize) -> Cell {
        if col < self.cells.len() {
            self.cells[col].clone()
        } else {
            Cell::blank_with_tag(FormatTag::default())
        }
    }

    #[must_use]
    pub const fn get_characters(&self) -> &Vec<Cell> {
        &self.cells
    }

    /// Clean up when overwriting wide cells:
    /// - If overwriting a continuation, clear the head + all its continuations.
    /// - If overwriting a head, clear its continuations.
    fn cleanup_wide_overwrite(&mut self, col: usize) {
        if col >= self.cells.len() {
            return;
        }

        // Overwriting a continuation: clean up head + all continuations.
        if self.cells[col].is_continuation() {
            if col == 0 {
                // Invariant violation; nothing to the left
                return;
            }
            // find head to the left
            let mut head = col - 1;
            while head > 0 && !self.cells[head].is_head() {
                head -= 1;
            }
            if !self.cells[head].is_head() {
                return;
            }

            // clear head + all following continuations
            let mut idx = head;
            while idx < self.cells.len() && self.cells[idx].is_continuation() || idx == head {
                self.cells[idx] = Cell::new(TChar::Space, FormatTag::default());
                idx += 1;
                if idx >= self.cells.len() {
                    break;
                }
            }
            return;
        }

        // Overwriting a head: clear trailing continuations
        if self.cells[col].is_head() {
            let mut idx = col + 1;
            while idx < self.cells.len() && self.cells[idx].is_continuation() {
                self.cells[idx] = Cell::new(TChar::Space, FormatTag::default());
                idx += 1;
            }
        }
    }

    pub fn insert_text(
        &mut self,
        start_col: usize,
        text: &[TChar],
        tag: &FormatTag,
    ) -> InsertResponse {
        let mut col = start_col;

        // ---------------------------------------------------------------
        // If we start at or beyond the logical width, this row is full.
        // Caller must wrap the entire input to the next row.
        // ---------------------------------------------------------------
        if col >= self.width {
            return InsertResponse::Leftover {
                data: text.to_vec(),
                final_col: col, // typically == self.width
            };
        }

        // ---------------------------------------------------------------
        // Walk each character and try to insert it.
        // ---------------------------------------------------------------
        for (i, tchar) in text.iter().enumerate() {
            let w = tchar.display_width().max(1);

            // If we've reached the row's width, nothing else fits here.
            if col >= self.width {
                return InsertResponse::Leftover {
                    data: text[i..].to_vec(),
                    final_col: col,
                };
            }

            // If this glyph would overflow, stop here and wrap remaining text.
            if col + w > self.width {
                return InsertResponse::Leftover {
                    data: text[i..].to_vec(),
                    final_col: col,
                };
            }

            // -----------------------------------------------------------
            // Pad up to current column with blanks if there's a gap.
            // -----------------------------------------------------------
            if col > self.cells.len() {
                let pad = col - self.cells.len();
                for _ in 0..pad {
                    self.cells.push(Cell::new(TChar::Space, tag.clone()));
                }
            }

            // -----------------------------------------------------------
            // If we're overwriting, clean up any wide-glyph debris.
            // -----------------------------------------------------------
            if col < self.cells.len() {
                self.cleanup_wide_overwrite(col);
            }

            // -----------------------------------------------------------
            // Ensure we have enough storage for head + continuations,
            // but never grow beyond self.width.
            // -----------------------------------------------------------
            let target_len = (col + w).min(self.width);
            if self.cells.len() < target_len {
                self.cells
                    .resize(target_len, Cell::new(TChar::Space, tag.clone()));
            }

            // After resize, col must be within bounds; double-check defensively.
            if col >= self.cells.len() {
                return InsertResponse::Leftover {
                    data: text[i..].to_vec(),
                    final_col: col,
                };
            }

            // -----------------------------------------------------------
            // Insert head cell
            // -----------------------------------------------------------
            self.cells[col] = Cell::new(tchar.clone(), tag.clone());

            // -----------------------------------------------------------
            // Insert continuation cells within bounds
            // -----------------------------------------------------------
            for offset in 1..w {
                let idx = col + offset;
                if idx >= self.width || idx >= self.cells.len() {
                    break;
                }
                self.cells[idx] = Cell::wide_continuation();
            }

            // Move column forward by glyph width, but never beyond width
            col += w;
            if col > self.width {
                col = self.width;
            }
        }

        // ---------------------------------------------------------------
        // All text successfully inserted on this row.
        // ---------------------------------------------------------------
        InsertResponse::Consumed(col)
    }

    /// Insert `n` spaces starting at `col`, shifting existing cells right.
    /// This implements VT ICH (Insert Character).
    pub fn insert_spaces_at(&mut self, col: usize, n: usize, tag: &FormatTag) {
        let width = self.width;

        if n == 0 || col >= width {
            return;
        }

        // How many blanks can actually be inserted within the logical row width?
        let insert_len = n.min(width.saturating_sub(col));

        // Current number of stored cells (may be < width).
        let old_len = self.cells.len();

        // We need enough capacity to:
        //  - hold all existing cells, shifted by insert_len
        //  - plus any new blank cells starting at `col`
        //
        // NOTE: There might be an implicit gap between old_len and `col`,
        // which represents default-blank cells; we handle that by creating
        // default blanks in the resized vector.
        let needed_len = (old_len + insert_len).max(col + insert_len);

        if needed_len == 0 {
            return;
        }

        // Resize with default blank cells; many of these will be overwritten.
        self.cells
            .resize(needed_len, Cell::blank_with_tag(FormatTag::default()));

        // Shift existing cells [col..old_len) to the right by insert_len.
        // Anything whose destination is >= width "falls off" to the right.
        for i in (col..old_len).rev() {
            let dest = i + insert_len;
            if dest < width {
                self.cells[dest] = self.cells[i].clone();
            }
            // if dest >= width, the cell is discarded (clamped off the row)
        }

        // Fill the gap [col..col+insert_len) with blanks using the current tag.
        for i in col..(col + insert_len) {
            if i < width {
                self.cells[i] = Cell::blank_with_tag(tag.clone());
            }
        }

        // Finally, clamp physical storage so we don't have cells beyond logical width.
        if self.cells.len() > width {
            self.cells.truncate(width);
        }

        // Maintain sparse-row invariant by trimming trailing default blanks
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag() == &FormatTag::default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }
}
