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
    buffer_type::BufferType,
    cursor::{CursorPos, CursorState},
    modes::{declrmm::Declrmm, decom::Decom},
    row_number::RowNumber,
};

use crate::row::{RowJoin, RowOrigin};

use super::clamped_offset;
use crate::buffer::Buffer;

/// A DECSC-saved cursor.
///
/// DECSC saves the cursor's *screen* position, not a place in the content
/// (Task 125.C6): xterm stores `screen->cur_row`, a row relative to the top of
/// the screen, and DECRC restores it through `CursorSet`, which clamps it to
/// the screen. The VT510 manual defines the saved item only as "cursor
/// position" on a terminal that has no scrollback.
///
/// `cursor.pos` is therefore in **screen coordinates** (row 0 is the top of
/// the visible window), unlike the live cursor, whose `pos.y` indexes the
/// retained rows. Because the saved row never names a stored row, scrolling,
/// eviction, reflow and a switch between the primary and alternate screens
/// cannot invalidate it; a resize only matters through the clamp on restore.
#[derive(Debug, Clone)]
pub(in crate::buffer) struct SavedCursor {
    /// The cursor state at the time of the save, with `pos` in screen
    /// coordinates.
    pub(in crate::buffer) cursor: CursorState,
}

impl Buffer {
    /// Logical number of the oldest retained row of the active screen.
    ///
    /// Row `i` of [`Self::rows`] is numbered `row_base() + i`. Advances when
    /// rows are evicted from the front; never moves backwards.
    #[must_use]
    pub const fn row_base(&self) -> RowNumber {
        self.rows.base()
    }

    /// Logical number the next row appended to the active screen will get:
    /// `row_base() + rows().len()`.
    #[must_use]
    pub fn next_row_number(&self) -> RowNumber {
        self.rows.next_number()
    }

    /// The live row-number span `(base, next_number)` of the *parked* store of
    /// `screen`, or `None` when `screen` is the active screen or has never been
    /// parked (the alternate screen before its first use).
    ///
    /// A row of the parked screen is numbered `n` and still retained iff
    /// `base <= n < next_number`. This lets a caller that keeps state anchored
    /// to a parked screen's rows (the kitty placement maps) judge that state
    /// after a resize that trimmed or evicted rows of the parked store.
    #[must_use]
    pub fn parked_row_span(&self, screen: BufferType) -> Option<(RowNumber, RowNumber)> {
        if screen == self.kind {
            return None;
        }
        let parked = match screen {
            BufferType::Primary => self.parked_primary.as_ref(),
            BufferType::Alternate => self.parked_alternate.as_ref(),
        }?;
        Some((parked.rows.base(), parked.rows.next_number()))
    }

    /// Logical number of the row at retained index `index` of the active screen.
    ///
    /// Plain arithmetic: it does not check that `index` is in range.
    #[must_use]
    pub fn row_number_at(&self, index: usize) -> RowNumber {
        self.rows.number_of(index)
    }

    /// Retained index of the row numbered `number`, or `None` if it is no
    /// longer (or not yet) stored on the active screen: evicted, past the end,
    /// or numbered in the other screen's namespace.
    #[must_use]
    pub fn row_index_of(&self, number: RowNumber) -> Option<usize> {
        self.rows.index_of(number)
    }

    /// Logical number of the row the cursor is on.
    #[must_use]
    pub fn cursor_row_number(&self) -> RowNumber {
        self.rows.number_of(self.cursor.pos.y)
    }

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
    pub fn cursor_screen_pos(&self) -> CursorPos {
        let screen_y = self.cursor_screen_y();
        CursorPos {
            x: self.cursor.pos.x,
            y: screen_y,
        }
    }

    /// Retained index of the row at 0-based screen row `screen_row` of the live
    /// window (the one the PTY thread operates on), or `None` when that screen
    /// row does not exist: it is at or past the screen height, or the buffer has
    /// not grown a row there yet.
    ///
    /// The inverse of the subtraction in [`Self::cursor_screen_pos`]. Use it to
    /// resolve a protocol coordinate that names a screen cell (kitty graphics
    /// `d=p`/`d=q`) to a row of [`Self::rows`], rather than treating the screen
    /// coordinate as a buffer index, which is wrong whenever scrollback exists.
    #[must_use]
    pub fn screen_row_index(&self, screen_row: usize) -> Option<usize> {
        if screen_row >= self.height {
            return None;
        }
        let index = self.visible_window_start(0) + screen_row;
        (index < self.rows.len()).then_some(index)
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
    /// Saves the cursor's **screen** position (see [`SavedCursor`]) together
    /// with its associated `CursorState`.
    pub fn save_cursor(&mut self) {
        let mut cursor = self.cursor.clone();
        cursor.pos = self.cursor_screen_pos();
        self.saved_cursor = Some(SavedCursor { cursor });
    }

    /// Implements DECRC – Restore Cursor.
    ///
    /// Restores the previously saved cursor. If no cursor has been saved, this
    /// is a no-op.
    ///
    /// The saved position is screen-relative, so the cursor returns to the
    /// same screen row and column however much output has scrolled, evicted
    /// or reflowed the content in between (xterm `CursorRestore`). The
    /// position is clamped to the current screen dimensions, so a resize
    /// between save and restore never produces an out-of-bounds cursor, and
    /// the cursor is never placed in off-screen scrollback.
    ///
    /// The same screen position is used on whichever screen is active when
    /// DECRC runs, so a save made on one screen restores sensibly on the
    /// other.
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor.clone() {
            let buffer_y = self.buffer_row_for_screen_row(saved.cursor.pos.y);

            self.cursor = saved.cursor;
            // Clamp to current dimensions after restore.
            if self.width > 0 {
                self.cursor.pos.x = self.cursor.pos.x.min(self.width - 1);
            }
            self.cursor.pos.y = buffer_y;
            self.debug_assert_invariants();
        }
        // No saved cursor → silent no-op.
    }

    /// Index into `self.rows` of the live-window row at `screen_y`, clamped to
    /// the screen height, growing the store with `ScrollFill` rows until that
    /// row exists (as CUP does).
    ///
    /// Shared by DECRC and the alternate-screen switch, which both place the
    /// cursor at a screen row recorded on another store.
    pub(in crate::buffer) fn buffer_row_for_screen_row(&mut self, screen_y: usize) -> usize {
        let screen_y = screen_y.min(self.height.saturating_sub(1));
        let buffer_y = self.visible_window_start(0) + screen_y;

        while buffer_y >= self.rows.len() {
            self.push_row(RowOrigin::ScrollFill, RowJoin::NewLogicalLine);
        }
        buffer_y
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
