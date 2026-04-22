// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Scroll region management and scrolling operations for [`Buffer`].
//!
//! Covers DECSTBM scroll region setup (`set_scroll_region`,
//! `reset_scroll_region_to_full`), region-relative scrolling
//! (`scroll_region_up_n`, `scroll_region_down_n`, `scroll_region_up_for_wrap`),
//! low-level row/column shift primitives (`scroll_slice_up`,
//! `scroll_slice_down`, `scroll_slice_up_columns`, `scroll_slice_down_columns`),
//! user-facing scrollback navigation (`scroll_back`, `scroll_forward`,
//! `scroll_to_bottom`, `scroll_up`), and visible-window helpers
//! (`visible_rows`, `visible_line_widths`, `visible_window_start`,
//! `any_visible_dirty`, `visible_image_placements`, `has_visible_images`,
//! `max_scroll_offset`, `erase_scrollback`).

use freminal_common::buffer_states::{buffer_type::BufferType, format_tag::FormatTag};

use crate::{
    cell::Cell,
    image_store::ImagePlacement,
    row::{Row, RowJoin, RowOrigin},
};

use crate::buffer::Buffer;

impl Buffer {
    /// Return the [`LineWidth`] for each row in the visible window.
    ///
    /// The returned vector has `min(term_height, row_count)` entries, one per
    /// visible row in top-to-bottom order.  Used by `build_snapshot` to thread
    /// per-row line-width data through to the renderer.
    #[must_use]
    pub fn visible_line_widths(&self, scroll_offset: usize) -> Vec<crate::row::LineWidth> {
        let start = self.visible_window_start(scroll_offset);
        let end = (start + self.height).min(self.rows.len());
        self.rows[start..end].iter().map(|r| r.line_width).collect()
    }

    /// Get the rows that should be *visually displayed* in the GUI.
    ///
    /// Contract:
    /// - Returns a contiguous slice of `self.rows`.
    /// - `visible_rows(scroll_offset).len() <= self.height`.
    /// - When `self.rows.len() <= self.height`, the slice is the entire buffer.
    /// - When `scroll_offset == 0`, the slice is the last `height` rows
    ///   (the live bottom).
    /// - When `scroll_offset > 0`, the slice is shifted upwards into
    ///   scrollback, clamped so it never goes before the oldest row.
    /// - Never allocates; always borrows from `self.rows`.
    ///
    /// `scroll_offset` is owned by the caller (e.g. `ViewState`) and is never
    /// stored inside `Buffer`.
    #[must_use]
    pub fn visible_rows(&self, scroll_offset: usize) -> &[Row] {
        if self.rows.is_empty() {
            return &[];
        }

        let total = self.rows.len();
        let h = self.height;

        // Clamp scroll_offset within bounds.
        let max_offset = self.max_scroll_offset();
        let offset = scroll_offset.min(max_offset);

        let start = total.saturating_sub(h + offset);
        let end = start + h;

        &self.rows[start.min(total)..end.min(total)]
    }

    pub(in crate::buffer) fn reset_scroll_region_to_full(&mut self) {
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);
        // Reset cursor to home position (screen row 0, col 0).
        // Use set_cursor_pos so rows are created if they don't exist yet.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Set DECSTBM scroll region (1-based inclusive).
    /// If invalid, resets to full screen.
    pub fn set_scroll_region(&mut self, top1: usize, bottom1: usize) {
        // 0 or missing → ignore and reset
        if top1 == 0 || bottom1 == 0 {
            self.reset_scroll_region_to_full();
            return;
        }

        // Convert to 0-based
        let top = top1.saturating_sub(1);
        let bottom = bottom1.saturating_sub(1);

        // Validate
        if top >= bottom || bottom >= self.height {
            self.reset_scroll_region_to_full();
            return;
        }

        self.scroll_region_top = top;
        self.scroll_region_bottom = bottom;

        // DECSTBM always homes the cursor to (0, 0).
        // When DECOM is enabled, set_cursor_pos interprets y=0 as
        // scroll_region_top.  When DECOM is disabled, y=0 is screen top.
        // Both cases are handled correctly by a single call.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Compute the index into `self.rows` of the first visible row for a given
    /// `scroll_offset`.
    ///
    /// `scroll_offset` is always the caller's `ViewState` value — `Buffer`
    /// itself never stores it.  The PTY thread always passes `0`; the GUI
    /// passes the value from `ViewState`.
    #[must_use]
    pub(in crate::buffer) fn visible_window_start(&self, scroll_offset: usize) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let total = self.rows.len();
        let h = self.height.min(total);
        let offset = scroll_offset.min(self.max_scroll_offset());

        total.saturating_sub(h + offset)
    }

