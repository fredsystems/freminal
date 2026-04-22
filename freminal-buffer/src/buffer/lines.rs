// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Line feed, cursor movement, and line manipulation operations for [`Buffer`].
//!
//! Covers backspace (`handle_backspace`, `reverse_wrap_up`), line feed
//! variants (`handle_lf`, `handle_ind`, `handle_nel`, `handle_ri`), screen
//! alignment test (`screen_alignment_test`), and insert/delete line/character
//! operations (`insert_lines`, `delete_lines`, `insert_spaces`, `delete_chars`).

use freminal_common::buffer_states::{
    buffer_type::BufferType,
    modes::{
        declrmm::Declrmm, lnm::Lnm, reverse_wrap_around::ReverseWrapAround,
        xt_rev_wrap2::XtRevWrap2,
    },
};

use crate::row::{Row, RowJoin, RowOrigin};

use crate::buffer::Buffer;

impl Buffer {
    /// Handle ANSI Backspace (BS, 0x08).
    ///
    /// Semantics (ECMA-48, VT100):
    /// - Move cursor left by one cell.
    /// - If the cursor is in the pending-wrap state (`cursor.pos.x == self.width`,
    ///   i.e. just wrote into the last column), BS treats the cursor as if it were
    ///   at `self.width - 1` (the last visible column) before subtracting, which
    ///   places it at `self.width - 2`.  This matches real VT100 hardware, where
    ///   the pending-wrap flag is separate from the reported column position and BS
    ///   clears the flag and moves left from the reported column.
    /// - If the cell to the left is a continuation cell of a wide glyph,
    ///   skip left until the glyph head.
    /// - If cursor is at column 0 and `reverse_wrap` is false, do nothing.
    /// - If cursor is at column 0 and `reverse_wrap` is true, wrap to the
    ///   last column of the previous line (within the visible screen, or
    ///   into scrollback if `xt_rev_wrap2` is also true).
    /// - Never deletes characters.
    pub fn handle_backspace(&mut self, reverse_wrap: ReverseWrapAround, xt_rev_wrap2: XtRevWrap2) {
        if self.cursor.pos.x == 0 {
            if reverse_wrap == ReverseWrapAround::DontWrap {
                return;
            }
            self.reverse_wrap_up(xt_rev_wrap2);
            return;
        }

        let row_idx = self.cursor.pos.y;

        if row_idx >= self.rows.len() {
            return;
        }

        let row = &self.rows[row_idx];

        // When in pending-wrap state the internal x equals self.width (one past
        // the last column).  A real VT100 keeps the cursor at the last column with
        // a separate pending-wrap bit; BS clears the bit and moves left from the
        // reported column, landing at width-2 (0-based).  Clamp before subtracting.
        let effective_x = self.cursor.pos.x.min(self.width.saturating_sub(1));
        let mut new_x = effective_x - 1;

        // Skip left over continuation cells of a wide glyph
        while new_x > 0 {
            if let Some(cell) = row.char_at(new_x) {
                if !cell.is_continuation() {
                    break;
                }
            } else {
                break;
            }
            new_x -= 1;
        }

        self.cursor.pos.x = new_x;

        // This MUST be the only post-condition for backspace.
        debug_assert!(self.cursor.pos.y < self.rows.len());
        debug_assert!(self.cursor.pos.x < self.width);
    }

    /// Move the cursor up one row and to the last column (reverse wrap).
    ///
    /// If the cursor is already at the top of the visible screen and
    /// `into_scrollback` is true, the cursor enters the scrollback region.
    /// If at the top and `into_scrollback` is false, the cursor stays put.
    fn reverse_wrap_up(&mut self, into_scrollback: XtRevWrap2) {
        let visible_start = self.visible_window_start(0);

        if self.cursor.pos.y > visible_start {
            // Not at the top of the visible screen — wrap to previous row.
            self.cursor.pos.y -= 1;
            self.cursor.pos.x = self.width.saturating_sub(1);
        } else if into_scrollback == XtRevWrap2::Enabled && self.cursor.pos.y > 0 {
            // At top of visible screen, but scrollback exists and
            // extended reverse-wrap is enabled.
            self.cursor.pos.y -= 1;
            self.cursor.pos.x = self.width.saturating_sub(1);
        }
        // Otherwise: at absolute top or scrollback not allowed — no-op.
    }

