// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::tchar::TChar;

/// Helper: build a screen where row `i` (0-based) has `i+1` cells.
/// This gives each row a unique "signature" without needing to inspect the glyph itself.
fn build_tagged_screen(buf: &mut Buffer, width: usize, height: usize) {
    // Safety: we assume `height > 0`, `width > 0`.
    for row_idx in 0..height {
        // Insert (row_idx + 1) glyphs on this row.
        let count = (row_idx + 1).min(width);
        let mut line: Vec<TChar> = Vec::with_capacity(count);
        for _ in 0..count {
            line.push(TChar::new_from_single_char(b'X'));
        }

        buf.insert_text(&line);

        // Move to next line, except after the last row.
        if row_idx + 1 < height {
            buf.handle_lf();
            buf.handle_cr(); // Reset cursor X position
        }
    }
}

/// Helper: Get visible rows' "length signatures".
fn visible_lengths(buf: &Buffer) -> Vec<usize> {
    buf.visible_rows()
        .iter()
        .map(|row| row.get_characters().len())
        .collect()
}

/// Basic sanity: DECSTBM + LF in PRIMARY buffer scrolls region up.
///
/// Key insight: set_scroll_region moves cursor to the TOP of the region.
/// To test scrolling UP, we must move cursor to the BOTTOM of the region first.
#[test]
fn decstbm_lf_scrolls_region_up_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    // Build tagged rows: lengths [1,2,3,4,5]
    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Set scroll region to rows 2..4 (1-based: 2..4 => 0-based indices: 1..3)
    // This also moves cursor to y=1 (top of region)
    buf.set_scroll_region(2, 4);

    // Move cursor to bottom of region (y=3)
    // From y=1, we need 2 LF calls to reach y=3
    buf.handle_lf(); // y: 1 -> 2 (inside region, below top)
    buf.handle_lf(); // y: 2 -> 3 (inside region, at bottom)

    // Now cursor is at bottom margin (y=3)
    // Next LF should scroll the region up
    buf.handle_lf();

    let lengths = visible_lengths(&buf);
    // Original lengths: [1,2,3,4,5]
    // Region: indices 1..3 => [2,3,4]
    // After scrolling up within region:
    //   rows[1] = rows[2] (length 3)
    //   rows[2] = rows[3] (length 4)
    //   rows[3] = blank (length 0)
    // So full visible lengths should be: [1,3,4,0,5]
    assert_eq!(lengths, vec![1, 3, 4, 0, 5]);
}

/// DECSTBM + RI in PRIMARY buffer scrolls region down.
///
/// set_scroll_region moves cursor to TOP of region, which is perfect
/// for testing RI (reverse index) - it should scroll region DOWN.
#[test]
fn decstbm_ri_scrolls_region_down_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    // Build tagged rows: [1,2,3,4,5]
    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Region rows 2..4 (1-based: 2..4 => 0-based: 1..3)
    // This moves cursor to y=1 (top of region)
    buf.set_scroll_region(2, 4);

    // Cursor is now at TOP margin of region (y=1)
    // RI at top margin should scroll the region DOWN
    buf.handle_ri();

    let lengths = visible_lengths(&buf);
    // Region indices 1..3 with original lengths [2,3,4]
    // After scroll DOWN:
    //   rows[3] = rows[2] (length 3)
    //   rows[2] = rows[1] (length 2)
    //   rows[1] = blank (length 0)
    // Full: [1,0,2,3,5]
    assert_eq!(lengths, vec![1, 0, 2, 3, 5]);
}

/// DECSTBM IL (Insert Lines) in PRIMARY buffer.
///
/// Insert lines at cursor position within the scroll region,
/// shifting lines below down and discarding the bottom line of the region.
#[test]
fn decstbm_insert_lines_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Region: rows 2..4 (1-based) => indices 1..3
    // Cursor moves to y=1
    buf.set_scroll_region(2, 4);

    // Cursor is at y=1 (top of region)
    // Insert 1 line here should:
    // - Insert blank at y=1
    // - Shift y=1 -> y=2, y=2 -> y=3
    // - Discard original y=3
    buf.insert_lines(1);

    let lengths = visible_lengths(&buf);
    // Region [2,3,4] → insert blank at position 1:
    //   rows[1] = blank (0)
    //   rows[2] = old rows[1] (2)
    //   rows[3] = old rows[2] (3)
    //   (old rows[3] with length 4 is discarded)
    // Full: [1,0,2,3,5]
    assert_eq!(lengths, vec![1, 0, 2, 3, 5]);
}

/// DECSTBM DL (Delete Lines) in PRIMARY buffer.
///
/// Delete lines at cursor position within scroll region,
/// shifting lines below up and inserting blank at bottom of region.
#[test]
fn decstbm_delete_lines_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Region: rows 2..4 (1-based) => indices 1..3
    // Cursor moves to y=1
    buf.set_scroll_region(2, 4);

    // Cursor is at y=1 (top of region)
    // Delete 1 line here should:
    // - Delete line at y=1
    // - Shift y=2 -> y=1, y=3 -> y=2
    // - Insert blank at y=3
    buf.delete_lines(1);

    let lengths = visible_lengths(&buf);
    // Region [2,3,4] → delete line at position 1:
    //   rows[1] = old rows[2] (3)
    //   rows[2] = old rows[3] (4)
    //   rows[3] = blank (0)
    // Full: [1,3,4,0,5]
    assert_eq!(lengths, vec![1, 3, 4, 0, 5]);
}
