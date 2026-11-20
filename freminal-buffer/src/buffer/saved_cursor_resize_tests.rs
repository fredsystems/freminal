// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 147.18: the saved cursor (DECSC) follows its text through a resize.
//!
//! The slot is kept in screen coordinates, which a reflow, a height change or
//! eviction invalidates. `set_size` pins the saved cell to its row number and
//! column, carries it through the same remap the live cursor uses, and reads
//! it back as screen coordinates (Ghostty `Screen.resize` `saved_cursor_pin`;
//! xterm `AdjustSavedCursor` for the alternate-screen case).

use freminal_common::buffer_states::tchar::TChar;

use crate::{buffer::Buffer, cell::Cell};

fn text(s: &str) -> Vec<TChar> {
    s.chars().map(|c| TChar::Ascii(c as u8)).collect()
}

/// Write `s` on the cursor row and move to the start of the next row.
fn line(buf: &mut Buffer, s: &str) {
    buf.insert_text(&text(s));
    buf.handle_lf();
    buf.handle_cr();
}

fn row_text(buf: &Buffer, index: usize) -> String {
    let text: String = buf.rows()[index]
        .cells()
        .iter()
        .map(Cell::into_utf8)
        .collect();
    text.trim_end().to_owned()
}

/// The character in the cell under the cursor.
fn char_under_cursor(buf: &Buffer) -> String {
    let cursor = buf.cursor();
    buf.rows()[cursor.pos.y].cells()[cursor.pos.x].into_utf8()
}

/// 30 characters, so a 20-column screen soft-wraps them over two rows.
const WRAPPED: &str = "0123456789ABCDEFGHIJ0123456789";

#[test]
fn primary_save_then_narrowing_reflow_restores_onto_the_same_character() {
    let mut buf = Buffer::new(20, 6).with_scrollback_limit(50);
    buf.insert_text(&text(WRAPPED));
    // 'F' sits at column 15 of the first row.
    buf.set_cursor_pos(Some(15), Some(0));
    assert_eq!(char_under_cursor(&buf), "F");
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(5));

    let _ = buf.set_size(10, 6, 0);
    buf.restore_cursor();

    assert_eq!(char_under_cursor(&buf), "F");
    assert_eq!(buf.cursor().pos.x, 5, "column 5 of the 'ABCDEFGHIJ' row");
}

#[test]
fn primary_save_then_widening_reflow_restores_onto_the_same_character() {
    let mut buf = Buffer::new(20, 6).with_scrollback_limit(50);
    buf.insert_text(&text(WRAPPED));
    // '5' on the second (continuation) row.
    buf.set_cursor_pos(Some(5), Some(1));
    assert_eq!(char_under_cursor(&buf), "5");
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(5));

    let _ = buf.set_size(40, 6, 0);
    buf.restore_cursor();

    assert_eq!(char_under_cursor(&buf), "5");
    assert_eq!(buf.cursor().pos.x, 25, "now column 25 of the joined row");
}

#[test]
fn parked_primary_save_follows_a_reflow_done_while_the_alternate_screen_is_up() {
    let mut buf = Buffer::new(20, 6).with_scrollback_limit(50);
    buf.insert_text(&text(WRAPPED));
    buf.set_cursor_pos(Some(15), Some(0));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(5));

    buf.switch_to_alternate_cleared();
    let _ = buf.set_size(10, 6, 0);
    buf.switch_to_primary();
    buf.restore_cursor();

    assert_eq!(char_under_cursor(&buf), "F");
}

#[test]
fn saved_cursor_follows_its_text_when_alt_screen_resize_pulls_rows_from_scrollback() {
    // `?1049h`, a resize that grows the height and pulls rows back from
    // scrollback, `?1049l` (the xterm `AdjustSavedCursor` case).
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    for i in 0..10 {
        line(&mut buf, &format!("row {i}"));
    }
    // The window shows rows 7..=9 and the blank row below them; screen row 1
    // is "row 8".
    buf.set_cursor_pos(Some(2), Some(1));
    assert_eq!(row_text(&buf, buf.cursor().pos.y), "row 8");
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(0));

    buf.switch_to_alternate_cleared();
    let _ = buf.set_size(20, 8, 0);
    buf.switch_to_primary();
    buf.restore_cursor();

    assert_eq!(row_text(&buf, buf.cursor().pos.y), "row 8");
    assert_eq!(buf.cursor().pos.x, 2);
    assert!(
        buf.cursor_screen_pos().y > 1,
        "the row moved down the screen as scrollback was pulled back above it"
    );
}

