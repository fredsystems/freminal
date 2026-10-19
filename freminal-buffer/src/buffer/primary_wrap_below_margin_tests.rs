// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 148.C3: on the primary screen, LF / IND / NEL and auto-wrap below the
//! bottom margin on the last screen row must not scroll.
//!
//! With a partial DECSTBM region, the rows under it are outside the region.
//! xterm's `xtermIndex` (cursor.c) treats "indexing when below scrolling
//! region" as cursor down: if `cur_row > bot_marg` it calls `CursorDown`,
//! never `xtermScroll`, and `CursorDown` clamps at `max_row` when below the
//! margin (`max = (cur_row > bot_marg ? max_row : bot_marg)`). `WrapLine`
//! (charproc.c) goes through `xtermIndex` too, so a wrap behaves the same.
//!
//! freminal's primary screen instead pushed a new row, scrolling the whole
//! screen (region included) into scrollback. The alternate screen was fixed
//! in 148.7 (see `alt_wrap_below_margin_tests`).

use freminal_common::buffer_states::{buffer_type::BufferType, tchar::TChar};

use crate::{buffer::Buffer, cell::Cell};

fn text(s: &str) -> Vec<TChar> {
    s.chars().map(|c| TChar::Ascii(c as u8)).collect()
}

fn row_text(buf: &Buffer, index: usize) -> String {
    buf.rows()[index]
        .cells()
        .iter()
        .map(Cell::into_utf8)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// A primary 10x5 buffer holding `total_lines` lines (`L0`, `L1`, ...) with
/// the cursor on the last line, so `total_lines - 5` rows are in scrollback
/// when `total_lines > 5`. No scroll region is set yet.
fn filled_primary(total_lines: usize) -> Buffer {
    let mut buf = Buffer::new(10, 5);
    for i in 0..total_lines {
        if i > 0 {
            buf.handle_lf();
            buf.handle_cr();
        }
        buf.insert_text(&text(&format!("L{i}")));
    }
    assert_eq!(buf.kind(), BufferType::Primary);
    assert_eq!(buf.rows().len(), total_lines.max(5));
    buf
}

/// Primary 10x5 with scrollback, region rows 1..=3 (1-based, 0-based 0..=2)
/// and the cursor on the last screen row (screen row 4).
fn region_above_last_row() -> Buffer {
    let mut buf = filled_primary(8);
    buf.set_scroll_region(1, 3);
    assert_eq!(buf.scroll_region(), (0, 2));
    buf.set_cursor_pos(Some(0), Some(4));
    buf
}

fn assert_no_scroll(buf: &Buffer, rows_before: usize, y_before: usize) {
    assert_eq!(buf.rows().len(), rows_before, "no row may be pushed");
    assert_eq!(buf.cursor().pos.y, y_before, "the cursor stays on the row");
    buf.debug_assert_invariants();
}

#[test]
fn lf_below_the_margin_on_the_last_row_does_not_scroll() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);
    let last = row_text(&buf, y);

    buf.handle_lf();

    assert_no_scroll(&buf, rows, y);
    assert_eq!(row_text(&buf, y), last, "the row is untouched");
}

#[test]
fn repeated_lf_below_the_margin_never_scrolls() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);

    for _ in 0..3 {
        buf.handle_lf();
    }

    assert_no_scroll(&buf, rows, y);
}

#[test]
fn ind_below_the_margin_on_the_last_row_does_not_scroll() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);
    buf.set_cursor_pos(Some(3), Some(4));

    buf.handle_ind();

    assert_no_scroll(&buf, rows, y);
    assert_eq!(buf.cursor().pos.x, 3, "IND does not move the column");
}

#[test]
fn nel_below_the_margin_on_the_last_row_does_not_scroll_but_returns_to_col_0() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);
    buf.set_cursor_pos(Some(3), Some(4));

    buf.handle_nel();

    assert_no_scroll(&buf, rows, y);
    assert_eq!(buf.cursor().pos.x, 0, "NEL carries a CR");
}

#[test]
fn wrap_on_the_last_row_below_the_margin_overwrites_the_row() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);

    buf.insert_text(&text("abcdefghijKLM"));

    assert_no_scroll(&buf, rows, y);
    assert_eq!(
        row_text(&buf, y),
        "KLMdefghij",
        "the wrapped text overwrites the last row from the wrap column"
    );
}

#[test]
fn repeated_wraps_on_the_last_row_never_scroll() {
    let mut buf = region_above_last_row();
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);

    buf.insert_text(&text(&"x".repeat(100)));

    assert_no_scroll(&buf, rows, y);
}

#[test]
fn lf_below_the_margin_above_the_last_row_moves_down_without_scrolling() {
    let mut buf = filled_primary(8);
    buf.set_scroll_region(1, 2);
    assert_eq!(buf.scroll_region(), (0, 1));
    // Screen row 3: below the margin (bottom = 1) but not the last row (4).
    buf.set_cursor_pos(Some(0), Some(3));
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);

    buf.handle_lf();

    assert_eq!(buf.rows().len(), rows, "no row may be pushed");
    assert_eq!(buf.cursor().pos.y, y + 1, "the cursor moves down one row");
    buf.debug_assert_invariants();
}

#[test]
fn wrap_below_the_margin_above_the_last_row_still_advances() {
    let mut buf = filled_primary(8);
    buf.set_scroll_region(1, 2);
    buf.set_cursor_pos(Some(0), Some(3));
    let (rows, y) = (buf.rows().len(), buf.cursor().pos.y);

    buf.insert_text(&text("abcdefghijKLM"));

    assert_eq!(buf.rows().len(), rows);
    assert_eq!(buf.cursor().pos.y, y + 1);
    assert_eq!(row_text(&buf, y), "abcdefghij");
    assert_eq!(row_text(&buf, y + 1), "KLM");
    buf.debug_assert_invariants();
}

#[test]
fn lf_at_the_region_bottom_still_scrolls_the_region() {
    let mut buf = filled_primary(8);
    buf.set_scroll_region(1, 3);
    buf.set_cursor_pos(Some(0), Some(2));
    let rows = buf.rows().len();
    let y = buf.cursor().pos.y;

    buf.handle_lf();

    assert_eq!(buf.rows().len(), rows, "a region scroll pushes no row");
    assert_eq!(
        buf.cursor().pos.y,
        y,
        "the cursor stays on the bottom margin"
    );
    buf.debug_assert_invariants();
}

#[test]
fn full_screen_region_still_scrolls_into_scrollback() {
    // Pin: with a full-screen region the cursor can never be below the
    // margin, so LF on the last row pushes a row as before.
    let mut buf = filled_primary(8);
    let rows = buf.rows().len();
    let y = buf.cursor().pos.y;

    buf.handle_lf();

    assert_eq!(
        buf.rows().len(),
        rows + 1,
        "a row is pushed into scrollback"
    );
    assert_eq!(buf.cursor().pos.y, y + 1);
    buf.debug_assert_invariants();
}

#[test]
fn full_screen_region_wrap_on_the_last_row_still_scrolls_into_scrollback() {
    let mut buf = filled_primary(8);
    let rows = buf.rows().len();
    buf.set_cursor_pos(Some(0), Some(4));

    buf.insert_text(&text("abcdefghijKLM"));

    assert_eq!(buf.rows().len(), rows + 1);
    buf.debug_assert_invariants();
}
