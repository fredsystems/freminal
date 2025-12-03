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
#[test]
fn decstbm_lf_scrolls_region_up_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    // Build tagged rows: lengths [1,2,3,4,5]
    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Set scroll region to rows 2..4 (1-based: 2..4 => 0-based: 1..3)
    buf.set_scroll_region(2, 4);

    // Cursor is at last row after build_tagged_screen (row 4).
    // Move it up once via RI; for PRIMARY buffer, since it's outside
    // the region initially (y=4, region=1..3), this will just move
    // cursor to y=3 without scrolling.
    buf.handle_ri();

    // Now cursor is on bottom margin of the region (screen row 3 / 0-based index 3)
    // LF should scroll the region up by one line.
    buf.handle_lf();

    let lengths = visible_lengths(&buf);
    // Original lengths: [1,2,3,4,5]
    // Region: indices 1..3 => [2,3,4]
    // After scrolling up within region:
    //   [2,3,4] -> [3,4,0]
    // So full visible lengths should be: [1,3,4,0,5]
    assert_eq!(lengths, vec![1, 3, 4, 0, 5]);
}

/// DECSTBM + RI in PRIMARY buffer scrolls region down.
#[test]
fn decstbm_ri_scrolls_region_down_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    // Build tagged rows: [1,2,3,4,5]
    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Region rows 2..4 (1-based: 2..4)
    buf.set_scroll_region(2, 4);

    // Move cursor to TOP margin of region: row 1 (screen coordinates).
    // After build_tagged_screen, cursor is at row 4.
    // We can reach row 1 via three RI operations:
    buf.handle_ri(); // y: 4 -> 3 (outside region branch)
    buf.handle_ri(); // y: 3 -> 2 (now inside region but y>top, so move up)
    buf.handle_ri(); // y: 2 -> 1 (inside, now at top margin)

    // Now at top margin; RI should scroll the region DOWN.
    buf.handle_ri();

    let lengths = visible_lengths(&buf);
    // Region indices 1..3 with lengths [2,3,4]
    // After scroll DOWN:
    //   [2,3,4] -> [0,2,3]
    // Full: [1,0,2,3,5]
    assert_eq!(lengths, vec![1, 0, 2, 3, 5]);
}

/// DECSTBM IL (Insert Lines) in PRIMARY buffer.
#[test]
fn decstbm_insert_lines_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    // Region: rows 2..4
    buf.set_scroll_region(2, 4);

    // Move cursor to row 2 (screen index 1), inside region.
    // Currently at row 4; two RI steps:
    buf.handle_ri(); // 4 -> 3
    buf.handle_ri(); // 3 -> 2

    // Insert 1 line inside region at row index 1.
    buf.insert_lines(1);

    let lengths = visible_lengths(&buf);
    // Region [2,3,4] → [0,2,3], discarding the last row of the region.
    // Full: [1,0,2,3,5]
    assert_eq!(lengths, vec![1, 0, 2, 3, 5]);
}

/// DECSTBM DL (Delete Lines) in PRIMARY buffer.
#[test]
fn decstbm_delete_lines_primary() {
    let width = 10;
    let height = 5;
    let mut buf = Buffer::new(width, height);

    build_tagged_screen(&mut buf, width, height);
    assert_eq!(visible_lengths(&buf), vec![1, 2, 3, 4, 5]);

    buf.set_scroll_region(2, 4);

    // Move cursor to row 2 (screen index 1)
    buf.handle_ri(); // 4 -> 3
    buf.handle_ri(); // 3 -> 2

    buf.delete_lines(1);

    let lengths = visible_lengths(&buf);
    // Region [2,3,4] -> delete line at row index 1:
    //   effectively [3,4,0]
    // Full: [1,3,4,0,5]
    assert_eq!(lengths, vec![1, 3, 4, 0, 5]);
}
