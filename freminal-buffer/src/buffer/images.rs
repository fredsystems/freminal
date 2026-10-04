// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Inline image placement and management for the terminal buffer.
//!
//! This module contains methods for placing, clearing, and querying image
//! cells within the buffer, including support for the Kitty graphics protocol
//! and iTerm2 inline images.

use freminal_common::buffer_states::{
    buffer_type::BufferType, format_tag::FormatTag, row_number::RowNumber,
};

use crate::{
    image_store::{
        ImagePlacement, ImageProtocol, ImageStore, InlineImage, SourceCrop, SubCellOffset,
    },
    row::{Row, RowJoin, RowOrigin},
};

use super::Buffer;

/// Result of [`Buffer::place_image`]: the scroll offset adjustment made by
/// scrollback trimming, plus the TRUE stamped origin of the image.
///
/// `origin_row`/`origin_col` reflect where the image's top-left cell
/// (`row_in_image == 0`, `col_in_image == 0`) actually landed. `origin_row` is
/// a stable [`RowNumber`] (Task 125.14): it names the row itself, so it stays
/// correct however many rows an `enforce_scrollback_limit` drain during this
/// call (or a later eviction) removes above it. Convert it to a retained index
/// with [`Buffer::row_index_of`]; `None` means the row has since been evicted.
/// Callers that need to record a placement's origin (e.g. Kitty
/// relative-placement parents, Task 100.14) must use these fields rather than
/// re-reading the cursor before calling `place_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaceImageResult {
    /// The (possibly adjusted) scroll offset, mirroring the value
    /// previously returned directly by `place_image`.
    pub scroll_offset: usize,
    /// The logical row number at which the image's top-left cell was
    /// actually stamped.
    pub origin_row: RowNumber,
    /// The column at which the image's top-left cell was actually stamped.
    pub origin_col: usize,
    /// The placement instance id (Task 100.18) this call stamped every
    /// cell with — echoed back from the caller-supplied `placement_instance`
    /// argument so the caller can feed it into `record_real_placement`
    /// without having to keep a second copy around.
    pub placement_instance: u64,
}

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
    /// Also invalidates the corresponding row cache entry, and raises the
    /// image's stamp horizon to this row (Task 125.15). Used by
    /// `TerminalHandler` for Kitty Unicode placeholder cells.
    pub fn set_image_cell_at(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        placement: ImagePlacement,
        tag: FormatTag,
    ) {
        let image_id = placement.image_id;
        if self.set_image_cell_unstamped(row_idx, col_idx, placement, tag) {
            self.stamp_image_row(image_id, row_idx);
        }
    }

    /// [`Self::set_image_cell_at`] without raising the image's stamp horizon,
    /// for callers that stamp a whole row at once. Returns `true` if the cell
    /// was set (the row exists).
    fn set_image_cell_unstamped(
        &mut self,
        row_idx: usize,
        col_idx: usize,
        placement: ImagePlacement,
        tag: FormatTag,
    ) -> bool {
        if row_idx >= self.rows.len() {
            return false;
        }
        // Check if the old cell already had an image (avoid double-counting).
        let had_image = self.rows[row_idx]
            .cells()
            .get(col_idx)
            .is_some_and(crate::cell::Cell::has_image);
        self.rows[row_idx].set_image_cell(col_idx, placement, tag);
        if !had_image {
            self.image_cell_count += 1;
        }
        self.rows.invalidate(row_idx);
        true
    }

    /// Raise image `image_id`'s stamp horizon to the logical number of row
    /// `row_idx` (Task 125.15). Every site that puts an image cell on a row
    /// must call this, so that front eviction can tell when the image's last
    /// row is gone without scanning cells.
    fn stamp_image_row(&mut self, image_id: u64, row_idx: usize) {
        let number = self.rows.number_of(row_idx);
        self.image_store.stamp_row(image_id, number);
    }

    /// Raise the stamp horizon of every image that has a cell on rows
    /// `[first, last]` to the row the cell now sits on.
    ///
    /// Called after an operation that moved image cells to a *higher* row
    /// within the window (`scroll_slice_down` and its column-confined form),
    /// where a cell can now lie above its image's recorded horizon. Scans
    /// bottom-up so the first sighting of an image is its greatest row. Does
    /// nothing unless an image cell exists, so the common case costs one
    /// comparison.
    pub(in crate::buffer) fn restamp_image_horizons(&mut self, first: usize, last: usize) {
        if self.image_cell_count == 0 || self.image_store.is_empty() {
            return;
        }
        let last = last.min(self.rows.len().saturating_sub(1));
        // Only an image whose horizon lies in [first - 1, last) can have a
        // cell moved above it: a cell now on row `first` came from row
        // `first - 1`, so its image's horizon is at least that, and a horizon
        // already at `last` cannot be exceeded. Usually no image qualifies and
        // the scan below is skipped.
        if !self.image_store.has_horizon_in(
            self.rows.number_of(first.saturating_sub(1)),
            self.rows.number_of(last),
        ) {
            return;
        }
        let mut seen: Vec<u64> = Vec::new();
        for row_idx in (first..=last).rev() {
            for cell in self.rows[row_idx].cells_for_image_scan() {
                if let Some(placement) = cell.image_placement()
                    && !seen.contains(&placement.image_id)
                {
                    seen.push(placement.image_id);
                    let number = self.rows.number_of(row_idx);
                    self.image_store.stamp_row(placement.image_id, number);
                }
            }
        }
    }

    /// Free the cell-owned (Sixel/iTerm2) images among `candidates` that no
    /// cell references any more.
    ///
    /// `candidates` are the images a partial clear (the pre-clear in
    /// [`Self::place_image`]) just took cells from. Unlike
    /// [`Self::clear_image_placements_by_id`] that clear is not buffer-wide, so
    /// an image may or may not still have cells elsewhere. When the buffer
    /// holds no image cell at all (the in-place animation case: one image is
    /// replaced by the next) the answer is immediate; otherwise the cells are
    /// scanned, stopping as soon as every candidate has been found alive.
    /// Kitty images are never candidates for removal: their data outlives
    /// their cells.
    fn release_unreferenced_cell_owned_images(&mut self, mut candidates: Vec<u64>) {
        candidates.retain(|&id| {
            self.image_store.contains(id) && !self.image_store.is_protocol_retained(id)
        });
        if candidates.is_empty() {
            return;
        }
        if self.image_cell_count > 0 {
            for row in &self.rows {
                for cell in row.cells_for_image_scan() {
                    if let Some(placement) = cell.image_placement() {
                        candidates.retain(|&id| id != placement.image_id);
                        if candidates.is_empty() {
                            return;
                        }
                    }
                }
            }
        }
        for id in candidates {
            let _ = self.image_store.remove(id);
        }
    }

    /// Clear every image cell in columns `[start_col, end_col)` of the rows
    /// from the cursor row down, ahead of stamping the new image
    /// `new_image_id` there, and free the cell-owned images this left without
    /// any cell.
    fn clear_image_cells_under(&mut self, start_col: usize, end_col: usize, new_image_id: u64) {
        // Ids of the images this pre-clear took cells from: some may now have
        // no cell left at all (see `release_unreferenced_cell_owned_images`).
        let mut touched: Vec<u64> = Vec::new();
        for row_idx in self.cursor.pos.y..self.rows.len() {
            let row = &mut self.rows[row_idx];
            let mut changed = false;
            for col in start_col..end_col.min(row.max_width()) {
                if let Some(cell) = row.cells_mut().get_mut(col)
                    && let Some(old_id) = cell.image_placement().map(|p| p.image_id)
                {
                    cell.clear_image();
                    self.image_cell_count -= 1;
                    changed = true;
                    if old_id != new_image_id && !touched.contains(&old_id) {
                        touched.push(old_id);
                    }
                }
            }
            if changed {
                row.dirty = true;
                self.rows.invalidate(row_idx);
            }
        }
        self.release_unreferenced_cell_owned_images(touched);
    }

    /// Recompute every image's stamp horizon from the cells themselves, and
    /// free the images nothing references any more.
    ///
    /// For after an operation that renumbers every row (reflow), where the old
    /// horizons name rows that no longer exist. O(rows) when image cells
    /// exist; only called from such whole-buffer rewrites, which are already
    /// O(cells).
    pub(in crate::buffer) fn rebuild_image_horizons(&mut self) {
        if self.image_store.is_empty() {
            return;
        }
        self.image_store
            .retain_referenced(self.rows.iter().map(Row::cells_for_image_scan));

        let mut stamps: Vec<(u64, RowNumber)> = Vec::new();
        if self.image_cell_count > 0 {
            for (row_idx, row) in self.rows.iter().enumerate() {
                let number = self.rows.number_of(row_idx);
                let mut previous: Option<u64> = None;
                for cell in row.cells_for_image_scan() {
                    if let Some(placement) = cell.image_placement()
                        && previous != Some(placement.image_id)
                    {
                        previous = Some(placement.image_id);
                        stamps.push((placement.image_id, number));
                    }
                }
            }
        }
        self.image_store.rebuild_horizons(stamps);
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
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: an evicted (or Task-118 compact) row provably holds
            // no image cells, so skip it — reading its cells would trip
            // `cells_mut`'s eviction debug_assert (and needlessly decompact a
            // compact row) for no possible clearing work.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
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
                *entry = None;
            }
        }
        self.image_cell_count -= cleared;
    }

    /// Clear all image placements for a specific image ID from every cell.
    pub fn clear_image_placements_by_id(&mut self, image_id: u64) {
        let mut cleared = 0usize;
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
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
                *entry = None;
            }
        }
        self.image_cell_count -= cleared;
        // Every cell of this id was just cleared buffer-wide, so a cell-owned
        // (Sixel/iTerm2) image has nothing left that could keep it alive: free
        // it now rather than let it sit until its rows are evicted. A Kitty
        // image's data stays addressable without any cell and is kept.
        let _ = self.image_store.remove_cell_owned(image_id);
    }

    /// Clear image placements matching BOTH a specific image ID and a
    /// specific (non-zero) Kitty placement ID (`p=`) from every cell.
    ///
    /// Unlike [`Self::clear_image_placements_by_id`] (which clears every
    /// placement of an image regardless of `p=`), this narrows to the ONE
    /// named placement — used to implement the kitty spec's REPLACE
    /// semantics (a second `a=p` put with the same non-zero `p=` replaces
    /// only that placement, leaving any other coexisting placements of the
    /// same image untouched, Task 100.18/100.20) and `d=i,p=<n>` deletion
    /// narrowing (Task 100.20).
    pub fn clear_image_placements_by_placement(&mut self, image_id: u64, placement_id: u32) {
        let mut cleared = 0usize;
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.image_id == image_id && p.placement_id == Some(placement_id))
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                *entry = None;
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
                self.rows.invalidate(i);
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
            self.rows.invalidate(row_idx);
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
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
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
                *entry = None;
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
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
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

    /// Clear image placements from every cell in the VISIBLE window only
    /// (Kitty `d=a`/`d=A` — "all placements visible on screen").
    ///
    /// Scrollback rows above the visible window are left untouched, unlike
    /// [`Self::clear_all_image_placements`] which clears the entire buffer.
    /// Returns the distinct image ids that had a cleared placement, so the
    /// caller can decide (per the `d=a`/`d=A` case) whether to also free
    /// the underlying store data for images no longer referenced anywhere.
    ///
    /// `scroll_offset` is always `0` from the PTY thread (see
    /// [`Self::visible_window_start`]).
    pub fn clear_image_placements_visible(&mut self, scroll_offset: usize) -> Vec<u64> {
        let start = self.visible_window_start(scroll_offset);
        let mut ids: Vec<u64> = Vec::new();
        let mut cleared = 0usize;
        for i in start..self.rows.len() {
            let row = &mut self.rows[i];
            let mut changed = false;
            for cell in row.cells_mut() {
                if let Some(placement) = cell.image_placement() {
                    let id = placement.image_id;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                self.rows.invalidate(i);
            }
        }
        self.image_cell_count -= cleared;
        ids
    }

    /// Clear image placements at a specific cell position, but only when
    /// the cell's placement z-index matches `z` (Kitty `d=q`/`d=Q` — cell +
    /// z-index intersection).
    ///
    /// Clears only the specific on-screen placement instance the matched
    /// cell belongs to — not every placement sharing the same `image_id`.
    /// With coexisting placements of one image, `d=q`/`d=Q` must not delete
    /// unrelated placements at other cells or z-indexes. Returns the cleared
    /// image's id, if any cell matched, so the caller can free the
    /// underlying store data when requested (`d=Q`).
    pub fn clear_image_placements_at_cell_with_z(
        &mut self,
        row: usize,
        col: usize,
        z: i32,
    ) -> Option<u64> {
        let placement = self
            .rows
            .get(row)?
            .cells()
            .get(col)?
            .image_placement()
            .filter(|p| p.z_index == z)?;
        let id = placement.image_id;
        let instance = placement.placement_instance;

        let mut cleared = 0usize;
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
            let mut changed = false;
            for cell in row.cells_mut() {
                if cell
                    .image_placement()
                    .is_some_and(|p| p.placement_instance == instance)
                {
                    cell.clear_image();
                    cleared += 1;
                    changed = true;
                }
            }
            if changed {
                row.dirty = true;
                *entry = None;
            }
        }
        self.image_cell_count -= cleared;
        Some(id)
    }

    /// Clear all image placements with the given z-index.
    pub fn clear_image_placements_by_z_index(&mut self, z: i32) {
        let mut cleared = 0usize;
        let (rows, cache, _) = self.rows.split_mut();
        for (row, entry) in rows.iter_mut().zip(cache.iter_mut()) {
            // Task 119: skip evicted/compact rows — they hold no images.
            if row.is_compact() || row.is_evicted() {
                continue;
            }
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
                *entry = None;
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
    /// the image, at column 0 — matching iTerm2/Kitty behaviour. If no row
    /// already exists below the image (the common case: the image was
    /// placed at the buffer's tail), a fresh blank row is appended so
    /// subsequent output never overwrites the image's own cells (Task
    /// 100.15 — previously the cursor was parked on the image's own last
    /// row in this case, so the very next character write destroyed the
    /// image via [`Self::clear_images_overwritten_by_text`]). The only
    /// exception is a primary-buffer image tall enough that appending
    /// this row and re-enforcing the scrollback limit drains the image
    /// itself off the top — an edge case limited to images taller than
    /// the entire scrollback, where the cursor falls back to the last
    /// remaining row.
    ///
    /// If the image extends beyond the right edge of the terminal, it is
    /// clipped to the terminal width (cells beyond the edge are not placed).
    /// If the image extends below the visible area, new rows are created
    /// (scrolling if necessary in the primary buffer).
    ///
    /// Returns a [`PlaceImageResult`] describing the (possibly adjusted)
    /// scroll offset and the TRUE stamped origin row/column of the image —
    /// the origin reflects any `enforce_scrollback_limit` drain that
    /// occurred during placement, so callers must use it (not a
    /// pre-call cursor read) when recording where the image actually
    /// landed (Task 100.14).
    #[allow(clippy::too_many_arguments)]
    // All parameters are required image placement inputs; grouping into a
    // struct would obscure the data flow without reducing coupling, matching
    // the established convention for `place_image`-adjacent methods.
    pub fn place_image(
        &mut self,
        image: InlineImage,
        scroll_offset: usize,
        protocol: ImageProtocol,
        image_number: Option<u32>,
        placement_id: Option<u32>,
        z_index: i32,
        source_crop: Option<SourceCrop>,
        placement_instance: u64,
        subcell_offset: Option<SubCellOffset>,
    ) -> PlaceImageResult {
        let image_id = image.id;
        let display_cols = image.display_cols;
        let display_rows = image.display_rows;

        // Store the image centrally. Kitty image data must survive
        // scroll-out (addressable by id/number until explicit free or quota
        // eviction); Sixel/iTerm2 data is cell-owned and GC'd on scroll-out.
        if protocol == ImageProtocol::Kitty {
            self.image_store.insert_protocol_retained(image);
        } else {
            self.image_store.insert(image);
        }

        let start_col = self.cursor.pos.x;

        // Clamp display_cols to not exceed terminal width.
        let effective_cols = display_cols.min(self.width.saturating_sub(start_col));

        // Before placing new image cells, clear any existing image cells in
        // the column range from the cursor row downward.  This handles the
        // common case where a new (possibly smaller) image replaces an old
        // (possibly larger) one at the same position — without this, stale
        // cells from the old image persist below the new one.
        self.clear_image_cells_under(start_col, start_col + effective_cols, image_id);

        let mut current_offset = scroll_offset;

        // Place image cells row by row.
        //
        // We track `base_row` which starts at the cursor's current row and
        // shifts downward as rows are created.  Unlike `scroll_up()`, we
        // grow the buffer by pushing new rows and then let
        // `enforce_scrollback_limit` trim excess from the top — this avoids
        // the infinite-loop problem where `scroll_up()` keeps rows.len()
        // constant.
        //
        // The origin is recorded as a stable row number, so the scrollback
        // drains below need no compensation: `base_row` is re-derived from the
        // number after each one.
        let origin_row = self.cursor_row_number();
        let mut base_row = self.cursor.pos.y;
        let mut any_stamped = false;

        for img_row in 0..display_rows {
            let target_row = base_row + img_row;

            // Ensure the target row exists.
            while target_row >= self.rows.len() {
                self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);
            }

            // Invalidate the row cache for this row.
            self.rows.invalidate(target_row);

            let row = &mut self.rows[target_row];
            row.dirty = true;

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
                    source_crop,
                    placement_instance,
                    subcell_offset,
                };
                row.set_image_cell(col, placement, self.current_tag.clone());
                placed_count += 1;
            }
            self.image_cell_count += placed_count;
            if placed_count > 0 {
                self.stamp_image_row(image_id, target_row);
                any_stamped = true;
            }
        }

        // An image none of whose cells fit (e.g. the cursor sat past the last
        // column) is referenced by nothing and has no horizon, so eviction
        // would never release it. Sixel/iTerm2 images are cell-owned: drop it
        // now. A Kitty image's data stays addressable by id regardless.
        if !any_stamped && protocol != ImageProtocol::Kitty {
            let _ = self.image_store.remove(image_id);
        }

        // Enforce scrollback limit — this may drain rows from the top.
        if self.kind == BufferType::Primary {
            current_offset = self.enforce_scrollback_limit(current_offset);
            // An origin drained off the top clamps to the oldest row, as the
            // hand-counted compensation this replaces did.
            base_row = origin_row.rows_after(self.rows.base()).unwrap_or(0);
        }

        // Move cursor below the image, column 0 (iTerm2 behaviour).
        let mut final_row = base_row + display_rows;
        if final_row >= self.rows.len() {
            // No row exists below the image yet (the common case: the image
            // was placed at the buffer's tail). Append one so subsequent
            // output lands below the image instead of overwriting it.
            self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);

            if self.kind == BufferType::Primary {
                current_offset = self.enforce_scrollback_limit(current_offset);
                base_row = origin_row.rows_after(self.rows.base()).unwrap_or(0);
            }

            final_row = base_row + display_rows;
        }
        if final_row < self.rows.len() {
            self.cursor.pos.y = final_row;
        } else {
            self.cursor.pos.y = self.rows.len().saturating_sub(1);
        }
        self.cursor.pos.x = 0;

        self.debug_assert_invariants();
        PlaceImageResult {
            scroll_offset: current_offset,
            origin_row,
            origin_col: start_col,
            placement_instance,
        }
    }

    /// Stamp an image's cell grid at an explicit screen origin `(origin_row,
    /// origin_col)` WITHOUT moving the cursor or scrolling the buffer.
    ///
    /// Used for relative kitty graphics placements (Task 100.4a), whose
    /// position is derived from a parent placement's origin plus an offset,
    /// not from the cursor. Cells outside the current buffer bounds are
    /// silently skipped (the visible part is stamped). The image must
    /// already be in the image store — this method only writes the
    /// per-cell `ImagePlacement` references, mirroring the inner stamping
    /// loop of [`Self::place_image`] but positioned explicitly.
    #[allow(clippy::too_many_arguments)]
    // All parameters are required image placement inputs; grouping into a
    // struct would obscure the data flow without reducing coupling, matching
    // the established convention for `place_image`-adjacent methods.
    pub fn place_image_at(
        &mut self,
        image_id: u64,
        origin_row: usize,
        origin_col: usize,
        display_cols: usize,
        display_rows: usize,
        protocol: ImageProtocol,
        image_number: Option<u32>,
        placement_id: Option<u32>,
        z_index: i32,
        source_crop: Option<SourceCrop>,
        placement_instance: u64,
        subcell_offset: Option<SubCellOffset>,
    ) {
        for img_row in 0..display_rows {
            let target_row = origin_row + img_row;
            if target_row >= self.rows.len() {
                break; // below the buffer — skip (visible part only)
            }
            let mut stamped = false;
            for img_col in 0..display_cols {
                let col = origin_col + img_col;
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
                    source_crop,
                    placement_instance,
                    subcell_offset,
                };
                let tag = self.current_tag.clone();
                if self.set_image_cell_unstamped(target_row, col, placement, tag) {
                    stamped = true;
                }
            }
            if stamped {
                self.stamp_image_row(image_id, target_row);
            }
        }
    }
}
