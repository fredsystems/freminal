// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

use crate::{cell::Cell, response::InsertResponse};

/// Indicates whether a row was produced by a hard line break, a soft wrap, or as
/// a blank scroll-fill placeholder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowOrigin {
    /// The row begins a new logical line (e.g. from a newline character or initial content).
    HardBreak,
    /// The row is a continuation produced by soft-wrapping a long logical line.
    SoftWrap,
    /// The row is a blank placeholder created to fill newly visible screen space during scrolling.
    ScrollFill,
}

/// Indicates how a row connects to the next row in a multi-row logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowJoin {
    /// This row starts a new logical line; the previous logical line ends here.
    NewLogicalLine,
    /// This row is a soft-wrap continuation of the preceding logical line.
    ContinueLogicalLine,
}

/// A single row of terminal cells with a fixed logical width.
///
/// Cells are stored sparsely: trailing default-blank cells are not allocated.
/// The `origin` and `join` fields record how this row relates to the logical
/// line structure, which is used during reflow when the terminal is resized.
/// The `dirty` flag tracks whether the row's cached flat representation is stale.
#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    width: usize,
    pub origin: RowOrigin,
    pub join: RowJoin,
    pub dirty: bool,
}

impl Row {
    /// Create a new empty row with the given logical width, marked as a `ScrollFill` placeholder.
    #[must_use]
    pub const fn new(width: usize) -> Self {
        Self {
            cells: Vec::new(),
            width,
            origin: RowOrigin::ScrollFill,
            join: RowJoin::NewLogicalLine,
            dirty: true,
        }
    }

    /// Create a new empty row with the given width, origin, and join metadata.
    #[must_use]
    pub const fn new_with_origin(width: usize, origin: RowOrigin, join: RowJoin) -> Self {
        Self {
            cells: Vec::new(),
            width,
            origin,
            join,
            dirty: true,
        }
    }