    /// Fill the entire visible screen with 'E' characters (DECALN — ESC # 8).
    ///
    /// Also resets the scroll region to full screen and moves the cursor
    /// to the home position (0, 0).
    pub fn screen_alignment_test(&mut self) {
        use freminal_common::buffer_states::format_tag::FormatTag;
        use freminal_common::buffer_states::tchar::TChar;

        let visible_start = self.visible_window_start(0);
        let visible_end = visible_start + self.height;

        // Ensure we have enough rows
        while self.rows.len() < visible_end {
            self.rows.push(crate::row::Row::new(self.width));
            self.row_cache.push(None);
        }

        let default_tag = FormatTag::default();
        let e_chars: Vec<TChar> = vec![TChar::Ascii(b'E'); self.width];
        for i in visible_start..visible_end.min(self.rows.len()) {
            self.image_cell_count -= self.rows[i].count_image_cells();
            self.rows[i].clear();
            self.rows[i].insert_text(0, &e_chars, &default_tag);
            // Invalidate row cache
            if i < self.row_cache.len() {
                self.row_cache[i] = None;
            }
        }

        // Reset scroll region to full screen
        self.scroll_region_top = 0;
        self.scroll_region_bottom = self.height.saturating_sub(1);

        // Move cursor to home position
        self.cursor.pos.x = 0;
        self.cursor.pos.y = visible_start;
    }

    /// Handle ANSI LF (line feed), IND-style advance, and LNM mode.
    ///
    /// Moves the cursor down one row within the current `DECSTBM` scroll region.
    /// When the cursor is at the bottom margin the region scrolls up by one line.
    /// In `LNM` (new-line) mode an implicit CR is also applied.
    pub fn handle_lf(&mut self) {
        // Clear the implicit pending-wrap state.  When the cursor is at
        // `x == width` (one past the last column) it means a character was
        // just written at the rightmost column and the next printable
        // character should wrap.  LF must cancel that deferred wrap —
        // the cursor stays at the last column of the current/new row
        // rather than wrapping on the next character write.
        if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }

        match self.kind {
            BufferType::Primary => {
                // LNM: CR implied
                if self.lnm_enabled == Lnm::NewLine {
                    self.cursor.pos.x = 0;
                }

                let sy = self.cursor_screen_y();

                // --- DECSTBM applies ONLY when:
                //     - region is not full screen
                //     - not scrolled back (already ensured above)
                if self.scroll_region_top == 0 && self.scroll_region_bottom == self.height - 1 {
                    //
                    // FAST PATH: FULL-SCREEN REGION
                    //
                    // When the cursor is above the last visible row we simply move
                    // it down.  We must NOT clear the destination row if it already
                    // holds real content (e.g. logo lines written in a previous pass
                    // before a CUU brought the cursor back up).  We only clear when
                    // the destination is a pristine ScrollFill placeholder, and we
                    // only scroll-in a brand-new blank row when the cursor is at the
                    // very bottom of the visible window.
                    self.cursor.pos.y += 1;

                    if self.cursor.pos.y >= self.rows.len() {
                        // Row doesn't exist yet — always create it fresh.
                        // BCE: fill with current SGR background.
                        let mut new_row = Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        );
                        new_row.fill_with_tag(&self.current_tag);
                        self.rows.push(new_row);
                        self.row_cache.push(None);
                    } else {
                        let row = &mut self.rows[self.cursor.pos.y];
                        if row.origin == RowOrigin::ScrollFill {
                            // Pristine placeholder: stamp it as a real hard-break
                            // line but leave its (empty) cell content alone.
                            row.origin = RowOrigin::HardBreak;
                            row.join = RowJoin::NewLogicalLine;
                        } else if sy == self.height.saturating_sub(1) {
                            // Cursor was at the bottom of the visible window and the
                            // next slot already has content from old scrollback — this
                            // is the newly-scrolled-in line, so wipe it.  Use default
                            // background (no BCE) — same rationale as `push_row`.
                            self.image_cell_count -= row.count_image_cells();
                            row.origin = RowOrigin::HardBreak;
                            row.join = RowJoin::NewLogicalLine;
                            row.clear();
                        }
                        // Otherwise (cursor was above the bottom, row has real
                        // content): leave the row completely untouched.
                    }

                    // PTY always at scroll_offset=0; return value is always 0 here.
                    let _ = self.enforce_scrollback_limit(0);
                    self.debug_assert_invariants();
                    return;
                }

                //
                // PARTIAL REGION (true DECSTBM behavior)
                //
                if sy >= self.scroll_region_top && sy <= self.scroll_region_bottom {
                    if sy < self.scroll_region_bottom {
                        // Move cursor down inside region
                        self.cursor.pos.y += 1;
                        // If the row doesn't exist yet (buffer filling up), create it.
                        while self.cursor.pos.y >= self.rows.len() {
                            self.push_row(RowOrigin::HardBreak, RowJoin::NewLogicalLine);
                        }
                    } else {
                        // At bottom margin → scroll region UP
                        self.scroll_region_up_primary();
                        // cursor stays in place
                    }
                } else {
                    // Outside DECSTBM → just move down with no scrolling
                    if self.cursor.pos.y + 1 < self.rows.len() {
                        self.cursor.pos.y += 1;
                    } else {
                        let mut new_row = Row::new_with_origin(
                            self.width,
                            RowOrigin::HardBreak,
                            RowJoin::NewLogicalLine,
                        );
                        new_row.fill_with_tag(&self.current_tag);
                        self.rows.push(new_row);
                        self.row_cache.push(None);
                        self.cursor.pos.y = self.rows.len() - 1;
                    }
                }

                // PTY always at scroll_offset=0; return value is always 0 here.
                let _ = self.enforce_scrollback_limit(0);
                self.debug_assert_invariants();
            }

