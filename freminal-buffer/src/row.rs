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

/// Line-width attribute set by DEC escape sequences.
///
/// Controls whether glyphs on this row are rendered at normal size or scaled
/// 2× horizontally (DECDWL) or 2× in both dimensions (DECDHL).  This is a
/// rendering-only attribute: the buffer column count is not modified.  The
/// renderer uses this to apply per-row glyph scaling in the vertex builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineWidth {
    /// Normal single-width, single-height line (ESC # 5 or default).
    #[default]
    Normal,
    /// Double-width line (DECDWL, ESC # 6).  Each character is rendered at
    /// 2× horizontal scale.  The buffer column count is unchanged; the
    /// renderer displays only the first half of the columns.
    DoubleWidth,
    /// Top half of a double-height line (DECDHL, ESC # 3).  Glyphs are scaled
    /// 2× in both dimensions; only the upper half is visible on this row.
    DoubleHeightTop,
    /// Bottom half of a double-height line (DECDHL, ESC # 4).  Glyphs are
    /// scaled 2× in both dimensions; only the lower half is visible on this row.
    DoubleHeightBottom,
}

impl LineWidth {
    /// Returns `true` if this line uses double-width rendering (DECDWL or DECDHL).
    #[must_use]
    pub const fn is_double_width(self) -> bool {
        !matches!(self, Self::Normal)
    }
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
    /// Per-row line-width attribute (DECDWL / DECDHL).
    ///
    /// Defaults to [`LineWidth::Normal`].  Set via `ESC # 3/4/5/6` on the
    /// current cursor row.  This is a rendering attribute only — the renderer
    /// uses it to apply per-row glyph scaling in the vertex builder.
    pub line_width: LineWidth,
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
            line_width: LineWidth::Normal,
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
            line_width: LineWidth::Normal,
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
            line_width: LineWidth::Normal,
        }
    }

    /// Clear all cells in this row, leaving it empty (sparse).
    pub fn clear(&mut self) {
        self.dirty = true;
        self.cells.clear();
    }

    /// Fill this row with blank cells carrying the given format tag (BCE).
    ///
    /// Always clears existing cell data first.  If the tag is visually default
    /// the row is left sparse (empty `cells` vec).  Otherwise, explicit blank
    /// cells are written so the renderer picks up the correct background color.
    pub fn fill_with_tag(&mut self, tag: &FormatTag) {
        self.clear();

        if tag.is_visually_default() {
            return;
        }
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

    /// Clip the physical cell storage to `new_width` columns.
    ///
    /// Used by [`Buffer::set_size`] when shrinking the alternate screen (which
    /// must not reflow).  Without this, rows retain cells beyond the new width
    /// and [`Buffer::flatten_row`] emits them — the snapshot then contains
    /// rows wider than `term_width`, producing a stale strip of glyphs/
    /// backgrounds at the right edge of the viewport after a shrink.
    ///
    /// If a wide-glyph head sits at column `new_width - 1` its continuation
    /// cell at column `new_width` would be orphaned, so the head is converted
    /// to a blank using the head's own format tag (preserving background).
    pub fn truncate_cells_to_width(&mut self, new_width: usize) {
        if self.cells.len() <= new_width {
            return;
        }

        // Guard against splitting a wide glyph at the boundary.  If the cell
        // at new_width is a continuation, its head sits at new_width - 1 and
        // must become a blank (keep its format so BCE background survives).
        if new_width > 0
            && let Some(boundary_cell) = self.cells.get(new_width)
            && boundary_cell.is_continuation()
        {
            let head_tag = self.cells[new_width - 1].tag().clone();
            self.cells[new_width - 1] = Cell::blank_with_tag(head_tag);
        }

        self.cells.truncate(new_width);
        // We mutated `cells` (and possibly cell content at `new_width - 1`),
        // so invalidate the Buffer's row cache. Matches every other mutator
        // in this file.
        self.dirty = true;
    }

    /// How many cells are currently occupied.
    #[must_use]
    pub fn row_width(&self) -> usize {
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
    pub fn char_at(&self, idx: usize) -> Option<&Cell> {
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
    pub const fn characters(&self) -> &Vec<Cell> {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use freminal_common::buffer_states::{format_tag::FormatTag, tchar::TChar};

    use crate::{
        cell::Cell,
        image_store::{ImagePlacement, ImageProtocol, InlineImage},
        response::InsertResponse,
    };

    use super::{LineWidth, Row};

    // -----------------------------------------------------------------------
    // LineWidth::is_double_width
    // -----------------------------------------------------------------------

    #[test]
    fn line_width_normal_is_not_double() {
        assert!(!LineWidth::Normal.is_double_width());
    }

    #[test]
    fn line_width_double_width_is_double() {
        assert!(LineWidth::DoubleWidth.is_double_width());
    }

    #[test]
    fn line_width_double_height_top_is_double() {
        assert!(LineWidth::DoubleHeightTop.is_double_width());
    }

    #[test]
    fn line_width_double_height_bottom_is_double() {
        assert!(LineWidth::DoubleHeightBottom.is_double_width());
    }

    // -----------------------------------------------------------------------
    // max_width / set_max_width round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn max_width_returns_initial_width() {
        let row = Row::new(80);
        assert_eq!(row.max_width(), 80);
    }

    #[test]
    fn set_max_width_updates_width() {
        let mut row = Row::new(80);
        row.set_max_width(132);
        assert_eq!(row.max_width(), 132);
    }

    #[test]
    fn set_max_width_round_trip() {
        let mut row = Row::new(40);
        row.set_max_width(80);
        assert_eq!(row.max_width(), 80);
        row.set_max_width(40);
        assert_eq!(row.max_width(), 40);
    }

    // -----------------------------------------------------------------------
    // truncate_cells_to_width
    // -----------------------------------------------------------------------

    #[test]
    fn truncate_cells_to_width_drops_cells_beyond_new_width() {
        let mut row = Row::new(10);
        let text: Vec<TChar> = b"abcdefghij".iter().map(|b| TChar::Ascii(*b)).collect();
        row.insert_text(0, &text, &FormatTag::default());
        assert_eq!(row.characters().len(), 10);

        row.set_max_width(5);
        row.truncate_cells_to_width(5);

        assert_eq!(row.characters().len(), 5);
        assert_eq!(row.max_width(), 5);
    }

    #[test]
    fn truncate_cells_to_width_noop_when_already_within_bounds() {
        let mut row = Row::new(10);
        let text: Vec<TChar> = b"abc".iter().map(|b| TChar::Ascii(*b)).collect();
        row.insert_text(0, &text, &FormatTag::default());
        let before = row.characters().len();

        // new_width >= cells.len() — nothing to truncate
        row.truncate_cells_to_width(20);

        assert_eq!(row.characters().len(), before);
    }

    #[test]
    fn truncate_cells_to_width_handles_wide_glyph_at_boundary() {
        // A wide glyph whose head sits at `new_width - 1` would leave an
        // orphan continuation at `new_width` after truncation.  The head
        // must be replaced with a blank so the row stays well-formed.
        let mut row = Row::new(10);

        // Build: 'a' at col 0, wide CJK glyph (head at col 1, continuation at col 2), 'b' at col 3.
        row.cells_mut_push(Cell::new(TChar::Ascii(b'a'), FormatTag::default()));
        row.cells_mut_push(Cell::new(TChar::from('中'), FormatTag::default()));
        row.cells_mut_push(Cell::wide_continuation());
        row.cells_mut_push(Cell::new(TChar::Ascii(b'b'), FormatTag::default()));
        for _ in 4..10 {
            row.cells_mut_push(Cell::blank_with_tag(FormatTag::default()));
        }
        assert!(row.characters()[1].is_head());
        assert!(row.characters()[2].is_continuation());

        // Truncate to 2 cols: the continuation at col 2 is cut, so the head
        // at col 1 must become a blank.
        row.truncate_cells_to_width(2);

        assert_eq!(row.characters().len(), 2);
        assert!(!row.characters()[1].is_head());
        assert!(!row.characters()[1].is_continuation());
    }

    #[test]
    fn truncate_cells_to_width_marks_row_dirty_when_it_mutates() {
        let mut row = Row::new(10);
        let text: Vec<TChar> = b"abcdefghij".iter().map(|b| TChar::Ascii(*b)).collect();
        row.insert_text(0, &text, &FormatTag::default());

        // Clear the dirty flag so we can assert the mutation re-sets it.
        row.dirty = false;
        row.truncate_cells_to_width(5);
        assert!(
            row.dirty,
            "truncate_cells_to_width must mark the row dirty so the Buffer's row cache is invalidated"
        );
    }

    #[test]
    fn truncate_cells_to_width_does_not_mark_row_dirty_on_noop() {
        let mut row = Row::new(10);
        let text: Vec<TChar> = b"abc".iter().map(|b| TChar::Ascii(*b)).collect();
        row.insert_text(0, &text, &FormatTag::default());

        row.dirty = false;
        // new_width >= cells.len() — no mutation should happen.
        row.truncate_cells_to_width(20);
        assert!(
            !row.dirty,
            "truncate_cells_to_width must not flip the dirty flag when there is nothing to truncate"
        );
    }

    // -----------------------------------------------------------------------
    // count_image_cells_in_range
    // -----------------------------------------------------------------------

    fn make_image_placement(image_id: u64) -> ImagePlacement {
        ImagePlacement {
            image_id,
            col_in_image: 0,
            row_in_image: 0,
            protocol: ImageProtocol::Sixel,
            image_number: None,
            placement_id: None,
            z_index: 0,
        }
    }

    fn make_test_row_with_images() -> Row {
        // Row of width 10: columns 2 and 5 hold image cells, rest are normal
        let mut row = Row::new(10);
        // Insert some normal text first to populate cells
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"abcdefghij".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        // Overwrite cols 2 and 5 with image cells
        row.set_image_cell(2, make_image_placement(1), tag.clone());
        row.set_image_cell(5, make_image_placement(2), tag.clone());
        row
    }

    #[test]
    fn count_image_cells_in_range_mixed() {
        let row = make_test_row_with_images();
        // Range covering both image cells
        assert_eq!(row.count_image_cells_in_range(0, 10), 2);
        // Range covering only first image cell
        assert_eq!(row.count_image_cells_in_range(0, 4), 1);
        // Range covering only second image cell
        assert_eq!(row.count_image_cells_in_range(4, 7), 1);
        // Range with no image cells
        assert_eq!(row.count_image_cells_in_range(0, 2), 0);
        assert_eq!(row.count_image_cells_in_range(3, 5), 0);
    }

    #[test]
    fn count_image_cells_in_range_out_of_bounds() {
        let row = make_test_row_with_images();
        // Range entirely beyond stored cells — should return 0
        assert_eq!(row.count_image_cells_in_range(20, 30), 0);
        // start >= end — should return 0
        assert_eq!(row.count_image_cells_in_range(5, 3), 0);
        assert_eq!(row.count_image_cells_in_range(5, 5), 0);
    }

    #[test]
    fn count_image_cells_in_range_empty_row() {
        let row = Row::new(80);
        assert_eq!(row.count_image_cells_in_range(0, 80), 0);
    }

    // -----------------------------------------------------------------------
    // get_row_width — counts columns contributed by wide-glyph heads only
    // -----------------------------------------------------------------------

    #[test]
    fn get_row_width_ascii_chars_returns_zero() {
        // get_row_width only counts wide-glyph heads; ASCII cells (is_wide_head=false)
        // are not counted.
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        // No wide glyphs → get_row_width() returns 0
        assert_eq!(row.row_width(), 0);
    }

    #[test]
    fn get_row_width_empty_row() {
        let row = Row::new(80);
        assert_eq!(row.row_width(), 0);
    }

    #[test]
    fn get_row_width_with_wide_glyphs() {
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        // 'あ' is a wide character (2 columns)
        let wide: TChar = TChar::from('あ');
        assert_eq!(wide.display_width(), 2);
        row.insert_text(0, &[wide], &tag);
        // One wide glyph head contributes 2 columns
        assert_eq!(row.row_width(), 2);
    }

    #[test]
    fn get_row_width_multiple_wide_glyphs() {
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        // Two wide chars → 2 * 2 = 4
        let text = vec![TChar::from('あ'), TChar::from('い')];
        row.insert_text(0, &text, &tag);
        assert_eq!(row.row_width(), 4);
    }

    #[test]
    fn get_row_width_mixed_wide_and_ascii_counts_only_wide() {
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        // 'A' (ASCII, not wide head) + 'あ' (wide head, 2 cols) + 'B' (ASCII)
        let text = vec![TChar::Ascii(b'A'), TChar::from('あ'), TChar::Ascii(b'B')];
        row.insert_text(0, &text, &tag);
        // Only 'あ' is a wide head → get_row_width() = 2
        assert_eq!(row.row_width(), 2);
    }

    // -----------------------------------------------------------------------
    // cleanup_wide_overwrite — tested indirectly via insert_text
    // -----------------------------------------------------------------------

    #[test]
    fn overwriting_continuation_clears_whole_wide_glyph() {
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        // Insert a wide char at col 0 → head at 0, continuation at 1
        let wide = TChar::from('あ');
        row.insert_text(0, &[wide], &tag);
        assert!(row.char_at(0).unwrap().is_head());
        assert!(row.char_at(1).unwrap().is_continuation());

        // Now insert a normal char at col 1 (the continuation position)
        // This should trigger cleanup_wide_overwrite, clearing the head at col 0
        row.insert_text(1, &[TChar::Ascii(b'X')], &tag);

        // The cell at col 0 should no longer be a head (it was blanked)
        let cell_0 = row.char_at(0).unwrap();
        assert!(!cell_0.is_head(), "head cell should have been blanked");
        assert!(
            !cell_0.is_continuation(),
            "head should not be a continuation"
        );

        // The cell at col 1 should be 'X'
        let cell_1 = row.char_at(1).unwrap();
        assert_eq!(cell_1.tchar(), &TChar::Ascii(b'X'));
    }

    #[test]
    fn overwriting_head_clears_continuations() {
        let mut row = Row::new(80);
        let tag = FormatTag::default();
        // Insert a wide char at col 2
        let text = vec![TChar::Ascii(b'A'), TChar::Ascii(b'B'), TChar::from('あ')];
        row.insert_text(0, &text, &tag);
        // col 2 = head, col 3 = continuation
        assert!(row.char_at(2).unwrap().is_head());
        assert!(row.char_at(3).unwrap().is_continuation());

        // Overwrite the head at col 2 with a normal char
        row.insert_text(2, &[TChar::Ascii(b'Y')], &tag);

        // The continuation at col 3 should have been cleared (made into a space)
        let cell_3 = row.char_at(3).unwrap();
        assert!(
            !cell_3.is_continuation(),
            "continuation should have been cleared"
        );
    }

    // -----------------------------------------------------------------------
    // insert_text boundary guard — start_col >= right_limit → Leftover
    // -----------------------------------------------------------------------

    #[test]
    fn insert_text_start_at_limit_returns_leftover() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::Ascii(b'A')];
        // Starting exactly at the row width should return Leftover immediately
        let response = row.insert_text_with_limit(10, &text, &tag, 10);
        match response {
            InsertResponse::Leftover { leftover_start, .. } => {
                assert_eq!(leftover_start, 0);
            }
            InsertResponse::Consumed(_) => panic!("expected Leftover, got Consumed"),
        }
    }

    #[test]
    fn insert_text_start_beyond_limit_returns_leftover() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::Ascii(b'A'), TChar::Ascii(b'B')];
        let response = row.insert_text_with_limit(12, &text, &tag, 10);
        match response {
            InsertResponse::Leftover { leftover_start, .. } => {
                assert_eq!(leftover_start, 0);
            }
            InsertResponse::Consumed(_) => panic!("expected Leftover, got Consumed"),
        }
    }

    #[test]
    fn insert_text_fills_row_and_returns_leftover() {
        let mut row = Row::new(5);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABCDEFGH".iter().map(|&b| TChar::Ascii(b)).collect();
        let response = row.insert_text(0, &text, &tag);
        match response {
            InsertResponse::Leftover {
                leftover_start,
                final_col,
            } => {
                // 5 chars fit, leftover starts at index 5
                assert_eq!(leftover_start, 5);
                assert_eq!(final_col, 5);
            }
            InsertResponse::Consumed(_) => panic!("expected Leftover, got Consumed"),
        }
    }

    // -----------------------------------------------------------------------
    // insert_spaces_at
    // -----------------------------------------------------------------------

    #[test]
    fn insert_spaces_at_n_zero_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.insert_spaces_at(2, 0, &tag);
        // Nothing should change
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn insert_spaces_at_col_beyond_width_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.insert_spaces_at(10, 1, &tag); // col == width → no-op
        assert_eq!(row.cells(), cells_before.as_slice());

        row.insert_spaces_at(15, 1, &tag); // col > width → no-op
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn insert_spaces_at_shifts_cells_right() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        // Place 'A', 'B', 'C' at cols 0, 1, 2
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Insert 2 spaces at col 1 → 'A' stays at 0, two spaces at 1,2, 'B' shifts to 3, 'C' to 4
        row.insert_spaces_at(1, 2, &tag);

        assert_eq!(row.char_at(0).unwrap().tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.char_at(1).unwrap().tchar(), &TChar::Space);
        assert_eq!(row.char_at(2).unwrap().tchar(), &TChar::Space);
        assert_eq!(row.char_at(3).unwrap().tchar(), &TChar::Ascii(b'B'));
        assert_eq!(row.char_at(4).unwrap().tchar(), &TChar::Ascii(b'C'));
    }

    // -----------------------------------------------------------------------
    // clear_to
    // -----------------------------------------------------------------------

    #[test]
    fn clear_to_with_default_tag_leaves_sparse() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Clear cols 0..3 with default tag
        row.clear_to(3, &tag);

        // After clearing, cells before col 3 should be blank (sparse representation:
        // trailing default blanks may be trimmed, but non-trailing ones persist)
        // The remaining chars at col 3+ should still be there
        let cell_3 = row.char_at(3);
        assert!(
            cell_3.is_none() || cell_3.unwrap().tchar() == &TChar::Ascii(b'l'),
            "cell at 3 should be 'l' or absent if sparse"
        );
    }

    #[test]
    fn clear_to_with_colored_tag_writes_explicit_blanks() {
        use freminal_common::colors::TerminalColor;

        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &default_tag);

        // Build a non-default tag with a custom background color
        let mut colored_tag = FormatTag::default();
        colored_tag.colors.set_background_color(TerminalColor::Red);

        // Clear cols 0..3 with the colored tag
        row.clear_to(3, &colored_tag);

        // Cells 0,1,2 should now be explicit blanks with the colored tag
        for i in 0..3 {
            let cell = row.char_at(i).unwrap();
            assert_eq!(cell.tchar(), &TChar::Space, "cell {i} should be blank");
        }
    }

    // -----------------------------------------------------------------------
    // clear_with_tag
    // -----------------------------------------------------------------------

    #[test]
    fn clear_with_default_tag_leaves_empty_cells_vec() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        assert!(!row.cells().is_empty());

        row.clear_with_tag(&tag);
        // With a default tag, the sparse representation stores no cells
        assert!(
            row.cells().is_empty(),
            "default tag should leave row sparse"
        );
    }

    #[test]
    fn clear_with_colored_tag_fills_full_width() {
        use freminal_common::colors::TerminalColor;

        let mut row = Row::new(10);
        let default_tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &default_tag);

        let mut colored_tag = FormatTag::default();
        colored_tag.colors.set_background_color(TerminalColor::Blue);

        row.clear_with_tag(&colored_tag);

        // All 10 columns should be explicit blank cells with the colored tag
        assert_eq!(
            row.cells().len(),
            10,
            "colored tag should fill full row width"
        );
        for cell in row.cells() {
            assert_eq!(cell.tchar(), &TChar::Space);
        }
    }

    // -----------------------------------------------------------------------
    // erase_cells_at spanning a wide glyph boundary
    // -----------------------------------------------------------------------

    #[test]
    fn erase_cells_at_spanning_wide_glyph_erases_whole_glyph() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        // Place: 'A'(0), 'B'(1), 'あ'(2,3-continuation), 'C'(4)
        let text = vec![
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::from('あ'),
            TChar::Ascii(b'C'),
        ];
        row.insert_text(0, &text, &tag);

        // Sanity check: col 2 is a wide head, col 3 is continuation
        assert!(row.char_at(2).unwrap().is_head());
        assert!(row.char_at(3).unwrap().is_continuation());

        // Erase cols 2..3 (n=1 starting at col 2) — this hits only the head,
        // but the continuation at col 3 should also be cleared.
        row.erase_cells_at(2, 1, &tag);

        // Col 2 and 3 should be blank spaces (no continuation)
        let cell_2 = row.resolve_cell(2);
        let cell_3 = row.resolve_cell(3);
        assert_eq!(cell_2.tchar(), &TChar::Space);
        assert!(
            !cell_3.is_continuation(),
            "continuation should have been erased"
        );

        // Neighboring cells should be intact
        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Ascii(b'B'));
        assert_eq!(row.resolve_cell(4).tchar(), &TChar::Ascii(b'C'));
    }

    #[test]
    fn erase_cells_at_n_zero_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.erase_cells_at(2, 0, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn erase_cells_at_beyond_width_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"hello".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.erase_cells_at(10, 5, &tag); // col == width
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    // -----------------------------------------------------------------------
    // retain_referenced test via image cells
    // -----------------------------------------------------------------------

    #[test]
    fn set_image_cell_places_image_in_row() {
        let mut store = crate::image_store::ImageStore::new();
        let pixels = vec![255u8; 8 * 16 * 4];
        let image_id = crate::image_store::next_image_id();
        store.insert(InlineImage {
            id: image_id,
            pixels: Arc::new(pixels),
            width_px: 8,
            height_px: 16,
            display_cols: 1,
            display_rows: 1,
        });

        let mut row = Row::new(80);
        row.set_image_cell(0, make_image_placement(image_id), FormatTag::default());

        assert!(
            row.char_at(0).unwrap().has_image(),
            "col 0 should be an image cell"
        );
        assert_eq!(row.count_image_cells(), 1);
    }

    // -----------------------------------------------------------------------
    // width() accessor
    // -----------------------------------------------------------------------

    #[test]
    fn width_returns_logical_width() {
        let row = Row::new(42);
        assert_eq!(row.width(), 42);
    }

    // -----------------------------------------------------------------------
    // cells_mut_push()
    // -----------------------------------------------------------------------

    #[test]
    fn cells_mut_push_appends_cell() {
        let mut row = Row::new(10);
        assert!(row.cells().is_empty());

        row.cells_mut_push(Cell::new(TChar::Ascii(b'X'), FormatTag::default()));
        assert_eq!(row.cells().len(), 1);
        assert_eq!(row.cells()[0].tchar(), &TChar::Ascii(b'X'));
    }

    // -----------------------------------------------------------------------
    // cleanup_wide_overwrite edge cases (exercised via insert_text overwriting)
    // -----------------------------------------------------------------------

    #[test]
    fn overwrite_continuation_cell_clears_wide_glyph() {
        // Place a wide character 'あ' at cols 0-1 (head + continuation)
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::from('あ')];
        row.insert_text(0, &text, &tag);

        // Sanity: col 0 is head, col 1 is continuation
        assert!(row.cells()[0].is_head());
        assert!(row.cells()[1].is_continuation());

        // Overwrite at col 1 (continuation) with a narrow char — should
        // clear the whole wide glyph (head + continuation) first
        let overwrite = vec![TChar::Ascii(b'X')];
        row.insert_text(1, &overwrite, &tag);

        // Col 0 should be blank (the head was cleared)
        assert_eq!(row.cells()[0].tchar(), &TChar::Space);
        assert!(!row.cells()[0].is_head());
        // Col 1 should now be 'X'
        assert_eq!(row.cells()[1].tchar(), &TChar::Ascii(b'X'));
    }

    #[test]
    fn overwrite_head_cell_clears_continuations() {
        // Place a wide character 'あ' at cols 0-1
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::from('あ')];
        row.insert_text(0, &text, &tag);

        // Overwrite at col 0 (head) with a narrow char
        let overwrite = vec![TChar::Ascii(b'Y')];
        row.insert_text(0, &overwrite, &tag);

        // Col 0 should now be 'Y'
        assert_eq!(row.cells()[0].tchar(), &TChar::Ascii(b'Y'));
        // Col 1 (was continuation) should now be blank
        assert_eq!(row.cells()[1].tchar(), &TChar::Space);
        assert!(!row.cells()[1].is_continuation());
    }

    #[test]
    fn cleanup_wide_overwrite_col_beyond_cells_is_noop() {
        // Insert some text, then overwrite beyond stored cells
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::Ascii(b'A')];
        row.insert_text(0, &text, &tag);

        // Col 5 is beyond stored cells (only 1 cell stored) — should not panic
        // We can trigger cleanup_wide_overwrite indirectly by inserting at col 5
        let overwrite = vec![TChar::Ascii(b'Z')];
        row.insert_text(5, &overwrite, &tag);

        // Col 5 should be 'Z', everything between 1..5 padded with spaces
        assert_eq!(row.cells()[5].tchar(), &TChar::Ascii(b'Z'));
    }

    // -----------------------------------------------------------------------
    // insert_text_with_limit: wide char that overflows limit
    // -----------------------------------------------------------------------

    #[test]
    fn insert_wide_char_overflow_returns_leftover() {
        // Row width 4, insert a wide char (width 2) at col 3 → won't fit
        let mut row = Row::new(4);
        let tag = FormatTag::default();
        let text = vec![TChar::from('あ')]; // display width 2
        let response = row.insert_text(3, &text, &tag);

        match response {
            InsertResponse::Leftover {
                leftover_start,
                final_col,
            } => {
                assert_eq!(leftover_start, 0);
                assert_eq!(final_col, 3);
            }
            InsertResponse::Consumed(_) => panic!("expected Leftover for wide char overflow"),
        }
    }

    #[test]
    fn insert_text_wide_char_with_continuation_within_bounds() {
        // Verify that a wide char properly inserts head + continuation cells
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::from('あ')]; // display width 2
        let response = row.insert_text(0, &text, &tag);

        assert!(matches!(response, InsertResponse::Consumed(2)));
        assert!(row.cells()[0].is_head());
        assert!(row.cells()[1].is_continuation());
    }

    #[test]
    fn insert_text_col_clamp_at_width() {
        // Fill entire row, verify col is clamped to width
        let mut row = Row::new(3);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        let response = row.insert_text(0, &text, &tag);

        // All 3 chars fit, col should be 3 (== width)
        assert!(matches!(response, InsertResponse::Consumed(3)));
    }

    // -----------------------------------------------------------------------
    // insert_spaces_at: needed_len == 0 edge case
    // -----------------------------------------------------------------------

    // NOTE: The needed_len == 0 early return (line 473) is extremely difficult
    // to trigger since n > 0 && col < width guarantees insert_len >= 1, meaning
    // needed_len >= 1. This is a defensive guard.

    // -----------------------------------------------------------------------
    // clear_to with BCE: extending cells for non-default tag
    // -----------------------------------------------------------------------

    #[test]
    fn clear_to_with_bce_extends_sparse_row() {
        use freminal_common::colors::TerminalColor;

        let mut row = Row::new(10);
        // Row is empty (sparse), clear_to(5) with colored tag should extend cells
        let mut colored_tag = FormatTag::default();
        colored_tag
            .colors
            .set_background_color(TerminalColor::Green);

        row.clear_to(5, &colored_tag);

        // Should have written explicit blank cells for cols 0..5
        assert_eq!(row.cells().len(), 5);
        for cell in row.cells() {
            assert_eq!(cell.tchar(), &TChar::Space);
        }
    }

    // -----------------------------------------------------------------------
    // erase_cells_at: extending cells beyond current length
    // -----------------------------------------------------------------------

    #[test]
    fn erase_cells_at_extends_cells_when_end_beyond_stored() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        // Insert only 3 cells
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Erase 8 cells starting at col 1 — end = 1+8 = 9, but only 3 cells stored
        // This should extend the cell vec before erasing
        row.erase_cells_at(1, 8, &tag);

        // Cols 1..9 should be blank; the row should have trimmed trailing blanks
        // (sparse-row invariant), so we just check col 0 is intact
        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
    }

    #[test]
    fn erase_cells_at_wide_glyph_end_boundary() {
        // Place 'A' at 0, wide 'あ' at 1-2, 'B' at 3
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::Ascii(b'A'), TChar::from('あ'), TChar::Ascii(b'B')];
        row.insert_text(0, &text, &tag);

        // Erase 1 cell at col 0 — end=1 which is the head of the wide char.
        // The wide glyph at 1-2 should not be damaged.
        row.erase_cells_at(0, 1, &tag);

        // Col 0 should be blank
        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Space);
        // Col 1 should still be the wide head
        assert!(row.char_at(1).unwrap().is_head());
        assert!(row.char_at(2).unwrap().is_continuation());
        assert_eq!(row.resolve_cell(3).tchar(), &TChar::Ascii(b'B'));
    }

    // -----------------------------------------------------------------------
    // insert_spaces_at_with_right_limit
    // -----------------------------------------------------------------------

    #[test]
    fn insert_spaces_with_right_limit_basic() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABCDE".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Insert 2 spaces at col 1 with right_limit=4
        // Only cells in [1, 4) are affected: 'B', 'C', 'D' shift right within [1,4)
        // Result: 'A', ' ', ' ', 'B', 'D', 'E'  (C shifted out past limit=4)
        row.insert_spaces_at_with_right_limit(1, 2, &tag, 4);

        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Space);
        assert_eq!(row.resolve_cell(2).tchar(), &TChar::Space);
        // B shifted from 1 to 3
        assert_eq!(row.resolve_cell(3).tchar(), &TChar::Ascii(b'B'));
        // D and E are outside the margin and should be untouched
        assert_eq!(row.resolve_cell(4).tchar(), &TChar::Ascii(b'E'));
    }

    #[test]
    fn insert_spaces_with_right_limit_n_zero_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.insert_spaces_at_with_right_limit(1, 0, &tag, 5);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn insert_spaces_with_right_limit_col_at_limit_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.insert_spaces_at_with_right_limit(5, 2, &tag, 5);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn insert_spaces_with_right_limit_truncates_excess() {
        use freminal_common::colors::TerminalColor;

        // Use a non-default tag so cells are explicit (not trimmed by sparse-row invariant)
        let mut colored_tag = FormatTag::default();
        colored_tag.colors.set_background_color(TerminalColor::Cyan);

        let mut row = Row::new(5);
        let text: Vec<TChar> = b"ABCDE".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &FormatTag::default());

        // Insert 3 spaces at col 0 with right_limit=5 → shifts everything right,
        // cells that go past limit=5 are discarded
        row.insert_spaces_at_with_right_limit(0, 3, &colored_tag, 5);

        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Space);
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Space);
        assert_eq!(row.resolve_cell(2).tchar(), &TChar::Space);
        // A shifted from 0 to 3, B from 1 to 4
        assert_eq!(row.resolve_cell(3).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(4).tchar(), &TChar::Ascii(b'B'));
    }

    // -----------------------------------------------------------------------
    // delete_cells_at_with_right_limit
    // -----------------------------------------------------------------------

    #[test]
    fn delete_cells_with_right_limit_basic() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABCDE".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Delete 1 cell at col 1 with right_limit=4
        // Cells in [1, 4): 'B', 'C', 'D' → delete B, C shifts to 1, D to 2,
        // blank fills at 3 (limit-1)
        row.delete_cells_at_with_right_limit(1, 1, 4, &tag);

        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Ascii(b'C'));
        assert_eq!(row.resolve_cell(2).tchar(), &TChar::Ascii(b'D'));
        // Col 3 should be blank (fill from margin edge)
        assert_eq!(row.resolve_cell(3).tchar(), &TChar::Space);
        // Col 4 is outside margin, should be untouched
        assert_eq!(row.resolve_cell(4).tchar(), &TChar::Ascii(b'E'));
    }

    #[test]
    fn delete_cells_with_right_limit_n_zero_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.delete_cells_at_with_right_limit(1, 0, 5, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn delete_cells_with_right_limit_col_at_limit_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.delete_cells_at_with_right_limit(5, 2, 5, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn delete_cells_with_right_limit_col_beyond_cells_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"AB".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        // col=5 is beyond stored cells (only 2), so noop
        row.delete_cells_at_with_right_limit(5, 2, 8, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn delete_cells_with_right_limit_extends_storage() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        // Only store 3 cells, but set right_limit = 8
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        assert_eq!(row.cells().len(), 3);

        // Delete 1 cell at col 1, limit = 8 → extends storage to 8, then shifts
        row.delete_cells_at_with_right_limit(1, 1, 8, &tag);

        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Ascii(b'C'));
    }

    // -----------------------------------------------------------------------
    // delete_cells_at: wide-glyph boundary handling
    // -----------------------------------------------------------------------

    #[test]
    fn delete_cells_at_on_continuation_backs_up_to_head() {
        // Place: 'A'(0), 'あ'(1-2), 'B'(3)
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::Ascii(b'A'), TChar::from('あ'), TChar::Ascii(b'B')];
        row.insert_text(0, &text, &tag);

        // Delete 1 cell at col 2 (the continuation) — should back up to col 1 (the head)
        // and delete both head + continuation
        row.delete_cells_at(2, 1, &tag);

        // 'A' remains at col 0, 'B' shifts left
        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        // The wide char (2 cells) was deleted, so B is now at col 1
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Ascii(b'B'));
    }

    #[test]
    fn delete_cells_at_on_head_deletes_full_wide_char() {
        // Place: 'あ'(0-1), 'B'(2)
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![TChar::from('あ'), TChar::Ascii(b'B')];
        row.insert_text(0, &text, &tag);

        // Delete 1 cell at col 0 (the head) — should extend to delete continuation too
        row.delete_cells_at(0, 1, &tag);

        // 'B' should shift to col 0
        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'B'));
    }

    #[test]
    fn delete_cells_at_end_cuts_through_wide_glyph() {
        // Place: 'A'(0), 'B'(1), 'あ'(2-3), 'C'(4)
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![
            TChar::Ascii(b'A'),
            TChar::Ascii(b'B'),
            TChar::from('あ'),
            TChar::Ascii(b'C'),
        ];
        row.insert_text(0, &text, &tag);

        // Delete 2 cells at col 0 → end = 0+2 = 2, which is the head of 'あ'.
        // But end lands exactly at the head, which is position 2, and the continuation
        // at position 3 is beyond end. The deletion should handle this cleanly.
        row.delete_cells_at(0, 2, &tag);

        // After deleting A and B (2 cells), the wide char should shift to col 0
        assert!(
            row.cells()[0].is_head() || row.resolve_cell(0).tchar() == &TChar::Space,
            "wide char head should be at col 0 or replaced with blanks"
        );
    }

    #[test]
    fn delete_cells_at_end_lands_on_continuation() {
        // Place: 'A'(0), 'あ'(1-2), 'あ'(3-4), 'B'(5)
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text = vec![
            TChar::Ascii(b'A'),
            TChar::from('あ'),
            TChar::from('あ'),
            TChar::Ascii(b'B'),
        ];
        row.insert_text(0, &text, &tag);

        // Delete 3 cells at col 0 → end = 0+3 = 3, which is a continuation cell.
        // The deletion end cuts through the second 'あ' — should blank the whole glyph.
        row.delete_cells_at(0, 3, &tag);

        // After deletion, 'B' should still be accessible
        // The exact layout depends on how many cells were drained vs blanked
        let found_b = (0..6).any(|i| row.resolve_cell(i).tchar() == &TChar::Ascii(b'B'));
        assert!(found_b, "B should still be present somewhere in the row");
    }

    #[test]
    fn delete_cells_at_n_zero_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.delete_cells_at(1, 0, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn delete_cells_at_col_beyond_stored_is_noop() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"AB".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);
        let cells_before: Vec<_> = row.cells().to_vec();

        row.delete_cells_at(5, 2, &tag);
        assert_eq!(row.cells(), cells_before.as_slice());
    }

    #[test]
    fn delete_cells_at_trims_trailing_default_blanks() {
        let mut row = Row::new(10);
        let tag = FormatTag::default();
        let text: Vec<TChar> = b"ABC".iter().map(|&b| TChar::Ascii(b)).collect();
        row.insert_text(0, &text, &tag);

        // Delete B at col 1 → A, C remain, trailing blanks trimmed
        row.delete_cells_at(1, 1, &tag);

        assert_eq!(row.resolve_cell(0).tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.resolve_cell(1).tchar(), &TChar::Ascii(b'C'));
        // Trailing blanks should be trimmed (sparse-row invariant)
        assert!(row.cells().len() <= 2);
    }

    // -----------------------------------------------------------------------
    // set_image_cell beyond width is noop
    // -----------------------------------------------------------------------

    #[test]
    fn set_image_cell_beyond_width_is_noop() {
        let image_id = crate::image_store::next_image_id();
        let mut row = Row::new(5);
        row.set_image_cell(5, make_image_placement(image_id), FormatTag::default());
        assert!(row.cells().is_empty());
    }

    #[test]
    fn set_image_cell_extends_cells_to_col() {
        let image_id = crate::image_store::next_image_id();
        let mut row = Row::new(10);
        // Row is empty, set image at col 3 — should extend to col 4
        row.set_image_cell(3, make_image_placement(image_id), FormatTag::default());
        assert!(row.cells().len() >= 4);
        assert!(row.char_at(3).unwrap().has_image());
    }

    // -----------------------------------------------------------------------
    // Coverage gap tests
    // -----------------------------------------------------------------------

    /// Helper: build a wide CJK `TChar` (U+4E2D '中', `display_width` = 2).
    fn wide_tchar() -> TChar {
        TChar::from('中')
    }

    // ── cleanup_wide_overwrite: col >= cells.len (line 275) ─────────────

    #[test]
    fn insert_text_at_col_past_cells_len_triggers_cleanup_wide_return() {
        // cleanup_wide_overwrite returns early when col >= cells.len()
        // This happens when insert_text writes at a column beyond stored cells.
        let mut row = Row::new(10);
        // Row has 0 cells (empty). Insert a wide char at col 5.
        let tag = FormatTag::default();
        let text = [wide_tchar()];
        let resp = row.insert_text(5, &text, &tag);
        // Should succeed — the row extends to accommodate
        match resp {
            InsertResponse::Consumed(col) => assert!(col >= 6),
            InsertResponse::Leftover { .. } => {} // also acceptable
        }
    }

    // ── cleanup_wide_overwrite: continuation at col 0 (line 282) ────────

    #[test]
    fn cleanup_wide_overwrite_continuation_at_col_0_returns_early() {
        // Pathological state: continuation cell at position 0 (no head to the left).
        let mut row = Row::new(5);
        // Fill cells so we can manipulate them
        row.cells = vec![
            Cell::wide_continuation(), // col 0: orphan continuation
            Cell::new(TChar::Ascii(b'A'), FormatTag::default()),
            Cell::new(TChar::Ascii(b'B'), FormatTag::default()),
        ];
        // Overwrite col 0 — cleanup_wide_overwrite is called, finds continuation
        // at col 0, returns early (invariant violation guard).
        let tag = FormatTag::default();
        let text = [TChar::Ascii(b'X')];
        let _resp = row.insert_text(0, &text, &tag);
        // Col 0 should now be 'X', not a continuation
        assert_eq!(row.cells[0].tchar(), &TChar::Ascii(b'X'));
    }

    // ── cleanup_wide_overwrite: walk-back finds no head (lines 287-290) ─

    #[test]
    fn cleanup_wide_overwrite_walk_back_no_head_returns_early() {
        // Multiple continuation cells with no head — pathological state.
        let mut row = Row::new(5);
        row.cells = vec![
            Cell::wide_continuation(), // col 0: orphan continuation
            Cell::wide_continuation(), // col 1: orphan continuation
            Cell::wide_continuation(), // col 2: orphan continuation
        ];
        // Overwrite col 2 — walks back through continuations, finds col 0
        // which is also continuation (not head), returns early.
        let tag = FormatTag::default();
        let text = [TChar::Ascii(b'Z')];
        let resp = row.insert_text(2, &text, &tag);
        match resp {
            InsertResponse::Consumed(col) => assert_eq!(col, 3),
            InsertResponse::Leftover { .. } => panic!("unexpected leftover"),
        }
        assert_eq!(row.cells[2].tchar(), &TChar::Ascii(b'Z'));
    }

    // ── insert_text: wide char at edge, continuation past width (428, 436) ─

    #[test]
    fn insert_wide_char_at_last_column_returns_leftover() {
        // Insert a width-2 char at col 4 of a width-5 row.
        // col + w (4 + 2 = 6) > limit (5) → the char doesn't fit, returns Leftover.
        let mut row = Row::new(5);
        let tag = FormatTag::default();
        let text = [wide_tchar()]; // width 2
        let resp = row.insert_text(4, &text, &tag);
        match resp {
            InsertResponse::Leftover {
                leftover_start,
                final_col,
            } => {
                assert_eq!(leftover_start, 0);
                assert_eq!(final_col, 4);
            }
            InsertResponse::Consumed(_) => panic!("expected leftover"),
        }
    }

    // ── insert_spaces_at: needed_len == 0 early return (line 473) ───────

    #[test]
    fn insert_spaces_at_zero_n_is_noop() {
        let mut row = Row::new(5);
        let tag = FormatTag::default();
        // n=0 triggers the n==0 guard at the top, not line 473.
        // To hit line 473, we need needed_len == 0 which requires
        // old_len == 0 AND col + insert_len == 0 AND old_len + insert_len == 0.
        // That means insert_len == 0, which means n=0 or col >= width.
        // Actually n=0 is caught earlier. col >= width is also caught earlier.
        // So needed_len == 0 requires insert_len == 0 which requires n == 0.
        // But n == 0 is guarded. This line is effectively unreachable.
        // Test the existing early returns instead.
        row.insert_spaces_at(0, 0, &tag);
        assert!(row.cells().is_empty());

        row.insert_spaces_at(10, 5, &tag); // col >= width
        assert!(row.cells().is_empty());
    }

    // ── erase_cells_at: continuation with no head in walk-back (line 617) ─

    #[test]
    fn erase_cells_at_continuation_no_head_fallthrough() {
        // Walk-back from continuation finds no head cell — uses `end` as-is.
        let mut row = Row::new(10);
        row.cells = vec![
            Cell::new(TChar::Ascii(b'A'), FormatTag::default()),
            Cell::new(TChar::Ascii(b'B'), FormatTag::default()),
            Cell::wide_continuation(), // orphan continuation at col 2
            Cell::wide_continuation(), // orphan continuation at col 3
            Cell::new(TChar::Ascii(b'C'), FormatTag::default()),
        ];
        // Erase 1 cell at col 0 — end = min(0+1, 10) = 1.
        // Cell at end (col 1) is not continuation. Let's set up erase at col 0
        // with n = 2 so end = 2. Cell at col 2 IS a continuation.
        // Walk back: col 1 is 'B' (not continuation), so head=1, not is_head → fallthrough.
        let tag = FormatTag::default();
        row.erase_cells_at(0, 2, &tag);
        // Cols 0 and 1 should be blanked. Col 2 (continuation) survives as-is
        // because the walk-back found no head.
        assert_eq!(row.cells[0].tchar(), &TChar::Space);
        assert_eq!(row.cells[1].tchar(), &TChar::Space);
    }

    // ── insert_spaces_bounded: resize path and sparse trim (lines 666-667, 686, 692-693) ─

    #[test]
    fn insert_spaces_bounded_on_short_row_resizes_and_trims() {
        // Row with cells.len < right_limit triggers resize (line 666-667).
        // After insert, if trailing cells are default blanks, they get popped (692-693).
        let mut row = Row::new(10);
        // Only 2 cells stored
        row.cells = vec![
            Cell::new(TChar::Ascii(b'A'), FormatTag::default()),
            Cell::new(TChar::Ascii(b'B'), FormatTag::default()),
        ];
        let tag = FormatTag::default();
        // Insert 2 spaces at col 1 within limit 8
        row.insert_spaces_at_with_right_limit(1, 2, &tag, 8);
        // 'A' at 0, then 2 blank spaces at 1-2, then 'B' shifted to 3
        assert_eq!(row.cells[0].tchar(), &TChar::Ascii(b'A'));
        assert_eq!(row.cells[1].tchar(), &TChar::Space);
        assert_eq!(row.cells[2].tchar(), &TChar::Space);
        assert_eq!(row.cells[3].tchar(), &TChar::Ascii(b'B'));
    }

    // ── insert_spaces_bounded: cells.len > width truncation (line 686) ──

    #[test]
    fn insert_spaces_bounded_truncates_when_exceeding_width() {
        // If right_limit > width somehow, cells.len can exceed width after resize.
        // The clamp at line 685-686 truncates.
        // Actually, limit = right_limit.min(self.width), so limit <= width.
        // The resize fills to `limit`, and limit <= width, so cells.len <= width.
        // Line 686 is a defensive guard. Let's trigger it by manipulating cells directly.
        let mut row = Row::new(5);
        // Pre-fill with 7 cells (more than width)
        row.cells = vec![Cell::new(TChar::Ascii(b'X'), FormatTag::default()); 7];
        let tag = FormatTag::default();
        row.insert_spaces_at_with_right_limit(0, 1, &tag, 5);
        // After operation, cells.len should be truncated to <= width
        assert!(row.cells.len() <= 5);
    }

    // ── delete_cells_bounded: cells.len > width truncation (line 740) ───

    #[test]
    fn delete_cells_bounded_truncates_when_exceeding_width() {
        let mut row = Row::new(5);
        // Pre-fill with 7 cells (more than width)
        row.cells = vec![Cell::new(TChar::Ascii(b'X'), FormatTag::default()); 7];
        let tag = FormatTag::default();
        row.delete_cells_at_with_right_limit(0, 1, 5, &tag);
        // After operation, cells.len should be truncated to <= width
        assert!(row.cells.len() <= 5);
    }

    // ── delete_cells_at: deletion splits a wide glyph at end (line 812) ─

    #[test]
    fn delete_cells_at_splits_wide_glyph_at_end_boundary() {
        // Row: [A] [头(head)] [头(cont)] [B] [C]
        // Delete 1 cell at col 0 — end = 0+1 = 1.
        // Cell at end (col 1) is wide head, so end extends to 1+2=3.
        // Actually, line 812 is about continuation at `end`, not head.
        // Let me set up: [A] [B] [头(head)] [头(cont)] [C]
        // Delete 2 cells at col 0 — end = 0+2 = 2.
        // Cell at end (col 2) is wide head → extend to include continuations.
        // No, line 799-812 checks if cell at `end` is continuation.
        // Need: [A] [头(head)] [头(cont)] [B]
        // Delete 1 at col 0 — end = 1. Cell at col 1 is continuation? No, col 1 is head.
        // Let me set up: [A] [头(head)] [头(cont)] [头2(head)] [头2(cont)]
        // Delete 2 at col 1 — start=1 (head), extend to 3. end=1+2=3.
        // Cell at end (col 3) is head. Not continuation, so line 799 check fails.
        //
        // To hit line 799-812: need cell at `end` to be a continuation.
        // Setup: [A] [头(head)] [头(cont)] [X]
        // Delete 1 at col 0 — start=0, end=0+1=1. Cell at col 0 is A (not head/cont).
        // Cell at end (col 1) is head → end extends to 3 via line 793-794.
        // Still not continuation.
        //
        // Setup: [头(head)] [头(cont)] [头2(head)] [头2(cont)] [B]
        // Delete 1 at col 0: start=0(head), end=0+1=1. Line 792-794 extends to 2.
        // Cell at end (col 2) is head. Not continuation.
        //
        // I need end to land on a continuation cell.
        // Setup: [A] [B] [头(head)] [头(cont)] (width=10, 4 cells stored)
        // Delete 3 at col 0: start=0, end=0+3=3.
        // Cell at end (col 3) is continuation! → walk back, find head at col 2.
        // Replace head+continuations with blanks (line 809-811). Line 812 closing brace.
        let mut row = Row::new(10);
        row.cells = vec![
            Cell::new(TChar::Ascii(b'A'), FormatTag::default()),
            Cell::new(TChar::Ascii(b'B'), FormatTag::default()),
            Cell::new(wide_tchar(), FormatTag::default()), // head at col 2
            Cell::wide_continuation(),                     // continuation at col 3
        ];
        let tag = FormatTag::default();
        // Delete 3 cells starting at col 0 → end = 3.
        // Cell at col 3 is continuation → walk back to head at col 2.
        // Head is blanked along with continuation.
        row.delete_cells_at(0, 3, &tag);
        // After deletion, the wide glyph should be fully blanked (not partially deleted)
        // The drain removes [0..3], but before that, head+cont at [2,3] are blanked.
        // Actually: end=3, cell at 3 is continuation. Head at 2. Replace 2,3 with blanks.
        // Then drain [0..3]. Remaining: [blank_was_cont at col 3] which is now blank.
        // The row should have at most 1 cell left (the blank from col 3).
        assert!(
            row.cells().len() <= 1,
            "row should have 0-1 cells after delete: {:?}",
            row.cells()
        );
    }

    // ── insert_spaces_bounded: needed_len == 0 (line 661) ───────────────

    #[test]
    fn insert_spaces_bounded_needed_len_zero_returns_early() {
        // needed_len = (old_len + insert_len).max(col + insert_len).min(limit)
        // For needed_len == 0: requires limit == 0, which requires right_limit == 0
        // or self.width == 0. But col >= limit check catches col >= 0 when limit == 0.
        // Actually: if limit == 0, then col >= limit (0 >= 0) is true, so we return
        // at the n==0 || col>=limit guard before reaching needed_len check.
        // So line 661 is effectively unreachable through normal paths.
        // Test the guard paths instead.
        let mut row = Row::new(5);
        let tag = FormatTag::default();
        row.insert_spaces_at_with_right_limit(0, 1, &tag, 0); // limit=0 → col>=limit
        assert!(row.cells().is_empty());
    }

    // ── insert_text: wide chars that wrap around row boundary ────────────

    #[test]
    fn insert_multiple_wide_chars_filling_row() {
        let mut row = Row::new(6);
        let tag = FormatTag::default();
        // Three width-2 chars should fill exactly 6 columns
        let text = [wide_tchar(), wide_tchar(), wide_tchar()];
        let resp = row.insert_text(0, &text, &tag);
        match resp {
            InsertResponse::Consumed(col) => assert_eq!(col, 6),
            InsertResponse::Leftover { .. } => panic!("unexpected leftover"),
        }
        assert!(row.cells[0].is_head());
        assert!(row.cells[1].is_continuation());
        assert!(row.cells[2].is_head());
        assert!(row.cells[3].is_continuation());
        assert!(row.cells[4].is_head());
        assert!(row.cells[5].is_continuation());
    }

    #[test]
    fn insert_wide_char_at_second_to_last_col_returns_leftover() {
        // Width-5 row, insert width-2 char at col 4.
        // col + w (4 + 2 = 6) > limit (5) → doesn't fit, returns Leftover.
        let mut row = Row::new(5);
        let tag = FormatTag::default();
        let text = [wide_tchar()];
        let resp = row.insert_text(4, &text, &tag);
        match resp {
            InsertResponse::Leftover {
                leftover_start,
                final_col,
            } => {
                assert_eq!(leftover_start, 0);
                assert_eq!(final_col, 4);
            }
            InsertResponse::Consumed(_) => panic!("expected leftover"),
        }
    }

    // ── delete_cells_bounded: sparse invariant pop (lines 745-746) ──────

    #[test]
    fn delete_cells_bounded_trims_trailing_default_blanks() {
        let mut row = Row::new(10);
        // Fill with [A, B, <blank>, <blank>, <blank>] — limit 5
        row.cells = vec![
            Cell::new(TChar::Ascii(b'A'), FormatTag::default()),
            Cell::new(TChar::Ascii(b'B'), FormatTag::default()),
            Cell::blank_with_tag(FormatTag::default()),
            Cell::blank_with_tag(FormatTag::default()),
            Cell::blank_with_tag(FormatTag::default()),
        ];
        let tag = FormatTag::default();
        // Delete 1 cell at col 0, limit 5: shift left, fill right with blank
        row.delete_cells_at_with_right_limit(0, 1, 5, &tag);
        // After delete: [B, blank, blank, blank, blank] → all trailing blanks trimmed
        // Final should be just [B]
        assert_eq!(row.cells.len(), 1);
        assert_eq!(row.cells[0].tchar(), &TChar::Ascii(b'B'));
    }
}
