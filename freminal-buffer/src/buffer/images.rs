// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Inline image placement and management for the terminal buffer.
//!
//! This module contains methods for placing, clearing, and querying image
//! cells within the buffer, including support for the Kitty graphics protocol
//! and iTerm2 inline images.

use freminal_common::buffer_states::{buffer_type::BufferType, format_tag::FormatTag};

use crate::{
    image_store::{ImagePlacement, ImageProtocol, ImageStore, InlineImage},
    row::{RowJoin, RowOrigin},
};

use super::Buffer;

impl Buffer {
    /// Advance the cursor by one column, wrapping to the next line if needed.
    ///
    /// Used after inserting a placeholder image cell that occupies one column.
    pub const fn advance_cursor_one(&mut self) {
        if self.cursor.pos.x + 1 < self.width {
            self.cursor.pos.x += 1;
        }
        // If at the rightmost column, don't wrap automatically — let the
        // next character insertion handle wrap/scroll as normal.
    }

    /// Set an image cell at a specific (row, col) position in the buffer.
    ///
    /// Also invalidates the corresponding row cache entry.  Used by
    /// `TerminalHandler` for Kitty Unicode placeholder cells.
    pub fn set_image_cell_at(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        placement: ImagePlacement,
        tag: FormatTag,
    ) {
        if row_idx < self.rows.len() {
            // Check if the old cell already had an image (avoid double-counting).
            let had_image = self.rows[row_idx]
                .cells()
                .get(col_idx)
                .is_some_and(crate::cell::Cell::has_image);
            self.rows[row_idx].set_image_cell(col_idx, placement, tag);
            if !had_image {
                self.image_cell_count += 1;
            }
            if row_idx < self.row_cache.len() {
                self.row_cache[row_idx] = None;
            }
        }
    }

    /// Access the image store (read-only).
    #[must_use]
    pub const fn image_store(&self) -> &ImageStore {
        &self.image_store
    }

    /// Access the image store (mutable).
    pub const fn image_store_mut(&mut self) -> &mut ImageStore {
        &mut self.image_store
    }

