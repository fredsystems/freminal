// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Erase operations for [`Buffer`].
//!
//! Covers character erase (ECH), line erase (EL 0/1/2), display erase
//! (ED 0/1/2), scrollback erase (ED 3), and the internal helper
//! `collect_and_clear_image_ids_in_rows` that sweeps non-Kitty image
//! placements before any bulk clear.

use freminal_common::buffer_states::modes::declrmm::Declrmm;

use crate::image_store::ImageProtocol;

use crate::buffer::Buffer;

impl Buffer {
    /// Implements ECH – Erase Characters.
    ///
    /// Replaces `n` cells starting at the cursor column with blanks using the current
    /// format tag.  Cells to the right of the erased range are **not** shifted.
    /// The cursor does not move.
    ///
    /// When DECLRMM is active, `n` is clamped so the erase never crosses the
    /// right margin.
    ///
    /// Wide-glyph cleanup is delegated to [`Row::erase_cells_at`].
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            // Row doesn't exist yet — nothing to erase.
            return;
        }

        // Clamp erase count to right margin when DECLRMM is active.
        let effective_n = if self.declrmm_enabled == Declrmm::Enabled {
            let max_erase = self.scroll_region_right.saturating_sub(col) + 1;
            n.min(max_erase)
        } else {
            n
        };

        // Sweep image cells that are about to be erased (non-Kitty images).
        self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(col + effective_n));

        // Count remaining image cells (e.g. Kitty) in the range being erased.
        let remaining = if self.image_cell_count > 0 {
            self.rows[row].count_image_cells_in_range(col, col + effective_n)
        } else {
            0
        };

        let tag = self.current_tag.clone();
        self.rows[row].erase_cells_at(col, effective_n, &tag);
        self.image_cell_count -= remaining;

        self.debug_assert_invariants();
    }

    /// Return the current scroll region as 0-based inclusive `(top, bottom)`.
    #[must_use]
    pub const fn scroll_region(&self) -> (usize, usize) {
        (self.scroll_region_top, self.scroll_region_bottom)
    }

    /// Reset left/right margins to the full terminal width.
    /// Public so `TerminalHandler` can call it when DECLRMM is reset.
    pub const fn reset_scroll_region_left_right(&mut self) {
        self.scroll_region_left = 0;
        self.scroll_region_right = self.width.saturating_sub(1);
    }

    /// Enable or disable DECLRMM (`?69`).
    ///
    /// When disabled, left/right margins are also reset to full-screen.
    pub fn set_declrmm(&mut self, mode: Declrmm) {
        self.declrmm_enabled = mode;
        if self.declrmm_enabled == Declrmm::Disabled {
            self.reset_scroll_region_left_right();
        }
    }

    /// Returns the current DECLRMM (`?69`) state.
    #[must_use]
    pub const fn is_declrmm_enabled(&self) -> Declrmm {
        self.declrmm_enabled
    }

    /// Set DECSLRM left/right margins (1-based inclusive).
    ///
    /// Only effective when DECLRMM (`?69`) is active; the caller
    /// (`TerminalHandler::handle_set_left_right_margins`) must gate on that flag.
    /// If parameters are invalid, resets to full-width.
    pub fn set_left_right_margins(&mut self, left1: usize, right1: usize) {
        // 0 → treat as 1 (reset)
        if left1 == 0 || right1 == 0 {
            self.reset_scroll_region_left_right();
            self.set_cursor_pos(Some(0), Some(0));
            return;
        }

        let left = left1.saturating_sub(1);
        let right = right1.saturating_sub(1);

        if left >= right || right >= self.width {
            self.reset_scroll_region_left_right();
            self.set_cursor_pos(Some(0), Some(0));
            return;
        }

        self.scroll_region_left = left;
        self.scroll_region_right = right;

        // DECSLRM homes the cursor to (col 0, row 0) — just like DECSTBM.
        self.set_cursor_pos(Some(0), Some(0));
    }

    /// Return the current left/right margins as 0-based inclusive `(left, right)`.
    #[must_use]
    pub const fn left_right_margins(&self) -> (usize, usize) {
        (self.scroll_region_left, self.scroll_region_right)
    }

    /// Erase from cursor to end of display (ED 0)
    pub fn erase_to_end_of_display(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        self.collect_and_clear_image_ids_in_rows(cursor_y, visible_end, Some(cursor_x), None);

        // Count any remaining image cells (e.g. Kitty images) that survived the
        // sweep but will be destroyed by the row-level clear operations below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            if cursor_y < self.rows.len() {
                remaining_images +=
                    self.rows[cursor_y].count_image_cells_in_range(cursor_x, self.width);
            }
            for i in (cursor_y + 1)..visible_end.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
        }

        // Clear from cursor to end of current row
        if cursor_y < self.rows.len() {
            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_from(cursor_x, &tag);
        }

        // Clear all rows below cursor in the visible window
        for i in (cursor_y + 1)..visible_end.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase from beginning of display to cursor (ED 1)
    pub fn erase_to_beginning_of_display(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        // First row being erased starts at col 0; the cursor row is erased
        // from col 0..=cursor_x.  For simplicity, sweep the entire row range.
        self.collect_and_clear_image_ids_in_rows(visible_start, cursor_y + 1, None, None);

        // Count any remaining image cells (e.g. Kitty images) that survived the
        // sweep but will be destroyed by the row-level clear operations below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            for i in visible_start..cursor_y.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
            if cursor_y < self.rows.len() {
                remaining_images += self.rows[cursor_y].count_image_cells_in_range(0, cursor_x + 1);
            }
        }

        // Clear all rows above cursor in the visible window
        for i in visible_start..cursor_y.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        // Clear from beginning of current row to cursor
        if cursor_y < self.rows.len() {
            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_to(cursor_x + 1, &tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase entire display (ED 2)
    pub fn erase_display(&mut self) {
        // PTY always operates at live bottom (scroll_offset = 0)
        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Sweep image cells that are about to be erased (clears non-Kitty
        // images buffer-wide and decrements image_cell_count for them).
        self.collect_and_clear_image_ids_in_rows(visible_start, visible_end, None, None);

        // Count any remaining image cells (e.g. Kitty images) that survived
        // the sweep but will be destroyed by the row-level clear below.
        let mut remaining_images = 0usize;
        if self.image_cell_count > 0 {
            for i in visible_start..visible_end.min(self.rows.len()) {
                remaining_images += self.rows[i].count_image_cells();
            }
        }

        for i in visible_start..visible_end.min(self.rows.len()) {
            let tag = self.current_tag.clone();
            self.rows[i].clear_with_tag(&tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }

    /// Erase from cursor to end of line (EL 0)
    pub fn erase_line_to_end(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, Some(cursor_x), None);

            // Count remaining image cells (e.g. Kitty) in the range being cleared.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells_in_range(cursor_x, self.width)
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_from(cursor_x, &tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Erase from beginning of line to cursor (EL 1)
    pub fn erase_line_to_beginning(&mut self) {
        let cursor_y = self.cursor.pos.y;
        let cursor_x = self.cursor.pos.x;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, None, None);

            // Count remaining image cells (e.g. Kitty) in the range being cleared.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells_in_range(0, cursor_x + 1)
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_to(cursor_x + 1, &tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Erase entire line (EL 2)
    pub fn erase_line(&mut self) {
        let cursor_y = self.cursor.pos.y;

        if cursor_y < self.rows.len() {
            // Sweep image cells that are about to be erased (non-Kitty images).
            self.collect_and_clear_image_ids_in_rows(cursor_y, cursor_y + 1, None, None);

            // Count remaining image cells (e.g. Kitty) in the row.
            let remaining = if self.image_cell_count > 0 {
                self.rows[cursor_y].count_image_cells()
            } else {
                0
            };

            let tag = self.current_tag.clone();
            self.rows[cursor_y].clear_with_tag(&tag);
            self.image_cell_count -= remaining;
        }

        self.debug_assert_invariants();
    }

    /// Scan rows in `[row_start..row_end)` for image cells and clear every
    /// cell of each found image across the entire buffer.
    ///
    /// If `start_col` is `Some(col)`, only cells from `col..` on the first
    /// row (or single row) are scanned.  If `end_col` is `Some(ec)`, only
    /// cells up to (exclusive) `ec` are scanned on the first row.  If
    /// `None`, scanning goes to the end of the row.
    ///
    /// For rows after the first (when `row_end - row_start > 1`), all cells
    /// are always scanned.
    ///
    /// This is a no-op when no image cells are present.
    pub(in crate::buffer) fn collect_and_clear_image_ids_in_rows(
        &mut self,
        row_start: usize,
        row_end: usize,
        start_col: Option<usize>,
        end_col: Option<usize>,
    ) {
        let end = row_end.min(self.rows.len());
        if row_start >= end {
            return;
        }

        // Collect unique image IDs from the targeted cells, skipping Kitty
        // images (they are cleared only via explicit `a=d` commands).
        let mut ids_to_clear: Vec<u64> = Vec::new();
        for (idx, row) in self.rows[row_start..end].iter().enumerate() {
            let cells = row.cells();
            let skip = if idx == 0 {
                start_col.unwrap_or(0).min(cells.len())
            } else {
                0
            };
            let limit = if idx == 0 {
                end_col.unwrap_or(cells.len()).min(cells.len())
            } else {
                cells.len()
            };
            if skip >= limit {
                continue;
            }
            for cell in &cells[skip..limit] {
                if let Some(placement) = cell.image_placement() {
                    // Kitty images are not cleared by text writes.
                    if placement.protocol == ImageProtocol::Kitty {
                        continue;
                    }
                    let id = placement.image_id;
                    if !ids_to_clear.contains(&id) {
                        ids_to_clear.push(id);
                    }
                }
            }
        }

        // Clear all cells of each affected image buffer-wide.
        for id in ids_to_clear {
            self.clear_image_placements_by_id(id);
        }
    }
}
