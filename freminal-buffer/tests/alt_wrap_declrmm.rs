// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Auto-wrap at the bottom of the scroll region on the alternate screen must
//! honour DECLRMM (Task 148.C12).
//!
//! xterm's `xtermScroll` (`util.c`) sets `scroll_full_line` to
//! `left == 0 && right == max_col`; when it is false the scroll is confined to
//! the left/right margins by `scrollInMargins`. The auto-wrap path
//! (`WrapLine` -> `xtermIndex` -> `xtermScroll`) takes that route on both screens, so
//! the alternate screen must not shift whole rows while DECLRMM margins are
//! narrower than the screen.

use freminal_buffer::buffer::Buffer;
use freminal_buffer::cell::Cell;
use freminal_common::buffer_states::modes::declrmm::Declrmm;
use freminal_common::buffer_states::tchar::TChar;

const WIDTH: usize = 10;
const HEIGHT: usize = 4;

fn text(s: &str) -> Vec<TChar> {
    s.bytes().map(TChar::Ascii).collect()
}

/// The text of row `row`, one char per column (blank cells as a space).
fn row_text(buf: &Buffer, row: usize) -> String {
    (0..WIDTH)
        .map(|col| {
            buf.rows()[row]
                .cells()
                .get(col)
                .map_or(' ', |cell: &Cell| match cell.tchar() {
                    TChar::Ascii(b) => char::from(*b),
                    _ => ' ',
                })
        })
        .collect()
}

/// Alternate screen, every row `r` filled with the digit `r`.
fn filled_alt_buffer() -> Buffer {
    let mut buf = Buffer::new(WIDTH, HEIGHT);
    buf.switch_to_alternate();
    for (row, digit) in "0123".chars().enumerate() {
        buf.set_cursor_pos(Some(0), Some(row));
        buf.insert_text(&text(&digit.to_string().repeat(WIDTH)));
    }
    buf
}

#[test]
fn alternate_wrap_scroll_is_confined_to_declrmm_margins() {
    let mut buf = filled_alt_buffer();
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_left_right_margins(2, 5); // 1-based -> 0-based columns [1, 4]
    buf.set_cursor_pos(Some(1), Some(HEIGHT - 1)); // left margin, bottom row

    // "abcd" fills the margins on the bottom row; "ef" wraps at the right
    // margin, which scrolls the (full-height) region up by one.
    buf.insert_text(&text("abcdef"));

    // Outside the margins (columns 0 and 5..9) nothing moved, on every row.
    // Inside, rows moved up by one; the bottom row holds the wrapped tail.
    assert_eq!(row_text(&buf, 0), "0111100000");
    assert_eq!(row_text(&buf, 1), "1222211111");
    assert_eq!(row_text(&buf, 2), "2abcd22222");
    assert_eq!(row_text(&buf, 3), "3ef  33333");
}

#[test]
fn alternate_wrap_scroll_shifts_whole_rows_without_declrmm() {
    let mut buf = filled_alt_buffer();
    buf.set_cursor_pos(Some(0), Some(HEIGHT - 1));

    // A full row plus one glyph: the wrap scrolls the whole screen up.
    buf.insert_text(&text("abcdefghijk"));

    assert_eq!(row_text(&buf, 0), "1111111111");
    assert_eq!(row_text(&buf, 1), "2222222222");
    assert_eq!(row_text(&buf, 2), "abcdefghij");
    assert_eq!(row_text(&buf, 3), "k         ");
}

#[test]
fn alternate_wrap_scroll_shifts_whole_rows_for_full_width_declrmm_margins() {
    let mut buf = filled_alt_buffer();
    buf.set_declrmm(Declrmm::Enabled);
    buf.set_cursor_pos(Some(0), Some(HEIGHT - 1));

    buf.insert_text(&text("abcdefghijk"));

    assert_eq!(row_text(&buf, 0), "1111111111");
    assert_eq!(row_text(&buf, 1), "2222222222");
    assert_eq!(row_text(&buf, 2), "abcdefghij");
    assert_eq!(row_text(&buf, 3), "k         ");
}
