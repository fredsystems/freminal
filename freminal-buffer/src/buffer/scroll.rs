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

use freminal_common::buffer_states::{
    buffer_type::BufferType, format_tag::FormatTag, modes::declrmm::Declrmm,
};

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
        self.visible_line_widths_extended(scroll_offset, 0)
    }

    /// Like [`Self::visible_line_widths`] but extends the window upward by
    /// `extra_rows` (see [`Self::visible_window_bounds`]). The returned vector
    /// has one entry per row in the extended window, top-to-bottom.
    #[must_use]
    pub fn visible_line_widths_extended(
        &self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> Vec<crate::row::LineWidth> {
        let (start, end) = self.visible_window_bounds(scroll_offset, extra_rows);
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

    /// Compute the `[start, end)` row-index bounds of the flatten window for a
    /// given `scroll_offset`, optionally extended **upward** by `extra_rows`.
    ///
    /// The window always ends at `visible_window_start(scroll_offset) +
    /// height` (clamped to the buffer length); `extra_rows` pulls the *start*
    /// earlier into scrollback so the window contains extra rows **above** the
    /// normal visible window. This is what the GUI uses when command-block
    /// folds collapse rows in the visible window: the renderer needs extra
    /// real rows above the fold so that, after collapsing, a full screen of
    /// rendered rows can still be painted with the live bottom pinned.
    ///
    /// `extra_rows` is clamped so the start never underflows past row 0.
    /// When `extra_rows == 0` this is exactly the normal visible window.
    #[must_use]
    pub(in crate::buffer) fn visible_window_bounds(
        &self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> (usize, usize) {
        let base_start = self.visible_window_start(scroll_offset);
        let end = (base_start + self.height).min(self.rows.len());
        let start = base_start.saturating_sub(extra_rows);
        (start, end)
    }

    /// Return `true` if any row in the visible window is dirty (needs re-flattening).
    ///
    /// The PTY thread calls this with `scroll_offset = 0`.  A `false` result means
    /// the cached flat representation for every visible row is still valid, so
    /// `build_snapshot` can skip the flatten step entirely and reuse the previous
    /// `visible_chars` / `visible_tags` vectors.
    #[must_use]
    pub fn any_visible_dirty(&self, scroll_offset: usize) -> bool {
        self.any_visible_dirty_extended(scroll_offset, 0)
    }

    /// Like [`Self::any_visible_dirty`] but checks the window extended upward
    /// by `extra_rows` (see [`Self::visible_window_bounds`]).
    #[must_use]
    pub fn any_visible_dirty_extended(&self, scroll_offset: usize, extra_rows: usize) -> bool {
        if self.rows.is_empty() || self.height == 0 {
            return false;
        }
        let (vis_start, vis_end) = self.visible_window_bounds(scroll_offset, extra_rows);
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
        self.visible_image_placements_extended(scroll_offset, 0)
    }

    /// Like [`Self::visible_image_placements`] but extends the window upward by
    /// `extra_rows` (see [`Self::visible_window_bounds`]). The returned vector
    /// has `window_rows * width` entries in row-major order, matching the
    /// extended `visible_chars` layout.
    #[must_use]
    pub fn visible_image_placements_extended(
        &self,
        scroll_offset: usize,
        extra_rows: usize,
    ) -> Vec<Option<ImagePlacement>> {
        if self.rows.is_empty() || self.height == 0 || self.width == 0 {
            return Vec::new();
        }
        let (vis_start, vis_end) = self.visible_window_bounds(scroll_offset, extra_rows);
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

    /// Scroll rows `[first, last]` UP by one line, honouring DECLRMM.
    ///
    /// When DECLRMM is active the scroll is confined to the DECSLRM
    /// left/right margins (`scroll_slice_up_columns`); otherwise the whole row
    /// is shifted (`scroll_slice_up`). Centralises the DECLRMM decision shared
    /// by SU, the primary LF/IND/RI/NEL path, and IL/DL.
    pub(in crate::buffer) fn scroll_slice_up_confined(&mut self, first: usize, last: usize) {
        if self.declrmm_enabled == Declrmm::Enabled {
            let (left, right) = (self.scroll_region_left, self.scroll_region_right);
            self.scroll_slice_up_columns(first, last, left, right);
        } else {
            self.scroll_slice_up(first, last);
        }
    }

    /// Scroll rows `[first, last]` DOWN by one line, honouring DECLRMM.
    ///
    /// Mirror of [`scroll_slice_up_confined`] for downward scrolls (SD, the
    /// primary RI path, IL/DL).
    pub(in crate::buffer) fn scroll_slice_down_confined(&mut self, first: usize, last: usize) {
        self.scroll_slice_down_confined_n(first, last, 1);
    }

    /// Scroll rows `[first, last]` DOWN by `n` lines, honouring DECLRMM.
    ///
    /// The moved images' stamp horizons (Task 125.15) are raised once, after
    /// the last shift: they only have to be correct before the next eviction,
    /// and restamping per line would rescan the region `n` times.
    pub(in crate::buffer) fn scroll_slice_down_confined_n(
        &mut self,
        first: usize,
        last: usize,
        n: usize,
    ) {
        for _ in 0..n {
            if self.declrmm_enabled == Declrmm::Enabled {
                let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                self.scroll_slice_down_columns(first, last, left, right);
            } else {
                self.scroll_slice_down(first, last);
            }
        }
        self.restamp_image_horizons(first + 1, last);
    }

    /// Scroll DECSTBM region UP by 1 (primary buffer)
    pub(in crate::buffer) fn scroll_region_up_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_up_confined(t, b);
        }
    }

    /// Scroll DECSTBM region DOWN by 1 (primary buffer)
    pub(in crate::buffer) fn scroll_region_down_primary(&mut self) {
        let (t, b) = self.scroll_region_rows();
        if t < b {
            self.scroll_slice_down_confined(t, b);
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
            self.scroll_slice_up_confined(t, b);
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
        self.scroll_slice_down_confined_n(t, b, clamped);
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

        // The shift overwrites rows[first] with a copy of rows[first + 1], so
        // any image cells it held are lost: deduct them before they are gone.
        if self.image_cell_count > 0 {
            self.image_cell_count -= self.rows[first].count_image_cells();
        }
        for row_idx in first..last {
            let next = self.rows[row_idx + 1].clone();
            self.rows[row_idx] = next;
            // Rotate the cache entry in lockstep: a moved row keeps its cached
            // flat representation (it hasn't changed content, only position).
            let cache = self.rows.cache_mut();
            cache[row_idx] = cache[row_idx + 1].take();
        }

        // The original rows[last] was not shifted (the loop only copies
        // rows[row_idx+1] into rows[row_idx] for row_idx in first..last), so
        // its content now exists twice: once moved into rows[last - 1] and
        // once still here. Blanking it removes the duplicate, which the image
        // counter never counted, so no deduction is due here.
        let new_row = Row::new(self.width);
        // Scroll-created blank rows use default background (no BCE).
        // See `push_row` comment for rationale.
        self.rows[last] = new_row;
        // New blank row at `last` — no cached representation yet.
        self.rows.cache_mut()[last] = None;

        // Task 121 Part C fix: this loop rotates already-clean row-cache
        // entries between row indices (a moved row's cache moves with it)
        // without marking the moved rows dirty or nulling their cache, and
        // without changing `self.rows.len()`. Neither the visible-window
        // fingerprint nor the per-call first-rebuilt-row tracking in
        // `flatten.rs` can observe that row *content* moved to a different
        // index, so a cached incremental merge built before this rotation
        // would serve stale, pre-rotation row content at the rotated
        // indices. Null the merge cache so the next flatten does a full
        // re-merge. See `Buffer::merge_cache`'s field doc.
        self.merge_cache = None;
    }

    /// Scroll a contiguous vertical slice [first, last] DOWN by one line.
    /// Rows outside that range are untouched. New top line is blank.
    ///
    /// Does **not** raise the moved images' stamp horizons (Task 125.15): go
    /// through [`Self::scroll_slice_down_confined_n`], which does so once.
    pub(in crate::buffer) fn scroll_slice_down(&mut self, first: usize, last: usize) {
        if first >= last {
            return;
        }
        if last >= self.rows.len() {
            return;
        }

        // The shift overwrites rows[last] with a copy of rows[last - 1], so
        // any image cells it held are lost: deduct them before they are gone.
        if self.image_cell_count > 0 {
            self.image_cell_count -= self.rows[last].count_image_cells();
        }
        for row_idx in (first + 1..=last).rev() {
            let prev = self.rows[row_idx - 1].clone();
            self.rows[row_idx] = prev;
            // Rotate the cache entry in lockstep.
            let cache = self.rows.cache_mut();
            cache[row_idx] = cache[row_idx - 1].take();
        }

        // The original rows[first] was not shifted (the loop only copies
        // rows[row_idx-1] into rows[row_idx] for row_idx in first+1..=last),
        // so its content now exists twice: once moved into rows[first + 1] and
        // once still here. Blanking it removes the duplicate, which the image
        // counter never counted, so no deduction is due here.
        let new_row = Row::new(self.width);
        // Scroll-created blank rows use default background (no BCE).
        // See `push_row` comment for rationale.
        self.rows[first] = new_row;
        // New blank row at `first` — no cached representation yet.
        self.rows.cache_mut()[first] = None;

        // Task 121 Part C fix: same rationale as the equivalent comment in
        // `scroll_slice_up` — this loop rotates already-clean row-cache
        // entries between row indices without dirtying the moved rows or
        // changing `self.rows.len()`, which the fingerprint + first-rebuilt
        // -row invalidation cannot observe. Null the merge cache to force a
        // full re-merge next flatten. See `Buffer::merge_cache`'s field doc.
        self.merge_cache = None;
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
            self.rows.cache_mut()[row_idx] = None;
        }
        // Blank [left_col, right_col] on the last row.
        let row = &mut self.rows[last];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.rows.cache_mut()[last] = None;

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
            self.rows.cache_mut()[row_idx] = None;
        }
        // Blank [left_col, right_col] on the first row.
        let row = &mut self.rows[first];
        row.erase_cells_at(left_col, right_col - left_col + 1, &tag);
        self.rows.cache_mut()[first] = None;

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
        // Remove the topmost row (and its cache entry and block reference).
        // The eviction deducts the row's image cells and releases anything
        // that only it kept alive: a compressed block, an image.
        let _ = self.evict_front_rows(1);

        // add a new empty row at the bottom, using default background (no BCE).
        // Scrolling only moves content; it is not an explicit erase, so the
        // scrolled-in row must not inherit the active SGR background — same
        // rationale as `push_row` and `scroll_slice_up`/`_down`.
        let new_row = Row::new(self.width);
        self.rows.push(new_row);

        // Task 121 Part C fix: `rows.evict_front(1)` + `rows.push(new_row)`
        // nets to the same `self.rows.len()`, and every already-clean
        // row-cache entry above index 0 shifts down by one index in
        // lockstep with its row's content — without any of the moved rows
        // being marked dirty or `None`. This is the same confined
        // in-place-rotation bug class as `scroll_slice_up`/`_down` (see
        // `Buffer::merge_cache`'s field doc, which explicitly names
        // `scroll_up` as part of the confirmed gap): the fingerprint and
        // first-rebuilt-row invalidation cannot observe the identity shift,
        // so a cached incremental merge would serve stale, pre-shift row
        // content. Null the merge cache to force a full re-merge next
        // flatten.
        self.merge_cache = None;

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
            // Each evicted compressed row releases its block's live-row count.
            // Do not clear `self.blocks` here: a block can straddle the visible
            // window (the window can grow over compressed rows, which are only
            // decompressed when read), and its surviving rows still need it.
            let _ = self.evict_front_rows(visible_start);
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod image_count_tests {
    use std::sync::Arc;

    use crate::{
        image_store::{AnimationControl, ImageProtocol, ImageSizeMode, InlineImage, next_image_id},
        row::Row,
    };

    use super::*;

    /// A 5-row, 10-column buffer with a 1x1 Kitty image on `image_row`.
    fn buffer_with_image_on(image_row: usize) -> Buffer {
        let mut buf = Buffer::new(10, 5);
        while buf.rows.len() < 5 {
            buf.rows.push(Row::new(10));
        }
        buf.cursor.pos.y = image_row;
        let image = InlineImage {
            id: next_image_id(),
            pixels: Arc::new(vec![0u8; 4]),
            width_px: 8,
            height_px: 16,
            display_cols: 1,
            display_rows: 1,
            size_mode: ImageSizeMode::NativePixels,
            frames: Vec::new(),
            root_gap_ms: 0,
            animation: AnimationControl::default(),
        };
        let _ = buf.place_image(image, 0, ImageProtocol::Kitty, None, None, 0, None, 1, None);
        assert_eq!(buf.image_cell_count, 1);
        buf
    }

    fn actual_image_cells(buf: &Buffer) -> usize {
        buf.rows.iter().map(Row::count_image_cells).sum()
    }

    /// A shifted image cell is moved, not lost: the counter must not drop.
    #[test]
    fn scroll_slice_down_does_not_deduct_a_cell_that_only_moved() {
        let mut buf = buffer_with_image_on(1);
        buf.scroll_slice_down_confined(1, 3);
        assert_eq!(actual_image_cells(&buf), 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_slice_up_does_not_deduct_a_cell_that_only_moved() {
        let mut buf = buffer_with_image_on(3);
        buf.scroll_slice_up(1, 3);
        assert_eq!(actual_image_cells(&buf), 1);
        assert_eq!(buf.image_cell_count, 1);
        buf.debug_assert_invariants();
    }

    /// The row pushed off the far end of the slice loses its cells.
    #[test]
    fn scroll_slice_down_deducts_a_cell_pushed_off_the_bottom() {
        let mut buf = buffer_with_image_on(3);
        buf.scroll_slice_down_confined(1, 3);
        assert_eq!(actual_image_cells(&buf), 0);
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }

    #[test]
    fn scroll_slice_up_deducts_a_cell_pushed_off_the_top() {
        let mut buf = buffer_with_image_on(1);
        buf.scroll_slice_up(1, 3);
        assert_eq!(actual_image_cells(&buf), 0);
        assert_eq!(buf.image_cell_count, 0);
        buf.debug_assert_invariants();
    }
}
