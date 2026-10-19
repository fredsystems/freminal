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
    /// the rightmost column.  HT never wraps to the next line.  A stale
    /// column beyond the table is handled the same way (the search finds no
    /// stop, so the cursor lands on the rightmost column).
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
    ///
    /// The starting column is clamped to the tab-stop table's length before
    /// the table is sliced. `cursor.pos.x` is at most `width` (the
    /// pending-wrap position) and `tab_stops.len() == width`, so for a
    /// consistent buffer this changes nothing; it only stops a stale column
    /// (e.g. one stranded by a resize) from indexing past the table, which
    /// used to panic (GHSA-8f32-mj37-fw6v). A clamped start behaves as
    /// "from the right margin": the previous stop is found at or before the
    /// last column.
    ///
    /// `count` is attacker-controlled, but the loop always makes progress or
    /// ends: every iteration either breaks at column 0 or moves `col` to a
    /// strictly smaller column (`rposition` over `..col` yields an index
    /// below `col`) or to 0 when no earlier stop exists, after which the next
    /// iteration breaks. It therefore runs at most `width + 1` times
    /// whatever `count` is.
    pub fn tab_backward(&mut self, count: usize) {
        let mut col = self.cursor.pos.x.min(self.tab_stops.len());
        for _ in 0..count {
            // Search backward from current column
            if col == 0 {
                break;
            }
            let prev = self
                .tab_stops
                .get(..col)
                .and_then(|stops| stops.iter().rposition(|&is_stop| is_stop));
            col = prev.unwrap_or(0);
        }
        self.cursor.pos.x = col;
    }

    /// Move cursor forward by `count` tab stops (CHT — CSI I).
    ///
    /// If there are fewer than `count` tab stops remaining, moves to the
    /// rightmost column.  CHT never wraps to the next line.
    ///
    /// `count` comes straight from the escape sequence and can be as large
    /// as `usize::MAX`, so the loop must not run `count` times
    /// unconditionally. [`Buffer::advance_to_next_tab_stop`] is a pure
    /// function of the cursor column, the tab-stop table and the width, and
    /// its only effect is to move the cursor column: once a step leaves the
    /// column unchanged, every further step would too, so the loop stops
    /// there. Each productive step strictly increases the column (or, for a
    /// stale column past the table, drops it to the last column once), so the
    /// loop runs at most `width + 1` times.
    pub fn tab_forward(&mut self, count: usize) {
        for _ in 0..count {
            let before = self.cursor.pos.x;
            self.advance_to_next_tab_stop();
            if self.cursor.pos.x == before {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 28-column buffer (stops at 8, 16, 24) whose cursor has been stranded
    /// at `stale_x`, past the right edge. No operation should produce this
    /// any more; the tab operations must still tolerate it, because a
    /// producer that leaks one would otherwise turn into a panic (CBT sliced
    /// the 28-entry table at 99 -- GHSA-8f32-mj37-fw6v).
    fn stranded(stale_x: usize) -> Buffer {
        let mut buf = Buffer::new(28, 5);
        buf.cursor.pos.x = stale_x;
        buf
    }

    #[test]
    fn tab_backward_from_stale_column_does_not_panic() {
        let mut buf = stranded(99);
        buf.tab_backward(1);
        // Treated as "from the right edge": the previous stop is the last one
        // in the table.
        assert_eq!(buf.cursor.pos.x, 24);
    }

    #[test]
    fn tab_backward_from_stale_column_walks_the_table() {
        let mut buf = stranded(99);
        buf.tab_backward(3);
        assert_eq!(buf.cursor.pos.x, 8);
        buf.tab_backward(100);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn tab_backward_zero_count_from_stale_column_lands_in_bounds() {
        let mut buf = stranded(99);
        buf.tab_backward(0);
        assert!(buf.cursor.pos.x <= buf.width);
    }

    #[test]
    fn tab_backward_from_stale_column_with_no_stops_goes_home() {
        let mut buf = stranded(99);
        buf.clear_all_tab_stops();
        buf.tab_backward(1);
        assert_eq!(buf.cursor.pos.x, 0);
    }

    #[test]
    fn tab_backward_in_range_is_unchanged() {
        let mut buf = stranded(20);
        buf.tab_backward(1);
        assert_eq!(buf.cursor.pos.x, 16);
        // Pending-wrap position (x == width) is a legitimate start.
        buf.cursor.pos.x = buf.width;
        buf.tab_backward(1);
        assert_eq!(buf.cursor.pos.x, 24);
    }

    #[test]
    fn tab_forward_from_stale_column_goes_to_the_last_column() {
        let mut buf = stranded(99);
        buf.tab_forward(1);
        assert_eq!(buf.cursor.pos.x, 27);
        let mut buf = stranded(99);
        buf.advance_to_next_tab_stop();
        assert_eq!(buf.cursor.pos.x, 27);
    }

    /// Run `tab_forward(count)` from column 0 on an 80-column buffer
    /// (stops at 8, 16, ... 72) and return the resulting column.
    fn forward_from_origin(count: usize) -> usize {
        let mut buf = Buffer::new(80, 5);
        buf.tab_forward(count);
        buf.cursor.pos.x
    }

    #[test]
    fn tab_forward_usize_max_lands_on_the_last_column() {
        // Hangs on the unfixed loop (CHT with a huge Ps, Task 148.C2).
        assert_eq!(forward_from_origin(usize::MAX), 79);
        assert_eq!(forward_from_origin(usize::MAX), forward_from_origin(100));
    }

    #[test]
    fn tab_forward_small_counts_walk_the_stops() {
        // 9 stops lie ahead of column 0: 8, 16, ..., 72.
        assert_eq!(forward_from_origin(0), 0);
        assert_eq!(forward_from_origin(1), 8);
        assert_eq!(forward_from_origin(2), 16);
        assert_eq!(forward_from_origin(9), 72);
        // One more than the number of stops: the right margin.
        assert_eq!(forward_from_origin(10), 79);
        assert_eq!(forward_from_origin(11), 79);
    }

    #[test]
    fn tab_forward_without_stops_goes_to_the_last_column() {
        let mut buf = Buffer::new(80, 5);
        buf.clear_all_tab_stops();
        buf.tab_forward(usize::MAX);
        assert_eq!(buf.cursor.pos.x, 79);
    }

    #[test]
    fn tab_forward_huge_count_from_stale_column_lands_in_bounds() {
        let mut buf = stranded(99);
        buf.tab_forward(usize::MAX);
        assert_eq!(buf.cursor.pos.x, 27);
    }

    #[test]
    fn tab_backward_usize_max_goes_home() {
        let mut buf = Buffer::new(80, 5);
        buf.cursor.pos.x = 79;
        buf.tab_backward(usize::MAX);
        assert_eq!(buf.cursor.pos.x, 0);
        // No earlier stop at all: straight to column 0.
        let mut buf = Buffer::new(80, 5);
        buf.clear_all_tab_stops();
        buf.cursor.pos.x = 40;
        buf.tab_backward(usize::MAX);
        assert_eq!(buf.cursor.pos.x, 0);
    }
}