#[test]
fn live_save_follows_its_text_when_a_height_grow_pulls_rows_from_scrollback() {
    let mut buf = Buffer::new(20, 4).with_scrollback_limit(50);
    for i in 0..10 {
        line(&mut buf, &format!("row {i}"));
    }
    buf.set_cursor_pos(Some(2), Some(1));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(0));

    let _ = buf.set_size(20, 8, 0);
    buf.restore_cursor();

    assert_eq!(row_text(&buf, buf.cursor().pos.y), "row 8");
}

#[test]
fn saved_cell_pushed_into_scrollback_by_a_height_shrink_goes_to_the_top_left() {
    let mut buf = Buffer::new(20, 6).with_scrollback_limit(50);
    for i in 0..10 {
        line(&mut buf, &format!("row {i}"));
    }
    // Screen row 0, column 7: the first row to scroll off when the window
    // shrinks from the top.
    buf.set_cursor_pos(Some(7), Some(0));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(5));

    let _ = buf.set_size(20, 3, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (0, 0));
}

#[test]
fn saved_cell_evicted_from_the_alternate_screen_by_a_height_shrink_goes_to_the_top_left() {
    let mut buf = Buffer::new(20, 6);
    buf.enter_fresh_alternate();
    buf.set_cursor_pos(Some(4), Some(0));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(5));

    // The alternate screen keeps the bottom rows; screen row 0 is dropped.
    let _ = buf.set_size(20, 3, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (0, 0));
}

#[test]
fn saved_cell_on_the_alternate_screen_tracks_the_rows_a_shrink_keeps() {
    let mut buf = Buffer::new(20, 6);
    buf.enter_fresh_alternate();
    buf.set_cursor_pos(Some(4), Some(5));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(0));

    let _ = buf.set_size(20, 3, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!(
        (restored.x, restored.y),
        (4, 2),
        "the bottom row stays last"
    );
}

#[test]
fn pending_wrap_across_a_widening_resize_becomes_the_next_column() {
    let mut buf = Buffer::new(10, 4);
    buf.insert_text(&text("0123456789"));
    assert_eq!(buf.cursor().pos.x, 10, "setup: pending wrap");
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(3));

    let _ = buf.set_size(20, 4, 0);
    buf.restore_cursor();

    assert_eq!(buf.cursor().pos.x, 10, "the column after the last written");
    buf.insert_text(&text("A"));
    assert_eq!(
        row_text(&buf, 0),
        "0123456789A",
        "the next character continues the row instead of wrapping"
    );
}

#[test]
fn pending_wrap_across_a_narrowing_resize_survives_when_still_in_the_last_column() {
    let mut buf = Buffer::new(10, 4);
    buf.insert_text(&text("0123456789"));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(3));

    // '9' reflows to the last column of the second row.
    let _ = buf.set_size(5, 4, 0);
    buf.restore_cursor();

    assert_eq!(buf.cursor().pos.x, 5, "still pending wrap");
    buf.insert_text(&text("A"));
    let y = buf.cursor().pos.y;
    assert_eq!(row_text(&buf, y), "A", "the next character wraps");
}

#[test]
fn pending_wrap_on_the_alternate_screen_becomes_the_next_column_when_widened() {
    let mut buf = Buffer::new(10, 4);
    buf.enter_fresh_alternate();
    buf.insert_text(&text("0123456789"));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(3));

    let _ = buf.set_size(20, 4, 0);
    buf.restore_cursor();

    assert_eq!(buf.cursor().pos.x, 10);
}

#[test]
fn cell_below_every_stored_row_keeps_its_screen_position_through_a_reflow() {
    let mut buf = Buffer::new(20, 5);
    buf.set_cursor_pos(Some(0), Some(4));
    buf.save_cursor();
    // The grow reclaims the blank rows under the cursor, including the saved
    // one, so the next resize finds the saved cell below every stored row.
    buf.set_cursor_pos(Some(0), Some(2));
    let _ = buf.set_size(20, 8, 0);
    assert!(buf.rows().len() < 5, "setup: the padding was reclaimed");

    let _ = buf.set_size(10, 8, 0);
    buf.restore_cursor();

    assert_eq!(buf.cursor_screen_pos().y, 4);
}

#[test]
fn height_only_resize_without_scrollback_keeps_the_screen_position() {
    let mut buf = Buffer::new(20, 5);
    buf.set_cursor_pos(Some(6), Some(3));
    buf.save_cursor();
    buf.set_cursor_pos(Some(0), Some(0));

    let _ = buf.set_size(20, 9, 0);
    buf.restore_cursor();

    let restored = buf.cursor_screen_pos();
    assert_eq!((restored.x, restored.y), (6, 3));
}