    /// Return `true` if any row in the visible window is dirty (needs re-flattening).
    ///
    /// The PTY thread calls this with `scroll_offset = 0`.  A `false` result means
    /// the cached flat representation for every visible row is still valid, so
    /// `build_snapshot` can skip the flatten step entirely and reuse the previous
    /// `visible_chars` / `visible_tags` vectors.
    #[must_use]
    pub fn any_visible_dirty(&self, scroll_offset: usize) -> bool {
        if self.rows.is_empty() || self.height == 0 {
            return false;
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        self.rows[vis_start..vis_end].iter().any(|r| r.dirty)
    }

    /// Extract image placements for all cells in the visible window.
    ///
    /// Returns a flat `Vec` of `Option<ImagePlacement>`, one entry per cell,
    /// in row-major order (row 0, col 0..width; row 1, col 0..width; …).
    ///
    /// `None` means the cell carries no image data.
    /// `Some(placement)` means the cell is part of an inline image.
    ///
    /// The length of the returned `Vec` is `height * width` (clamped to the
    /// actual number of visible rows × terminal width), matching the layout
    /// of `visible_chars` so the caller can index them in parallel.
    #[must_use]
    pub fn visible_image_placements(&self, scroll_offset: usize) -> Vec<Option<ImagePlacement>> {
        if self.rows.is_empty() || self.height == 0 || self.width == 0 {
            return Vec::new();
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        let mut out = Vec::with_capacity((vis_end - vis_start) * self.width);
        for row in &self.rows[vis_start..vis_end] {
            let cells = row.cells();
            for col in 0..self.width {
                let placement = cells.get(col).and_then(|c| c.image_placement()).cloned();
                out.push(placement);
            }
        }
        out
    }

    /// Returns `true` if any cell in the visible window carries an image
    /// placement.  Used by `build_snapshot` to cheaply decide whether to
    /// include image data in the snapshot.
    ///
    /// Short-circuits in O(1) when the buffer has no image cells at all
    /// (the overwhelmingly common case).
    #[must_use]
    pub fn has_visible_images(&self, scroll_offset: usize) -> bool {
        // Fast path: no image cells anywhere in the buffer.
        if self.image_cell_count == 0 {
            return false;
        }

        if self.rows.is_empty() || self.height == 0 {
            return false;
        }
        let vis_start = self.visible_window_start(scroll_offset);
        let vis_end = (vis_start + self.height).min(self.rows.len());
        self.rows[vis_start..vis_end]
            .iter()
            .any(|r| r.cells().iter().any(|c| c.image_placement().is_some()))
    }

    /// Convert DECSTBM region (screen coords) into buffer row indices (rows[])
    ///
    /// Ensures `self.rows` is extended to at least `height` entries so that
    /// the returned indices always point to real rows.  Without this, an early
    /// buffer (`rows.len()` < height) would clamp both top and bottom to the
    /// same index, causing every scroll operation to silently no-op.
    pub(in crate::buffer) fn scroll_region_rows(&mut self) -> (usize, usize) {
        let start = self.visible_window_start(0);
        let required = start + self.scroll_region_bottom + 1;
        while self.rows.len() < required {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }
        let top = start + self.scroll_region_top;
        let bottom = start + self.scroll_region_bottom;
        (top, bottom)
    }

    /// Scroll DECSTBM region UP by 1 (primary buffer)
    pub(in crate::buffer) fn scroll_region_up_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_up(t, b);
        }
    }