            BufferType::Alternate => {
                if self.lnm_enabled == Lnm::NewLine {
                    self.cursor.pos.x = 0;
                }

                let y = self.cursor.pos.y;

                if y >= self.scroll_region_top && y <= self.scroll_region_bottom {
                    if y < self.scroll_region_bottom {
                        if self.cursor.pos.y + 1 < self.height {
                            self.cursor.pos.y += 1;
                        }
                    } else {
                        self.scroll_slice_up(self.scroll_region_top, self.scroll_region_bottom);
                    }
                } else if self.cursor.pos.y + 1 < self.height {
                    self.cursor.pos.y += 1;
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// Handle ANSI CR (carriage return) — move cursor to column 0.
    pub const fn handle_cr(&mut self) {
        self.cursor.pos.x = 0;
    }

    /// IND – Index (move down, scroll within DECSTBM region).
    /// Same as LF, but *does not* honor LNM (no implicit CR).
    pub fn handle_ind(&mut self) {
        // Temporarily disable LNM so `handle_lf` won't do CR.
        let old_lnm = self.lnm_enabled;
        self.lnm_enabled = Lnm::LineFeed;
        self.handle_lf();
        self.lnm_enabled = old_lnm;
    }

    /// NEL – Next Line (CR + LF with scrolling in DECSTBM region).
    pub fn handle_nel(&mut self) {
        // Explicit CR then LF – this is allowed to honor LNM.
        self.handle_cr();
        self.handle_lf();
    }

    /// RI – Reverse Index.
    /// Move the cursor up; at the top margin of DECSTBM region,
    /// scroll the region down by one line (blank line at top).
    pub fn handle_ri(&mut self) {
        // Clear implicit pending-wrap state (same as handle_lf).
        if self.cursor.pos.x >= self.width {
            self.cursor.pos.x = self.width.saturating_sub(1);
        }

        match self.kind {
            BufferType::Primary => {
                let sy = self.cursor_screen_y();

                if sy >= self.scroll_region_top && sy <= self.scroll_region_bottom {
                    if sy > self.scroll_region_top {
                        // move up inside region
                        self.cursor.pos.y -= 1;
                    } else {
                        // at top margin → scroll region DOWN
                        self.scroll_region_down_primary();
                    }
                } else {
                    // outside region → never scroll
                    if self.cursor.pos.y > 0 {
                        self.cursor.pos.y -= 1;
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Alternate => {
                let y = self.cursor.pos.y;

                if y >= self.scroll_region_top && y <= self.scroll_region_bottom {
                    if y > self.scroll_region_top {
                        self.cursor.pos.y -= 1;
                    } else {
                        self.scroll_slice_down(self.scroll_region_top, self.scroll_region_bottom);
                    }
                } else if self.cursor.pos.y > 0 {
                    self.cursor.pos.y -= 1;
                }
                self.debug_assert_invariants();
            }
        }
    }

    /// IL – Insert Lines within DECSTBM region.
    /// Insert `n` blank lines at the cursor row, shifting lines down and
    /// discarding at the bottom of the region.
    pub fn insert_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        match self.kind {
            BufferType::Alternate => {
                // leave as-is
                let y = self.cursor.pos.y;
                if y < self.scroll_region_top || y > self.scroll_region_bottom {
                    return;
                }

                let max_lines = self.scroll_region_bottom.saturating_sub(y) + 1;
                let count = n.min(max_lines);

                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_down_columns(y, self.scroll_region_bottom, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_down(y, self.scroll_region_bottom);
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_down_columns(row, b, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_down(row, b);
                    }
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// DL – Delete Lines within DECSTBM region.
    /// Delete `n` lines at the cursor row, shifting lines up and
    /// inserting blanks at the bottom of the region.
    pub fn delete_lines(&mut self, n: usize) {
        if n == 0 {
            return;
        }

        match self.kind {
            BufferType::Alternate => {
                let y = self.cursor.pos.y;
                if y < self.scroll_region_top || y > self.scroll_region_bottom {
                    return;
                }

                let max_lines = self.scroll_region_bottom.saturating_sub(y) + 1;
                let count = n.min(max_lines);

                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_up_columns(y, self.scroll_region_bottom, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_up(y, self.scroll_region_bottom);
                    }
                }

                self.debug_assert_invariants();
            }

            BufferType::Primary => {
                let sy = self.cursor_screen_y();
                if sy < self.scroll_region_top || sy > self.scroll_region_bottom {
                    return;
                }

                let (t, b) = self.scroll_region_rows();
                let offset = sy - self.scroll_region_top;
                let row = t + offset;

                let count = n.min(b - row + 1);
                if self.declrmm_enabled == Declrmm::Enabled {
                    let (left, right) = (self.scroll_region_left, self.scroll_region_right);
                    for _ in 0..count {
                        self.scroll_slice_up_columns(row, b, left, right);
                    }
                } else {
                    for _ in 0..count {
                        self.scroll_slice_up(row, b);
                    }
                }

                self.debug_assert_invariants();
            }
        }
    }

    /// Implements ICH – Insert Characters (spaces).
    pub fn insert_spaces(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            return;
        }

        // Sweep any image cells in the range that will be shifted off the right
        // edge before the insert happens.  Cells shifted past the right margin
        // are discarded, so we must update image_cell_count now.
        let remaining_images = if self.image_cell_count > 0 {
            let right = if self.declrmm_enabled == Declrmm::Enabled {
                self.scroll_region_right + 1
            } else {
                self.width
            };
            self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(right));
            // Count Kitty images that survived the sweep in the cells that
            // will overflow past the right edge when we insert `n` blanks.
            let overflow_start = right.saturating_sub(n).max(col);
            self.rows[row].count_image_cells_in_range(overflow_start, right)
        } else {
            0
        };

        let tag = self.current_tag.clone();
        if self.declrmm_enabled == Declrmm::Enabled {
            // ICH: shift only within [col, scroll_region_right]; cells that
            // fall off the right margin are discarded.
            self.rows[row].insert_spaces_at_with_right_limit(
                col,
                n,
                &tag,
                self.scroll_region_right + 1,
            );
        } else {
            self.rows[row].insert_spaces_at(col, n, &tag);
        }

        self.image_cell_count -= remaining_images;
        // Cursor does NOT move for ICH
    }

    /// Implements DCH – Delete Characters.
    ///
    /// Removes `n` cells starting at the cursor column on the cursor row, shifting
    /// the cells to the right of the deleted range left to fill the gap. Cells
    /// shifted off the right edge are discarded.  The cursor does not move.
    ///
    /// When DECLRMM is active, the operation is confined to
    /// `[col, scroll_region_right]`; cells outside the right margin are
    /// unaffected.
    ///
    /// Wide-glyph cleanup is delegated to [`Row::delete_cells_at`] /
    /// [`Row::delete_cells_at_with_right_limit`].
    pub fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.pos.y;
        let col = self.cursor.pos.x;

        if row >= self.rows.len() {
            // Row doesn't even exist — nothing to delete.
            return;
        }

        // Sweep image cells in the range being deleted before the row-level
        // operation removes them without updating image_cell_count.
        let remaining_images = if self.image_cell_count > 0 {
            let end_col = if self.declrmm_enabled == Declrmm::Enabled {
                (col + n).min(self.scroll_region_right + 1)
            } else {
                col + n
            };
            self.collect_and_clear_image_ids_in_rows(row, row + 1, Some(col), Some(end_col));
            // Count Kitty images that survived the sweep in the deleted range.
            self.rows[row].count_image_cells_in_range(col, end_col)
        } else {
            0
        };

        if self.declrmm_enabled == Declrmm::Enabled {
            self.rows[row].delete_cells_at_with_right_limit(
                col,
                n,
                self.scroll_region_right + 1,
                &self.current_tag.clone(),
            );
        } else {
            let tag = self.current_tag.clone();
            self.rows[row].delete_cells_at(col, n, &tag);
        }

        self.image_cell_count -= remaining_images;
        self.debug_assert_invariants();
    }
}