    /// Create a row with the given width, origin, join, and pre-populated cells.
    ///
    /// Used by `Buffer::reflow_to_width` to install re-wrapped rows directly.
    /// The new row is marked dirty because it has never been snapshotted.
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
            dirty: true,
        }
    }

    /// Clear all cells in this row, leaving it empty (sparse).
    pub fn clear(&mut self) {
        self.dirty = true;
        self.cells.clear();
    }

    /// Fill this row with blank cells carrying the given format tag (BCE).
    ///
    /// If the tag is visually default, the row is left sparse (no-op on an
    /// already-empty row, or clears an existing one).  Otherwise, explicit
    /// blank cells are written so the renderer picks up the correct colors.
    pub fn fill_with_tag(&mut self, tag: &FormatTag) {
        if tag.is_visually_default() {
            return;
        }
        self.dirty = true;
        self.cells.clear();
        self.cells
            .resize(self.width, Cell::blank_with_tag(tag.clone()));
    }

    /// Mark this row as clean (its flat representation is up-to-date in the cache).
    /// Called by the snapshot machinery after producing a cached flat representation.
    pub const fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Count the number of cells in this row that carry an image placement.
    ///
    /// Used by `Buffer` to maintain its `image_cell_count` counter when rows
    /// are cleared or drained.
    #[must_use]
    pub fn count_image_cells(&self) -> usize {
        self.cells.iter().filter(|c| c.has_image()).count()
    }

    /// Count image cells in columns `[from..to)`.
    ///
    /// Columns beyond the stored cell count are treated as blank (no image).
    #[must_use]
    pub fn count_image_cells_in_range(&self, from: usize, to: usize) -> usize {
        let start = from.min(self.cells.len());
        let end = to.min(self.cells.len());
        if start >= end {
            return 0;
        }
        self.cells[start..end]
            .iter()
            .filter(|c| c.has_image())
            .count()
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

    /// Returns the cell at the given column index, or `None` if out of bounds.
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

    /// Returns a reference to the backing cell vector.
    ///
    /// Prefer [`Row::cells`] for slice access. This method is retained for
    /// callers that need a `&Vec<Cell>` specifically.
    #[must_use]
    pub const fn get_characters(&self) -> &Vec<Cell> {
        &self.cells
    }

    /// Returns the cells in this row as a slice.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Returns the cells in this row as a mutable slice.
    pub fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.cells
    }

    /// Returns the logical width of this row.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Push a single cell onto the backing store (used internally by column-
    /// selective scroll helpers which need to extend a row without a full clear).
    pub fn cells_mut_push(&mut self, cell: Cell) {
        self.cells.push(cell);
    }

    /// Mark this row as dirty (its cached flat representation is stale).
    pub const fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Clean up when overwriting wide cells:
    /// - If overwriting a continuation, clear the head + all its continuations.
    /// - If overwriting a head, clear its continuations.
    fn cleanup_wide_overwrite(&mut self, col: usize) {
        self.dirty = true;
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

    /// Insert `text` starting at `start_col`, wrapping at `self.width`.
    ///
    /// Returns [`InsertResponse::Consumed`] with the final cursor column if all
    /// characters fit, or [`InsertResponse::Leftover`] with the index into `text`
    /// at which the un-inserted portion begins if the row filled before all text
    /// was consumed.
    pub fn insert_text(
        &mut self,
        start_col: usize,
        text: &[TChar],
        tag: &FormatTag,
    ) -> InsertResponse {
        self.insert_text_with_limit(start_col, text, tag, self.width)
    }

    /// Like `insert_text`, but stops at `right_limit` columns instead of
    /// `self.width`.  Used by `Buffer::insert_text` when DECLRMM is active
    /// to enforce the right margin.  `right_limit` must be ≤ `self.width`.
    pub fn insert_text_with_limit(
        &mut self,
        start_col: usize,
        text: &[TChar],
        tag: &FormatTag,
        right_limit: usize,
    ) -> InsertResponse {
        let limit = right_limit.min(self.width);
        let mut col = start_col;

        // ---------------------------------------------------------------
        // If we start at or beyond the limit, this row is full.
        // Caller must wrap the entire input to the next row.
        // ---------------------------------------------------------------
        if col >= limit {
            return InsertResponse::Leftover {
                leftover_start: 0,
                final_col: col, // typically == limit
            };
        }

        // At least one cell will be written; mark dirty up front.
        self.dirty = true;

        // ---------------------------------------------------------------
        // Walk each character and try to insert it.
        // ---------------------------------------------------------------
        for (i, tchar) in text.iter().enumerate() {
            let w = tchar.display_width().max(1);

            // If we've reached the limit, nothing else fits here.
            if col >= limit {
                return InsertResponse::Leftover {
                    leftover_start: i,
                    final_col: col,
                };
            }

            // If this glyph would overflow the limit, stop here.
            if col + w > limit {
                return InsertResponse::Leftover {
                    leftover_start: i,
                    final_col: col,
                };
            }

            // -----------------------------------------------------------
            // Pad up to current column with blanks if there's a gap.
            // These cells were never explicitly written to, so they must
            // carry the default format rather than the incoming text's tag.
            // -----------------------------------------------------------
            if col > self.cells.len() {
                let pad = col - self.cells.len();
                for _ in 0..pad {
                    self.cells
                        .push(Cell::new(TChar::Space, FormatTag::default()));
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
                    .resize(target_len, Cell::new(TChar::Space, FormatTag::default()));
            }

            // After resize, col must be within bounds; double-check defensively.
            if col >= self.cells.len() {
                return InsertResponse::Leftover {
                    leftover_start: i,
                    final_col: col,
                };
            }

            // -----------------------------------------------------------
            // Insert head cell
            // -----------------------------------------------------------
            self.cells[col] = Cell::new(*tchar, tag.clone());

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

        self.dirty = true;

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

    /// Clear cells from `col` to the end of the row
    pub fn clear_from(&mut self, col: usize, tag: &FormatTag) {
        // BCE: when the tag has a non-default background, we must write explicit
        // blank cells all the way to the row width so the renderer picks up the
        // correct background color.  When the tag is visually default, we only
        // need to clear existing cells and can rely on the sparse representation.
        if !tag.is_visually_default() {
            // Extend the cell vector to the full row width so every column from
            // `col` to the end has an explicit cell carrying the BCE tag.
            if self.cells.len() < self.width {
                self.cells
                    .resize(self.width, Cell::blank_with_tag(FormatTag::default()));
            }
        } else if col >= self.cells.len() {
            return;
        }

        self.dirty = true;
        let end = self.cells.len();
        for i in col..end {
            self.cells[i] = Cell::blank_with_tag(tag.clone());
        }

        // Trim trailing blanks to maintain sparse invariant
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag().is_visually_default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }

    /// Clear cells from the beginning up to (exclusive) `col`.
    ///
    /// Callers that want an inclusive clear (e.g. EL 1 — "erase through cursor")
    /// pass `cursor_x + 1`.
    pub fn clear_to(&mut self, col: usize, tag: &FormatTag) {
        // BCE: when the tag is non-default, extend the cell vector so we can
        // write explicit blank cells for the full erased range.
        let limit = col.min(self.width);
        if !tag.is_visually_default() && self.cells.len() < limit {
            self.cells
                .resize(limit, Cell::blank_with_tag(FormatTag::default()));
        }
        let end = limit.min(self.cells.len());
        if end > 0 {
            self.dirty = true;
        }
        for i in 0..end {
            self.cells[i] = Cell::blank_with_tag(tag.clone());
        }
    }

    /// Clear the entire row with blanks using the given format tag.
    ///
    /// When the tag is visually default, the row is left sparse (empty cell vec)
    /// because implicit blanks already render as default.  When the tag carries
    /// a non-default background or other SGR attributes (BCE), explicit blank
    /// cells are written so the renderer can pick up the correct colors.
    pub fn clear_with_tag(&mut self, tag: &FormatTag) {
        self.dirty = true;
        self.cells.clear();
        if !tag.is_visually_default() {
            self.cells
                .resize(self.width, Cell::blank_with_tag(tag.clone()));
        }
    }

    /// Replace `n` cells starting at `col` with blanks, using `tag` for each blank.
    /// Implements VT ECH (Erase Character).
    ///
    /// - The cursor does not move (caller's responsibility).
    /// - Remaining characters to the right of the erased region are **not** shifted.
    /// - If the range `[col .. col + n]` extends beyond the stored cells, blanks are
    ///   written only up to `min(col + n, self.width)`.
    /// - Wide-glyph cleanup is applied across the entire erased range: any head or
    ///   continuation cell that falls within the range is replaced, and any wide glyph
    ///   that straddles the boundary is fully blanked so no dangling continuations remain.
    pub fn erase_cells_at(&mut self, col: usize, n: usize, tag: &FormatTag) {
        if n == 0 || col >= self.width {
            return;
        }

        self.dirty = true;

        let end = (col + n).min(self.width);

        // Extend the backing storage up to `end` if needed, filling with default blanks.
        if self.cells.len() < end {
            self.cells
                .resize(end, Cell::blank_with_tag(FormatTag::default()));
        }

        // If `end` cuts through a wide glyph (continuation at `end` whose head is
        // before `end`), extend `end` to cover the whole glyph so no dangling
        // continuation is left.
        let erase_end = if end < self.cells.len() && self.cells[end].is_continuation() {
            let mut head = end;
            while head > 0 && self.cells[head].is_continuation() {
                head -= 1;
            }
            if self.cells[head].is_head() {
                (head + self.cells[head].display_width()).min(self.cells.len())
            } else {
                end
            }
        } else {
            end
        };

        // Replace every cell in [col .. erase_end] with a blank using `tag`.
        for i in col..erase_end.min(self.cells.len()) {
            self.cells[i] = Cell::blank_with_tag(tag.clone());
        }

        // Trim trailing default blanks to maintain the sparse-row invariant.
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag() == &FormatTag::default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }

    /// Like `insert_spaces_at`, but shifts only within `[col, right_limit)`.
    /// Cells at or beyond `right_limit` are not affected; cells shifted beyond
    /// the limit are discarded.  `right_limit` must be ≤ `self.width`.
    pub fn insert_spaces_at_with_right_limit(
        &mut self,
        col: usize,
        n: usize,
        tag: &FormatTag,
        right_limit: usize,
    ) {
        let limit = right_limit.min(self.width);

        if n == 0 || col >= limit {
            return;
        }

        self.dirty = true;

        let insert_len = n.min(limit.saturating_sub(col));
        let old_len = self.cells.len().min(limit); // only cells inside the margin matter

        let needed_len = (old_len + insert_len).max(col + insert_len).min(limit);
        if needed_len == 0 {
            return;
        }

        // Ensure storage up to `limit` (fill with default blanks).
        if self.cells.len() < limit {
            self.cells
                .resize(limit, Cell::blank_with_tag(FormatTag::default()));
        }

        // Shift cells [col..limit-insert_len) right by insert_len within [col, limit).
        let shift_end = limit.saturating_sub(insert_len);
        for i in (col..shift_end).rev() {
            let dest = i + insert_len;
            if dest < limit {
                self.cells[dest] = self.cells[i].clone();
            }
        }

        // Fill [col..col+insert_len) with blanks.
        for i in col..(col + insert_len).min(limit) {
            self.cells[i] = Cell::blank_with_tag(tag.clone());
        }

        // Clamp storage to logical width.
        if self.cells.len() > self.width {
            self.cells.truncate(self.width);
        }

        // Maintain sparse-row invariant.
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag() == &FormatTag::default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }

    /// Like `delete_cells_at`, but the right boundary of the operation is
    /// `right_limit`.  Cells at `[col, col+n)` are removed; cells in
    /// `[col+n, right_limit)` shift left to fill the gap; blanks (tagged with
    /// `tag`) are inserted at the end of `[right_limit-n, right_limit)`.
    /// Cells outside `[col, right_limit)` are not affected.
    pub fn delete_cells_at_with_right_limit(
        &mut self,
        col: usize,
        n: usize,
        right_limit: usize,
        tag: &FormatTag,
    ) {
        let limit = right_limit.min(self.width);

        if n == 0 || col >= limit || col >= self.cells.len() {
            return;
        }

        self.dirty = true;

        let delete_n = n.min(limit.saturating_sub(col));

        // Ensure storage up to `limit`.
        if self.cells.len() < limit {
            self.cells
                .resize(limit, Cell::blank_with_tag(FormatTag::default()));
        }

        // Shift cells [col+delete_n, limit) left by delete_n.
        for i in col..limit.saturating_sub(delete_n) {
            self.cells[i] = self.cells[i + delete_n].clone();
        }

        // Fill [limit-delete_n, limit) with blanks.
        let fill_start = limit.saturating_sub(delete_n);
        for i in fill_start..limit {
            self.cells[i] = Cell::blank_with_tag(tag.clone());
        }

        // Clamp storage.
        if self.cells.len() > self.width {
            self.cells.truncate(self.width);
        }

        // Maintain sparse-row invariant.
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag() == &FormatTag::default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }

    /// Delete `n` cells starting at `col`, shifting cells to the right of the deleted
    /// range left to fill the gap. Implements VT DCH (Delete Character).
    ///
    /// - Cursor does not move (caller's responsibility).
    /// - If `col` is beyond the stored cells, this is a no-op.
    /// - If `n` exceeds the cells to the right of `col`, everything from `col` onward
    ///   is removed.
    /// - Wide-glyph cleanup: if `col` lands on a continuation cell, back up to its
    ///   head and extend the deletion to cover the whole glyph. If `col` lands on a
    ///   head, extend the deletion to include all its trailing continuation cells.
    /// - BCE: the `tag` parameter is used for any blank cells created during
    ///   wide-glyph boundary cleanup (e.g. when a deletion splits a wide glyph).
    pub fn delete_cells_at(&mut self, col: usize, n: usize, tag: &FormatTag) {
        if n == 0 || col >= self.cells.len() {
            return;
        }

        self.dirty = true;

        // --- Wide-glyph cleanup: find the real start of deletion --------
        let mut start = col;

        // If we land on a continuation, walk back to the head and include it.
        if start < self.cells.len() && self.cells[start].is_continuation() {
            let mut head = start;
            while head > 0 && self.cells[head].is_continuation() {
                head -= 1;
            }
            // head is now either the wide head or position 0.
            if self.cells[head].is_head() {
                start = head;
            }
        }

        // Extend deletion to cover any trailing continuations of a head at `start`.
        let mut end = (start + n).min(self.cells.len());

        // If the cell at `start` is a wide head, make sure we include all of its
        // continuation cells (they may already be covered, but let's be safe).
        if start < self.cells.len() && self.cells[start].is_head() {
            let head_width = self.cells[start].display_width();
            end = end.max((start + head_width).min(self.cells.len()));
        }

        // Also extend `end` if it cuts through a wide glyph (continuation at `end`
        // whose head is before `end`): we blank the whole glyph.
        if end < self.cells.len() && self.cells[end].is_continuation() {
            // Walk back to find head
            let mut head = end;
            while head > 0 && self.cells[head].is_continuation() {
                head -= 1;
            }
            if self.cells[head].is_head() {
                // Replace the head+continuations with blanks rather than splitting.
                let head_width = self.cells[head].display_width();
                let replace_end = (head + head_width).min(self.cells.len());
                for i in head..replace_end {
                    self.cells[i] = Cell::blank_with_tag(tag.clone());
                }
            }
        }

        // Clamp end to actual length
        let end = end.min(self.cells.len());

        // --- Remove the range [start..end] by draining it ---------------
        self.cells.drain(start..end);

        // Trim trailing visually-default blanks to maintain the sparse-row invariant.
        while let Some(last) = self.cells.last() {
            if last.tchar() == &TChar::Space && last.tag().is_visually_default() {
                self.cells.pop();
            } else {
                break;
            }
        }
    }

    /// Set a cell at the given column to an image placement.
    ///
    /// Extends the cell vector if `col` is beyond the current length,
    /// filling gaps with blank cells.
    pub fn set_image_cell(
        &mut self,
        col: usize,
        placement: crate::image_store::ImagePlacement,
        tag: FormatTag,
    ) {
        if col >= self.width {
            return;
        }
        self.dirty = true;

        // Extend cells to reach the target column if needed.
        if col >= self.cells.len() {
            let pad = col - self.cells.len() + 1;
            self.cells
                .extend(std::iter::repeat_n(Cell::blank_with_tag(tag.clone()), pad));
        }

        // Clean up any wide character at this position.
        self.cleanup_wide_overwrite(col);

        self.cells[col] = Cell::image_cell(placement, tag);
    }
}