    /// Clear all image placements from every cell in the buffer.
    pub fn clear_all_image_placements(&mut self) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.has_image() {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear all image placements for a specific image ID from every cell.
    pub fn clear_image_placements_by_id(&mut self, image_id: u64) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_id == image_id)
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at the current cursor row and all rows after.
    ///
    /// Used by Kitty `d=c` (at cursor) and `d=C` (at cursor and after) delete
    /// targets. Clears every image cell from the cursor row to the end of
    /// the buffer.
    pub fn clear_image_placements_at_cursor_and_after(&mut self) {
        let start_row = self.cursor.pos.y;
        let mut cleared = 0usize;
        for i in start_row..self.rows.len() {
            let row = &mut self.rows[i];
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.has_image() {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at the current cursor position only (single row).
    pub fn clear_image_placements_at_cursor(&mut self) {
        let row_idx = self.cursor.pos.y;
        if row_idx >= self.rows.len() {
            return;
        }
        let row = &mut self.rows[row_idx];
        let mut cleared = 0usize;
        for cell in row.cells_mut() {
            if cell.has_image() {
                cell.clear_image();
                cleared += 1;
            }
        }
        if cleared > 0 {
            row.dirty = true;
            if row_idx < self.row_cache.len() {
                self.row_cache[row_idx] = None;
            }
            self.image_cell_count -= cleared;
        }
    }

    /// Returns `true` if any cell in the buffer has an image placement.
    ///
    /// O(1) — backed by the `image_cell_count` counter.
    #[must_use]
    pub const fn has_any_image_cell(&self) -> bool {
        self.image_cell_count > 0
    }

    /// Clear all image placements whose Kitty image number matches `number`.
    pub fn clear_image_placements_by_number(&mut self, number: u32) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_number == Some(number))
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear image placements at a specific cell position.
    pub fn clear_image_placements_at_cell(&mut self, row: usize, col: usize) {
        if row >= self.rows.len() {
            return;
        }
        let id = {
            let cells = self.rows[row].cells();
            if col < cells.len() {
                cells[col].image_placement().map(|p| p.image_id)
            } else {
                None
            }
        };
        if let Some(id) = id {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear image placements at a specific cell and all cells after it.
    pub fn clear_image_placements_at_cell_and_after(&mut self, row: usize, col: usize) {
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for r in row..self.rows.len() {
            let start = if r == row { col } else { 0 };
            let cells = self.rows[r].cells();
            for cell in cells.iter().skip(start) {
                if let Some(placement) = cell.image_placement() {
                    let id = placement.image_id;
                    if !ids_to_clear.contains(&id) {
                        ids_to_clear.push(id);
                    }
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements that intersect the given column.
    pub fn clear_image_placements_in_column(&mut self, col: usize) {
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for row in &self.rows {
            let cells = row.cells();
            if col < cells.len()
                && let Some(placement) = cells[col].image_placement()
            {
                let id = placement.image_id;
                if !ids_to_clear.contains(&id) {
                    ids_to_clear.push(id);
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements that intersect the given row.
    pub fn clear_image_placements_in_row(&mut self, row: usize) {
        if row >= self.rows.len() {
            return;
        }
        let mut ids_to_clear: Vec<u64> = Vec::new();
        let cells = self.rows[row].cells();
        for cell in cells {
            if let Some(placement) = cell.image_placement() {
                let id = placement.image_id;
                if !ids_to_clear.contains(&id) {
                    ids_to_clear.push(id);
                }
            }
        }
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }

    /// Clear all image placements with the given z-index.
    pub fn clear_image_placements_by_z_index(&mut self, z: i32) {
        let mut cleared = 0usize;
        for (i, row) in self.rows.iter_mut().enumerate() {
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell.image_placement().is_some_and(|p| p.z_index == z) {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if i < self.row_cache.len() {
                    self.row_cache[i] = None;
                }
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Check whether a text insertion at `[col .. col + text_len)` on `row_idx`
    /// would overwrite any image cells.  If so, clear **all** cells of each
    /// affected image across the entire buffer.
    ///
    /// Only the cells in `[col .. col + text_len)` are inspected — images
    /// outside this range on the same row are not affected.
    ///
    /// Images are treated as atomic: overwriting even a single cell of an
    /// image invalidates the whole image.  This matches the behaviour of
    /// other terminal emulators (`iTerm2`, `WezTerm`, Kitty).
    pub(in crate::buffer) fn clear_images_overwritten_by_text(
        &mut self,
        row_idx: usize,
        col: usize,
        text_len: usize,
    ) {
        self.collect_and_clear_image_ids_in_rows(
            row_idx,
            row_idx + 1,
            Some(col),
            Some(col + text_len),
        );
    }

    /// Place an inline image at the current cursor position.
    ///
    /// The image is stored in the central `ImageStore` and cells in the
    /// rectangular region `[cursor_y .. cursor_y + display_rows) ×
    /// [cursor_x .. cursor_x + display_cols)` are filled with
    /// `ImagePlacement` references.
    ///
    /// After placement the cursor is moved to the row immediately below
    /// the image (or the last visible row if the image extends to the
    /// bottom), at column 0 — matching iTerm2 behaviour.
    ///
    /// If the image extends beyond the right edge of the terminal, it is
    /// clipped to the terminal width (cells beyond the edge are not placed).
    /// If the image extends below the visible area, new rows are created
    /// (scrolling if necessary in the primary buffer).
    pub fn place_image(
        &mut self,
        image: InlineImage,
        scroll_offset: usize,
        protocol: ImageProtocol,
        image_number: Option<u32>,
        placement_id: Option<u32>,
        z_index: i32,
    ) -> usize {
        let image_id = image.id;
        let display_cols = image.display_cols;
        let display_rows = image.display_rows;

        // Store the image centrally.
        self.image_store.insert(image);

        let start_col = self.cursor.pos.x;

        // Clamp display_cols to not exceed terminal width.
        let effective_cols = display_cols.min(self.width.saturating_sub(start_col));

        // Before placing new image cells, clear any existing image cells in
        // the column range from the cursor row downward.  This handles the
        // common case where a new (possibly smaller) image replaces an old
        // (possibly larger) one at the same position — without this, stale
        // cells from the old image persist below the new one.
        let clear_end_col = start_col + effective_cols;
        for row_idx in self.cursor.pos.y..self.rows.len() {
            let row = &mut self.rows[row_idx];
            let mut changed = false;
            for col in start_col..clear_end_col.min(row.max_width()) {
                if let Some(cell) = row.cells_mut().get_mut(col)
                    && cell.has_image()
                {
                    cell.clear_image();
                    self.image_cell_count -= 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                if row_idx < self.row_cache.len() {
                    self.row_cache[row_idx] = None;
                }
            }
        }

        let mut current_offset = scroll_offset;

        // Place image cells row by row.
        //
        // We track `base_row` which starts at the cursor's current row and
        // shifts downward as rows are created.  Unlike `scroll_up()`, we
        // grow the buffer by pushing new rows and then let
        // `enforce_scrollback_limit` trim excess from the top — this avoids
        // the infinite-loop problem where `scroll_up()` keeps rows.len()
        // constant.
        let mut base_row = self.cursor.pos.y;

        for img_row in 0..display_rows {
            let target_row = base_row + img_row;

            // Ensure the target row exists.
            while target_row >= self.rows.len() {
                self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);
            }

            let row = &mut self.rows[target_row];
            row.dirty = true;

            // Invalidate the row cache for this row.
            if target_row < self.row_cache.len() {
                self.row_cache[target_row] = None;
            }

            let mut placed_count = 0usize;
            for img_col in 0..effective_cols {
                let col = start_col + img_col;
                if col >= self.width {
                    break;
                }
                let placement = ImagePlacement {
                    image_id,
                    col_in_image: img_col,
                    row_in_image: img_row,
                    protocol,
                    image_number,
                    placement_id,
                    z_index,
                };
                row.set_image_cell(col, placement, self.current_tag.clone());
                placed_count += 1;
            }
            self.image_cell_count += placed_count;
        }

        // Enforce scrollback limit — this may drain rows from the top.
        if self.kind == BufferType::Primary {
            let rows_before = self.rows.len();
            current_offset = self.enforce_scrollback_limit(current_offset);
            let drained = rows_before - self.rows.len();
            // Adjust base_row for the drained rows.
            base_row = base_row.saturating_sub(drained);
        }

        // Move cursor below the image, column 0 (iTerm2 behaviour).
        let final_row = base_row + display_rows;
        if final_row < self.rows.len() {
            self.cursor.pos.y = final_row;
        } else {
            self.cursor.pos.y = self.rows.len().saturating_sub(1);
        }
        self.cursor.pos.x = 0;

        self.debug_assert_invariants();
        current_offset
    }
}
