// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tab stop manipulation for [`Buffer`].
//!
//! Covers HT (horizontal tab), HTS (set tab stop), TBC (clear tab stops), CBT
//! (cursor backward tab), and CHT (cursor horizontal tab) operations. Tab
//! stops are stored as a `Vec<bool>` indexed by column; default stops are at
//! every 8 columns.

use crate::buffer::Buffer;

impl Buffer {
    /// Advance the cursor to the next tab stop (HT / 0x09).
    ///
    /// If the cursor is already at or past the last tab stop, it moves to
    /// the rightmost column.  HT never wraps to the next line.
    pub fn advance_to_next_tab_stop(&mut self) {
        let col = self.cursor.pos.x;
        let max_col = self.width.saturating_sub(1);

        // Search for the next tab stop after the current column
        let next = self
            .tab_stops
            .iter()
            .enumerate()
            .skip(col + 1)
            .find(|&(_, &is_stop)| is_stop)
            .map(|(i, _)| i);

        self.cursor.pos.x = next.map_or(max_col, |stop| stop.min(max_col));
    }

    /// Set a tab stop at the current cursor column (HTS — ESC H).
    pub fn set_tab_stop(&mut self) {
        let col = self.cursor.pos.x;
        if col < self.tab_stops.len() {
            self.tab_stops[col] = true;
        }
    }

    /// Clear the tab stop at the current cursor column (TBC Ps=0).
    pub fn clear_tab_stop_at_cursor(&mut self) {
        let col = self.cursor.pos.x;
        if col < self.tab_stops.len() {
            self.tab_stops[col] = false;
        }
    }

    /// Clear all tab stops (TBC Ps=3).
    pub fn clear_all_tab_stops(&mut self) {
        self.tab_stops.iter_mut().for_each(|s| *s = false);
    }

    /// Move cursor backward to the Ps-th previous tab stop (CBT — CSI Z).
    ///
    /// If there is no previous tab stop, moves to column 0.
    /// CBT never wraps to the previous line.
    pub fn tab_backward(&mut self, count: usize) {
        let mut col = self.cursor.pos.x;
        for _ in 0..count {
            // Search backward from current column
            if col == 0 {
                break;
            }
            let prev = self.tab_stops[..col].iter().rposition(|&is_stop| is_stop);
            col = prev.unwrap_or(0);
        }
        self.cursor.pos.x = col;
    }

    /// Move cursor forward by `count` tab stops (CHT — CSI I).
    ///
    /// If there are fewer than `count` tab stops remaining, moves to the
    /// rightmost column.  CHT never wraps to the next line.
    pub fn tab_forward(&mut self, count: usize) {
        for _ in 0..count {
            self.advance_to_next_tab_stop();
        }
    }
}
