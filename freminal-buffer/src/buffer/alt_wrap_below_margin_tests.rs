// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 148.7 (issue #568): an auto-wrap on the alternate screen must never
//! change its row count.
//!
//! The alternate screen is a fixed `height`-row grid. When the cursor sits
//! below the bottom margin (DECSTBM) on the last screen row, there is no row
//! to wrap onto and nothing to scroll, but the wrap path appended one anyway.
//! The extra row broke the "alternate buffer has exactly `height` rows"
//! invariant, left `cursor.y` one past the grid, and was then parked with the
//! screen on `?1049l` (the "parked alternate store" invariant) and met by
//! `clear_alternate_screen` on the next `?1049h` (the `cursor.y < rows.len()`
//! invariant): the four failures the issue lists share this producer.
//!
//! xterm wraps with `WrapLine` (charproc.c) -> `xtermIndex` (cursor.c), which
//! for a cursor below `bot_marg` is `CursorDown`; that clamps to `max_row`, so
//! the cursor stays on the last row and the wrapped text overwrites it from
//! the left margin.

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

/// A 4x4 alternate screen with the scroll region on rows 1-2 (1-based), so
/// the last screen row (index 3) lies below the bottom margin.
fn alternate_with_region_above_last_row() -> Buffer {
    let mut buf = Buffer::new(4, 4);
    buf.switch_to_alternate();
    buf.set_scroll_region(1, 2);
    assert_eq!(buf.scroll_region(), (0, 1));
    buf
}

#[test]
fn wrap_on_the_last_row_below_the_margin_keeps_height_rows() {
    let mut buf = alternate_with_region_above_last_row();
    buf.set_cursor_pos(Some(0), Some(3));

    buf.insert_text(&text("abcdef"));

    assert_eq!(buf.kind(), BufferType::Alternate);
    assert_eq!(
        buf.rows().len(),
        4,
        "the alternate screen keeps `height` rows"
    );
    assert_eq!(buf.cursor().pos.y, 3, "the cursor stays on the last row");
    assert_eq!(
        row_text(&buf, 3),
        "efcd",
        "the wrapped text overwrites the last row from the left margin"
    );
    buf.debug_assert_invariants();
}

#[test]
fn leaving_the_alternate_screen_after_such_a_wrap_parks_height_rows() {
    // The parked-alternate invariant is checked in `switch_to_primary`.
    let mut buf = alternate_with_region_above_last_row();
    buf.set_cursor_pos(Some(0), Some(3));
    buf.insert_text(&text("abcdef"));

    buf.switch_to_primary();

    assert_eq!(buf.kind(), BufferType::Primary);
    buf.debug_assert_invariants();
}

#[test]
fn clearing_the_alternate_screen_after_such_a_wrap_leaves_the_cursor_on_it() {
    // `clear_alternate_screen` relies on the store holding exactly `height`
    // rows so that the cursor row stays valid.
    let mut buf = alternate_with_region_above_last_row();
    buf.set_cursor_pos(Some(0), Some(3));
    buf.insert_text(&text("abcdef"));

    buf.clear_alternate_screen();

    assert_eq!(buf.rows().len(), 4);
    assert!(buf.cursor().pos.y < buf.rows().len());
    buf.debug_assert_invariants();
}

#[test]
fn repeated_wraps_on_the_last_row_never_grow_the_screen() {
    let mut buf = alternate_with_region_above_last_row();
    buf.set_cursor_pos(Some(0), Some(3));

    buf.insert_text(&text(&"x".repeat(100)));

    assert_eq!(buf.rows().len(), 4);
    assert_eq!(buf.cursor().pos.y, 3);
    buf.debug_assert_invariants();
}

#[test]
fn wrap_below_the_margin_above_the_last_row_still_advances() {
    // The fix applies only to the last screen row: a wrap below the margin
    // with a row left underneath moves down onto it, as before.
    let mut buf = Buffer::new(4, 6);
    buf.switch_to_alternate();
    buf.set_scroll_region(1, 2);
    buf.set_cursor_pos(Some(0), Some(3));

    buf.insert_text(&text("abcdef"));

    assert_eq!(buf.rows().len(), 6);
    assert_eq!(buf.cursor().pos.y, 4);
    assert_eq!(row_text(&buf, 3), "abcd");
    assert_eq!(row_text(&buf, 4), "ef");
    buf.debug_assert_invariants();
}

#[test]
fn wrap_at_the_region_bottom_still_scrolls_the_region() {
    // At the bottom margin the region scrolls; that path is untouched.
    let mut buf = alternate_with_region_above_last_row();
    buf.set_cursor_pos(Some(0), Some(0));
    buf.insert_text(&text("1111"));
    buf.set_cursor_pos(Some(0), Some(1));

    buf.insert_text(&text("abcdef"));

    assert_eq!(buf.rows().len(), 4);
    assert_eq!(buf.cursor().pos.y, 1);
    assert_eq!(row_text(&buf, 0), "abcd");
    assert_eq!(row_text(&buf, 1), "ef");
    buf.debug_assert_invariants();
}