    /// Scroll DECSTBM region DOWN by 1 (primary buffer)
    pub(in crate::buffer) fn scroll_region_down_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_down(t, b);
        }
    }

    /// Check whether the cursor is at the bottom margin of the DECSTBM
    /// scroll region.  Used by `insert_text` to decide whether a right-margin
    /// wrap should scroll the region instead of advancing past it.
    pub(in crate::buffer) fn is_cursor_at_scroll_region_bottom(&self) -> bool {
        match self.kind {
            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                sy == self.scroll_region_bottom
            }
            BufferType::Alternate => self.cursor.pos.y == self.scroll_region_bottom,
        }
    }

    /// Scroll the DECSTBM region up by one line during an autowrap at the
    /// bottom margin.  Handles both Primary (with scrollback) and Alternate
    /// (fixed grid) buffer types.
    pub(in crate::buffer) fn scroll_region_up_for_wrap(&mut self) {
        match self.kind {
            BufferType::Primary => {
                self.scroll_region_up_primary();
            }
            BufferType::Alternate => {
                if self.scroll_region_top < self.scroll_region_bottom {
                    self.scroll_slice_up(self.scroll_region_top, self.scroll_region_bottom);
                }
            }
        }
    }

    /// SU — Scroll the scroll region UP by `n` lines.
    /// Content moves up; blank lines appear at the bottom.
    /// If no scroll region is set, operates on the whole screen.
    pub fn scroll_region_up_n(&mut self, n: usize) {
        let (t, b) = self.scroll_region_rows();
        if t >= b {
            return;
        }
        // Region indices are inclusive, so region has (b - t + 1) rows.
        let region_size = b - t + 1;
        let clamped = n.min(region_size);
        for _ in 0..clamped {
            self.scroll_slice_up(t, b);
        }
    }

    /// SD — Scroll the scroll region DOWN by `n` lines.
    /// Content moves down; blank lines appear at the top.
    /// If no scroll region is set, operates on the whole screen.
    pub fn scroll_region_down_n(&mut self, n: usize) {
        let (t, b) = self.scroll_region_rows();
        if t >= b {
            return;
        }
        // Region indices are inclusive, so region has (b - t + 1) rows.
        let region_size = b - t + 1;
        let clamped = n.min(region_size);
        for _ in 0..clamped {
            self.scroll_slice_down(t, b);
        }
    }

    /// Scroll a contiguous vertical slice [first, last] UP by one line.
    /// Rows outside that range are untouched. New bottom line is blank.
    pub(in crate::buffer) fn scroll_slice_up(&mut self, first: usize, last: usize) {
        if first >= last {
            return;
        }
        if last >= self.rows.len() {
            return;
        }

        for row_idx in first..last {
            let next = self.rows[row_idx + 1].clone();
            self.rows[row_idx] = next;
            // Rotate the cache entry in lockstep: a moved row keeps its cached
            // flat representation (it hasn't changed content, only position).
            self.row_cache[row_idx] = self.row_cache[row_idx + 1].take();
        }

        // The original rows[last] was not shifted (the loop only copies
        // rows[row_idx+1] into rows[row_idx] for row_idx in first..last).
        // It is now replaced with a blank row; deduct any image cells it held.
        self.image_cell_count -= self.rows[last].count_image_cells();
        let new_row = Row::new(self.width);
        // Scroll-created blank rows use default background (no BCE).
        // See `push_row` comment for rationale.
        self.rows[last] = new_row;
        // New blank row at `last` — no cached representation yet.
        self.row_cache[last] = None;
    }

    /// Scroll a contiguous vertical slice [first, last] DOWN by one line.
    /// Rows outside that range are untouched. New top line is blank.
    pub(in crate::buffer) fn scroll_slice_down(&mut self, first: usize, last: usize) {
        if first >= last {
            return;
        }
        if last >= self.rows.len() {
            return;
        }

        for row_idx in (first + 1..=last).rev() {
            let prev = self.rows[row_idx - 1].clone();
            self.rows[row_idx] = prev;
            // Rotate the cache entry in lockstep.
            self.row_cache[row_idx] = self.row_cache[row_idx - 1].take();
        }

        // The original rows[first] was not shifted (the loop only copies
        // rows[row_idx-1] into rows[row_idx] for row_idx in first+1..=last).
        // It is now replaced with a blank row; deduct any image cells it held.
        self.image_cell_count -= self.rows[first].count_image_cells();
        let new_row = Row::new(self.width);
        // Scroll-created blank rows use default background (no BCE).
        // See `push_row` comment for rationale.
        self.rows[first] = new_row;
        // New blank row at `first` — no cached representation yet.
        self.row_cache[first] = None;
    }

    /// Column-selective scroll-up: shifts cells within `[left_col, right_col]`
    /// on rows `[first, last]` up by one, without touching cells outside that
    /// horizontal range.  Used by `insert_lines` / `delete_lines` when DECLRMM
    /// is active.
    pub(in crate::buffer) fn scroll_slice_up_columns(
        &mut self,
        first: usize,
        last: usize,
        left_col: usize,
        right_col: usize,
    ) {
        if first >= last || last >= self.rows.len() || left_col > right_col {
            return;
        }
        // Sweep image cells in the destination columns [left_col, right_col] across
        // all rows [first, last] before any cells are overwritten or erased.
        // This covers both the copy-overwrite path (rows first..last) and the
        // final erase on rows[last].
        //
        // After the sweep, count remaining image cells (e.g. Kitty) in the
        // affected range before the copy/erase so we can compute the net
        // change after the operation.
        let images_before = if self.image_cell_count > 0 {
            self.collect_and_clear_image_ids_in_rows(
                first,
                last + 1,
                Some(left_col),
                Some(right_col + 1),
            );
            (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum::<usize>()
        } else {
            0
        };
        let tag = self.current_tag.clone();
        for row_idx in first..last {
            // Copy cells [left_col, right_col] from row_idx+1 into row_idx.
            // Use resolve_cell to handle sparse rows correctly — columns
            // beyond the stored cell count are treated as implicit blanks.
            let src_cells: Vec<_> = (left_col..=right_col)
                .map(|col| self.rows[row_idx + 1].resolve_cell(col))
                .collect();
            let row = &mut self.rows[row_idx];
            // Ensure storage.
            if row.cells_mut().len() < right_col + 1 {
                let width = row.width();
                while row.cells_mut().len() < (right_col + 1).min(width) {
                    row.cells_mut_push(Cell::blank_with_tag(FormatTag::default()));
                }
            }
            for (offset, cell) in src_cells.into_iter().enumerate() {
                let dst = left_col + offset;
                if dst <= right_col && dst < row.cells().len() {
                    row.cells_mut()[dst] = cell;
                }
            }
            row.mark_dirty();
            self.row_cache[row_idx] = None;
        }
        // Blank [left_col, right_col] on the last row.
        let row = &mut self.rows[last];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.row_cache[last] = None;

        // Adjust image_cell_count for any images lost during the shift/erase.
        if images_before > 0 {
            let images_after: usize = (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum();
            self.image_cell_count -= images_before.saturating_sub(images_after);
        }
    }

    /// Column-selective scroll-down: shifts cells within `[left_col, right_col]`
    /// on rows `[first, last]` down by one, without touching cells outside that
    /// horizontal range.  Used by `insert_lines` / `delete_lines` when DECLRMM
    /// is active.
    pub(in crate::buffer) fn scroll_slice_down_columns(
        &mut self,
        first: usize,
        last: usize,
        left_col: usize,
        right_col: usize,
    ) {
        if first >= last || last >= self.rows.len() || left_col > right_col {
            return;
        }
        // Sweep image cells in the destination columns [left_col, right_col] across
        // all rows [first, last] before any cells are overwritten or erased.
        // This covers both the copy-overwrite path (rows first+1..=last) and the
        // final erase on rows[first].
        let images_before = if self.image_cell_count > 0 {
            self.collect_and_clear_image_ids_in_rows(
                first,
                last + 1,
                Some(left_col),
                Some(right_col + 1),
            );
            (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum::<usize>()
        } else {
            0
        };
        let tag = self.current_tag.clone();
        for row_idx in (first + 1..=last).rev() {
            // Copy cells [left_col, right_col] from row_idx-1 into row_idx.
            // Use resolve_cell to handle sparse rows correctly — columns
            // beyond the stored cell count are treated as implicit blanks.
            let src_cells: Vec<_> = (left_col..=right_col)
                .map(|col| self.rows[row_idx - 1].resolve_cell(col))
                .collect();
            let row = &mut self.rows[row_idx];
            if row.cells_mut().len() < right_col + 1 {
                let width = row.width();
                while row.cells_mut().len() < (right_col + 1).min(width) {
                    row.cells_mut_push(Cell::blank_with_tag(FormatTag::default()));
                }
            }
            for (offset, cell) in src_cells.into_iter().enumerate() {
                let dst = left_col + offset;
                if dst <= right_col && dst < row.cells().len() {
                    row.cells_mut()[dst] = cell;
                }
            }
            row.mark_dirty();
            self.row_cache[row_idx] = None;
        }
        // Blank [left_col, right_col] on the first row.
        let row = &mut self.rows[first];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.row_cache[first] = None;

        // Adjust image_cell_count for any images lost during the shift/erase.
        if images_before > 0 {
            let images_after: usize = (first..=last)
                .map(|i| self.rows[i].count_image_cells_in_range(left_col, right_col + 1))
                .sum();
            self.image_cell_count -= images_before.saturating_sub(images_after);
        }
    }

    // ----------------------------------------------------------
    // Scrollback: only valid in the PRIMARY buffer
    // ----------------------------------------------------------

    /// How many lines above the live bottom the user can scroll.
    #[must_use]
    pub const fn max_scroll_offset(&self) -> usize {
        if self.rows.len() <= self.height {
            0
        } else {
            self.rows.len() - self.height
        }
    }

    /// Compute a new scroll offset after scrolling upward by `lines`.
    ///
    /// Alternate buffer always returns 0 (no scrollback).
    /// The caller is responsible for storing the returned value into `ViewState`.
    #[must_use]
    pub fn scroll_back(&self, scroll_offset: usize, lines: usize) -> usize {
        if self.kind != BufferType::Primary {
            return 0; // Alternate buffer: no scrollback
        }

        let max = self.max_scroll_offset();
        if max == 0 {
            return 0;
        }

        (scroll_offset + lines).min(max)
    }

    /// Compute a new scroll offset after scrolling downward by `lines`.
    ///
    /// The caller is responsible for storing the returned value into `ViewState`.
    #[must_use]
    pub fn scroll_forward(&self, scroll_offset: usize, lines: usize) -> usize {
        if self.kind != BufferType::Primary {
            return 0;
        }

        scroll_offset.saturating_sub(lines)
    }

    /// Returns `0` — the scroll offset for the live bottom view.
    ///
    /// Provided as a convenience so call sites read clearly.
    #[must_use]
    pub const fn scroll_to_bottom() -> usize {
        0
    }

    /// Scroll the visible window up by one row, discarding the top row and appending a blank row at
    /// the bottom.
    ///
    /// In the primary buffer the cursor row index is also decremented to follow the visible window.
    pub fn scroll_up(&mut self) {
        // Deduct any image cells in the row about to be removed.
        self.image_cell_count -= self.rows[0].count_image_cells();
        // remove topmost row (and its cache entry)
        self.rows.remove(0);
        self.row_cache.remove(0);

        // add a new empty row at the bottom (BCE: fill with current SGR background)
        let mut new_row = Row::new(self.width);
        new_row.fill_with_tag(&self.current_tag);
        self.rows.push(new_row);
        self.row_cache.push(None);

        // DO NOT move the cursor in alternate buffer
        if self.kind == BufferType::Primary {
            // primary buffer uses scrollback: move cursor with the visible window
            if self.cursor.pos.y > 0 {
                self.cursor.pos.y -= 1;
            }
        }
    }

    pub fn erase_scrollback(&mut self) {
        if self.kind == BufferType::Alternate {
            // Alternate buffer has no scrollback
            return;
        }

        let visible_start = self.visible_window_start(0);

        // Remove all scrollback rows (everything before visible window)
        if visible_start > 0 {
            // Account for image cells in the drained scrollback rows.
            if self.image_cell_count > 0 {
                let drained_images: usize = self.rows[..visible_start]
                    .iter()
                    .map(Row::count_image_cells)
                    .sum();
                self.image_cell_count -= drained_images;
            }
            self.rows.drain(0..visible_start);
            self.row_cache.drain(0..visible_start);
            self.adjust_prompt_rows(visible_start);

            // Adjust cursor
            if self.cursor.pos.y >= visible_start {
                self.cursor.pos.y -= visible_start;
            } else {
                self.cursor.pos.y = 0;
            }

            // scroll_offset is owned by the caller (ViewState); erase_scrollback
            // removes all rows before the visible window so the caller should
            // reset their scroll_offset to 0 after calling this method.
        }

        self.debug_assert_invariants();
    }
}
