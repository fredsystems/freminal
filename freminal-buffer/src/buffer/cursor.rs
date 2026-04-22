// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cursor position and state operations for [`Buffer`].
//!
//! Covers absolute cursor placement (`set_cursor_pos`, `set_cursor_pos_raw`),
//! relative movement (`move_cursor_relative`), screen-coordinate projection
//! (`get_cursor_screen_pos`, `cursor_screen_y`), and DECSC/DECRC save/restore.

use freminal_common::buffer_states::{
    cursor::CursorPos,
    modes::{declrmm::Declrmm, decom::Decom},
};

use crate::row::{RowJoin, RowOrigin};

use super::clamped_offset;
use crate::buffer::Buffer;

impl Buffer {
    /// Set the cursor to an absolute buffer position without any DECOM or
    /// screen-relative translation.
    ///
    /// The position is clamped to the current buffer dimensions.  Used by
    /// DECSDM to restore the cursor after `place_image` moves it.
    pub fn set_cursor_pos_raw(&mut self, pos: CursorPos) {
        self.cursor.pos.x = if self.width > 0 {
            pos.x.min(self.width - 1)
        } else {
            0
        };
        self.cursor.pos.y = pos.y.min(self.rows.len().saturating_sub(1));
    }

    /// Set the [`LineWidth`] attribute on the row under the cursor.
    ///
    /// This is the buffer-level primitive called by the terminal handler in
    /// response to `ESC # 3` (double-height top), `ESC # 4` (double-height
    /// bottom), `ESC # 5` (single-width), and `ESC # 6` (double-width).
    ///
    /// The row is marked dirty so the next snapshot rebuild re-flattens it.
    pub fn set_cursor_line_width(&mut self, lw: crate::row::LineWidth) {
        let row_idx = self.cursor.pos.y;
        if let Some(row) = self.rows.get_mut(row_idx)
            && row.line_width != lw
        {
            row.line_width = lw;
            row.dirty = true;
        }
    }

    /// Return the cursor position in **screen coordinates** (0-indexed, relative
    /// to the top of the visible window).
    ///
    /// Unlike `get_cursor().pos.y`, which is an absolute index into `self.rows`,
    /// this subtracts `visible_window_start()` so the result is always in the
    /// range `0..height` and matches what the GUI painter expects.
    #[must_use]
    pub fn get_cursor_screen_pos(&self) -> CursorPos {
        let screen_y = self.cursor_screen_y();
        CursorPos {
            x: self.cursor.pos.x,
            y: screen_y,
        }
    }

    /// Move cursor to absolute position (CUP, HVP).
    ///
    /// `x` and `y` are 0-indexed screen coordinates.  `None` means "leave this
    /// axis unchanged" (e.g. CHA only supplies x, VPA only supplies y, CUP
    /// supplies both).
    ///
    /// When DECOM (origin mode) is enabled, `y` is relative to `scroll_region_top`
    /// and is clamped to the scroll region height.  When DECOM is disabled, `y` is
    /// relative to the top of the visible window and clamped to the screen height.
    pub fn set_cursor_pos(&mut self, x: Option<usize>, y: Option<usize>) {
        // `None` means "leave this axis unchanged" (e.g. CHA only supplies x,
        // VPA only supplies y, CUP supplies both).
        let new_x = match x {
            Some(col) => col.min(self.width.saturating_sub(1)),
            None => self.cursor.pos.x,
        };

        // y is a screen-relative coordinate (0 = top of visible window in normal
        // mode, or 0 = top of scroll region in DECOM mode).
        let new_buffer_y = match y {
            Some(row) => {
                if self.decom_enabled == Decom::OriginMode {
                    // DECOM: row is relative to scroll_region_top, clamped to
                    // the scroll region height.
                    let region_height = self
                        .scroll_region_bottom
                        .saturating_sub(self.scroll_region_top);
                    let clamped = row.min(region_height);
                    let screen_row = self.scroll_region_top + clamped;
                    self.visible_window_start(0) + screen_row
                } else {
                    let clamped = row.min(self.height.saturating_sub(1));
                    // PTY always operates at live bottom (scroll_offset = 0)
                    self.visible_window_start(0) + clamped
                }
            }
            None => self.cursor.pos.y,
        };

        // Ensure rows exist up to the target position
        while new_buffer_y >= self.rows.len() {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }

        self.cursor.pos.x = new_x;
        self.cursor.pos.y = new_buffer_y;
        self.debug_assert_invariants();
    }

    /// Move cursor relatively (CUU, CUD, CUF, CUB)
    /// Move the cursor by a relative offset `(dx, dy)` in screen coordinates.
    ///
    /// Positive `dx` moves right; negative moves left. Positive `dy` moves down; negative moves
    /// up.  The cursor is clamped to the visible screen boundaries and never enters scrollback.
    pub fn move_cursor_relative(&mut self, dx: i32, dy: i32) {
        // When DECLRMM is active and moving horizontally, clamp to the
        // left/right margins if the cursor is currently within the margin zone.
        let new_x = if self.declrmm_enabled == Declrmm::Enabled && dx != 0 {
            let cx = self.cursor.pos.x;
            if cx >= self.scroll_region_left && cx <= self.scroll_region_right {
                // Cursor is inside the margin zone: clamp to [left, right].
                clamped_offset(cx, dx, self.scroll_region_left, self.scroll_region_right)
            } else {
                // Cursor is outside the margin zone: use normal full-width clamp.
                clamped_offset(cx, dx, 0, self.width.saturating_sub(1))
            }
        } else {
            clamped_offset(self.cursor.pos.x, dx, 0, self.width.saturating_sub(1))
        };

        let current_screen_y = self.cursor_screen_y();
        let new_screen_y = clamped_offset(current_screen_y, dy, 0, self.height.saturating_sub(1));

        // PTY always operates at live bottom (scroll_offset = 0)
        let new_buffer_y = self.visible_window_start(0) + new_screen_y;

        // Ensure rows exist
        while new_buffer_y >= self.rows.len() {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }

        self.cursor.pos.x = new_x;
        self.cursor.pos.y = new_buffer_y;
        self.debug_assert_invariants();
    }

    /// Implements DECSC – Save Cursor.
    ///
    /// Saves the current cursor position (and associated `CursorState`).
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor.clone());
    }

    /// Implements DECRC – Restore Cursor.
    ///
    /// Restores the previously saved cursor position.  If no cursor has been
    /// saved, this is a no-op.  The restored position is clamped to the
    /// current buffer dimensions so a resize between save and restore never
    /// produces an out-of-bounds cursor.
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor.clone() {
            self.cursor = saved;
            // Clamp to current dimensions after restore.
            if self.width > 0 {
                self.cursor.pos.x = self.cursor.pos.x.min(self.width - 1);
            }
            let max_row = self.rows.len().saturating_sub(1);
            self.cursor.pos.y = self.cursor.pos.y.min(max_row);
            self.debug_assert_invariants();
        }
        // No saved cursor → silent no-op.
    }

    /// Cursor Y expressed in "screen coordinates" (0..height-1).
    /// If the buffer is shorter than the height, we just return the raw Y.
    /// Always computed relative to the live bottom (`scroll_offset` = 0), because the
    /// PTY thread only ever mutates the buffer at the live bottom.
    pub(in crate::buffer) fn cursor_screen_y(&self) -> usize {
        if self.rows.is_empty() || self.height == 0 {
            return 0;
        }

        let start = self.visible_window_start(0);
        self.cursor.pos.y.saturating_sub(start)
    }
}
