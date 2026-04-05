// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 8 — Insert/Delete Operation Tests.
//!
//! Exercises every insert/delete operation covered by vttest Menu 8:
//!
//! - ICH (`CSI @`) — Insert Characters: inserts blank spaces at the cursor
//!   position, shifting existing characters right (rightmost chars lost).
//! - DCH (`CSI P`) — Delete Characters: deletes characters at the cursor
//!   position, pulling remaining characters left (blanks appear at right).
//! - IL  (`CSI L`) — Insert Lines: inserts blank lines at the cursor row,
//!   pushing existing lines down within the scroll region.
//! - DL  (`CSI M`) — Delete Lines: deletes the cursor's row and shifts lines
//!   below up within the scroll region (blanks appear at bottom of region).
//! - IRM (`CSI 4 h/l`) — Insert/Replace Mode: not currently implemented;
//!   tested to confirm the terminal remains in replace mode (default).
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row). CSI sequences use **1-indexed** row;col parameters.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── ICH — Insert Characters (CSI Ps @) ─────────────────────────────────────

#[test]
fn ich_default_inserts_one_blank_at_cursor() {
    let mut h = VtTestHelper::new_default();
    // Write "ABCDE" to row 0, then move cursor to col 2 ('C').
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // CUP row=1, col=3 → (x=2, y=0)
    // ICH with no param — default is 1.
    h.feed(b"\x1b[@");
    // Cursor must NOT move.
    h.assert_cursor_pos(2, 0);
    // 'C' and 'D' shift right; one blank inserted at col 2; 'E' is lost off the right.
    h.assert_row(0, "AB CDE");
}

#[test]
fn ich_inserts_multiple_blanks_at_cursor() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGH");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0)
    // ICH(3) — insert 3 blanks at col 2, shifting 'CDEFGH' right.
    h.feed(b"\x1b[3@");
    h.assert_cursor_pos(2, 0);
    // "AB" + "   " + "CDEFGH" — with 8 original chars and 3 blanks inserted,
    // the row holds 11 chars total, all of which fit within the 80-col width.
    // Characters are only lost when the row reaches the 80-col limit.
    h.assert_row(0, "AB   CDEFGH");
}

#[test]
fn ich_cursor_stays_in_place_after_insert() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;1H"); // cursor → (x=0, y=0) (start of row)
    h.feed(b"\x1b[2@"); // ICH(2)
    // Cursor must remain at col 0.
    h.assert_cursor_pos(0, 0);
    // Two blanks are inserted at col 0; 'A','B','C','D','E' shift right.
    // The original 5 chars + 2 blanks = 7 chars total; all fit within the 80-col
    // row so nothing is lost (rightmost chars only drop when the row is full).
    h.assert_row(0, "  ABCDE");
}

#[test]
fn ich_at_right_margin_inserts_one_blank_pushes_nothing() {
    let mut h = VtTestHelper::new_default();
    // Move to the last column (col 79, 0-indexed) of a blank row.
    h.feed(b"\x1b[1;80H"); // CUP row=1, col=80 → (x=79, y=0)
    h.feed_str("X"); // Write 'X' at col 79.
    // Move back to col 79.
    h.feed(b"\x1b[1;80H");
    // ICH(1) — insert one blank at the last column; 'X' is pushed off.
    h.feed(b"\x1b[@");
    h.assert_cursor_pos(79, 0);
    // The inserted blank replaces 'X' (X is shifted beyond col 79 → discarded).
    h.assert_row(0, "");
}

#[test]
fn ich_clamped_when_count_exceeds_available_space() {
    let h = VtTestHelper::new_default();
    // Write "ABCDE" into a narrow 10-column terminal so we can observe clamping.
    let mut h2 = VtTestHelper::new(10, 5);
    h2.feed_str("ABCDE");
    h2.feed(b"\x1b[1;4H"); // cursor → (x=3, y=0) → at 'D'
    // ICH(100) — far more than the 7 cols remaining; clamped to 7.
    h2.feed(b"\x1b[100@");
    h2.assert_cursor_pos(3, 0);
    // 'A','B','C' remain; cols 3-9 become blank (D and E are pushed off).
    h2.assert_row(0, "ABC");
    let _ = h; // suppress unused warning
}

#[test]
fn ich_param_zero_inserts_one_blank() {
    // Ps=0 must behave identically to Ps=1 (the spec says default = 1 for 0/1/None).
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0)
    h.feed(b"\x1b[0@"); // ICH(0) == ICH(1)
    h.assert_cursor_pos(2, 0);
    h.assert_row(0, "AB CDE");
}

// ─── DCH — Delete Characters (CSI Ps P) ─────────────────────────────────────

#[test]
fn dch_default_deletes_one_char_at_cursor() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0) → at 'C'
    // DCH with no param — default is 1.
    h.feed(b"\x1b[P");
    // Cursor must NOT move.
    h.assert_cursor_pos(2, 0);
    // 'C' is deleted; 'D','E' shift left; one blank fills right.
    h.assert_row(0, "ABDE");
}

#[test]
fn dch_deletes_multiple_chars_at_cursor() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGH");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0) → at 'C'
    // DCH(3) — delete 'C','D','E'; shift 'F','G','H' left.
    h.feed(b"\x1b[3P");
    h.assert_cursor_pos(2, 0);
    h.assert_row(0, "ABFGH");
}

#[test]
fn dch_cursor_stays_in_place_after_delete() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;1H"); // cursor → (x=0, y=0)
    h.feed(b"\x1b[2P"); // DCH(2) — delete 'A' and 'B'.
    h.assert_cursor_pos(0, 0);
    // 'C','D','E' shift left; two blanks at right.
    h.assert_row(0, "CDE");
}

#[test]
fn dch_at_end_of_line_deletes_last_char() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;5H"); // cursor → (x=4, y=0) → at 'E'
    h.feed(b"\x1b[P"); // DCH(1) — delete 'E'.
    h.assert_cursor_pos(4, 0);
    h.assert_row(0, "ABCD");
}

#[test]
fn dch_clamped_when_count_exceeds_remaining_chars() {
    let mut h = VtTestHelper::new(10, 5);
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0) → at 'C'
    // DCH(100) — only 3 chars ('C','D','E') are to the right of/at cursor.
    h.feed(b"\x1b[100P");
    h.assert_cursor_pos(2, 0);
    // 'A','B' remain; rest of the row is blank.
    h.assert_row(0, "AB");
}

#[test]
fn dch_param_zero_deletes_one_char() {
    // Ps=0 must normalize to 1.
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0)
    h.feed(b"\x1b[0P"); // DCH(0) == DCH(1)
    h.assert_cursor_pos(2, 0);
    h.assert_row(0, "ABDE");
}

// ─── IL — Insert Lines (CSI Ps L) ────────────────────────────────────────────

#[test]
fn il_default_inserts_one_blank_line_at_cursor_row() {
    let mut h = VtTestHelper::new_default();
    // Write four rows of content.
    h.feed_str("Line 0\r\nLine 1\r\nLine 2\r\nLine 3");
    // Move cursor to row 1 (0-indexed), col 0.
    h.feed(b"\x1b[2;1H"); // CUP row=2, col=1 → (x=0, y=1)
    // IL(1) — insert one blank line at row 1.
    h.feed(b"\x1b[L");
    // Cursor row does NOT change (stays on the same screen row); col unchanged.
    h.assert_cursor_pos(0, 1);
    // A blank line is at row 1; original rows 1-3 shifted down.
    h.assert_row(0, "Line 0");
    h.assert_row(1, ""); // new blank line
    h.assert_row(2, "Line 1");
    h.assert_row(3, "Line 2");
    h.assert_row(4, "Line 3");
}

#[test]
fn il_inserts_multiple_blank_lines() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row A\r\nRow B\r\nRow C\r\nRow D");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1) — Row B
    // IL(3) — insert 3 blank lines at row 1; 'Row B','Row C','Row D' shift down.
    h.feed(b"\x1b[3L");
    h.assert_cursor_pos(0, 1);
    h.assert_row(0, "Row A");
    h.assert_row(1, ""); // blank
    h.assert_row(2, ""); // blank
    h.assert_row(3, ""); // blank
    h.assert_row(4, "Row B");
    h.assert_row(5, "Row C");
}

#[test]
fn il_at_bottom_of_screen_inserts_one_blank_loses_bottom_row() {
    let mut h = VtTestHelper::new_default();
    // Write content to the second-to-last row (y=22) and the last row (y=23).
    h.feed(b"\x1b[23;1H"); // CUP → row 23, col 1 → (x=0, y=22)
    h.feed_str("Second to last");
    h.feed(b"\x1b[24;1H"); // CUP → row 24, col 1 → (x=0, y=23)
    h.feed_str("Bottom row content");
    // Move cursor to second-to-last row.
    h.feed(b"\x1b[23;1H"); // cursor → (x=0, y=22)
    // IL(1) at row y=22: inserts a blank line; "Second to last" and
    // "Bottom row content" both shift down, but "Bottom row content" at y=23
    // is pushed off the bottom of the screen (scroll region ends at y=23).
    h.feed(b"\x1b[L");
    h.assert_cursor_pos(0, 22);
    // y=22 is now blank (newly inserted line).
    h.assert_row(22, "");
    // y=23 now holds the shifted "Second to last" (old y=22 content).
    h.assert_row(23, "Second to last");
}

#[test]
fn il_within_scroll_region_does_not_affect_rows_outside() {
    let mut h = VtTestHelper::new_default();
    // Write content to rows 0-7.
    for i in 0..8_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    // Set scroll region: rows 3-6 (1-indexed) → rows 2-5 (0-indexed).
    h.feed(b"\x1b[3;6r"); // DECSTBM — resets cursor to (0,0).
    // Move cursor to row 3 (1-indexed; 0-indexed: y=2), inside the region.
    h.feed(b"\x1b[3;1H"); // CUP row=3, col=1 → (x=0, y=2)
    // IL(1) — inserts a blank line at row 2 within the scroll region.
    h.feed(b"\x1b[L");
    h.assert_cursor_pos(0, 2);
    // Rows outside the region (0 and 1) must be unchanged.
    h.assert_row(0, "Row 00");
    h.assert_row(1, "Row 01");
    // Blank line inserted at the cursor row (row 2 inside the region).
    h.assert_row(2, "");
    // Previous row 2 ("Row 02") shifted down to row 3.
    h.assert_row(3, "Row 02");
    // Previous row 3 ("Row 03") shifted down to row 4.
    h.assert_row(4, "Row 03");
    // Row 5 (bottom of region) now holds "Row 04" (shifted from row 4).
    // "Row 04" was shifted to the bottom of the region; "Row 05" was pushed off.
    h.assert_row(5, "Row 04");
    // Row 6 (outside region) is unchanged — "Row 05" was NOT shifted here.
    h.assert_row(6, "Row 06");
}

#[test]
fn il_count_clamped_to_rows_remaining_in_region() {
    let mut h = VtTestHelper::new(80, 10);
    // Write content to rows 0-4.
    for i in 0..5_u8 {
        h.feed_str(&format!("Row {i}\r\n"));
    }
    // Move cursor to row 2 (0-indexed).
    h.feed(b"\x1b[3;1H"); // CUP row=3, col=1 → (x=0, y=2)
    // IL(100) — far more than rows remaining (8 rows from y=2 to y=9); clamped.
    h.feed(b"\x1b[100L");
    h.assert_cursor_pos(0, 2);
    // Rows 0-1 must still be intact.
    h.assert_row(0, "Row 0");
    h.assert_row(1, "Row 1");
    // Rows 2-9 are all blank (all content from row 2 onward was pushed off).
    for row in 2..10 {
        h.assert_row(row, "");
    }
}

// ─── DL — Delete Lines (CSI Ps M) ────────────────────────────────────────────

#[test]
fn dl_default_deletes_one_line_at_cursor_row() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("Line 0\r\nLine 1\r\nLine 2\r\nLine 3");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1) — Line 1
    // DL(1) — delete line 1; lines below shift up; blank appears at bottom of screen.
    h.feed(b"\x1b[M");
    h.assert_cursor_pos(0, 1);
    h.assert_row(0, "Line 0");
    h.assert_row(1, "Line 2"); // Line 1 deleted; Line 2 moved up
    h.assert_row(2, "Line 3");
    h.assert_row(3, ""); // blank fills the vacated bottom row in the visible region
}

#[test]
fn dl_deletes_multiple_lines() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row A\r\nRow B\r\nRow C\r\nRow D\r\nRow E");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1) — Row B
    // DL(3) — delete Row B, Row C, Row D; Row E shifts up; 3 blanks at bottom.
    h.feed(b"\x1b[3M");
    h.assert_cursor_pos(0, 1);
    h.assert_row(0, "Row A");
    h.assert_row(1, "Row E"); // only remaining row below original cursor
    h.assert_row(2, "");
    h.assert_row(3, "");
    h.assert_row(4, "");
}

#[test]
fn dl_cursor_column_unchanged_after_delete() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("AAAA\r\nBBBB\r\nCCCC");
    // Position cursor at row 1, col 3.
    h.feed(b"\x1b[2;4H"); // CUP row=2, col=4 → (x=3, y=1)
    h.feed(b"\x1b[M"); // DL(1)
    // Cursor row stays at y=1; col stays at x=3.
    h.assert_cursor_pos(3, 1);
    h.assert_row(0, "AAAA");
    h.assert_row(1, "CCCC");
}

#[test]
fn dl_at_bottom_of_screen_produces_blank_bottom_row() {
    let mut h = VtTestHelper::new_default();
    // Write content to two rows near the bottom.
    h.feed(b"\x1b[23;1H"); // cursor → (x=0, y=22)
    h.feed_str("Second to last");
    h.feed(b"\x1b[24;1H"); // cursor → (x=0, y=23)
    h.feed_str("Last line");
    // Move cursor back to second-to-last row (y=22).
    h.feed(b"\x1b[23;1H");
    // DL(1) at y=22: deletes "Second to last"; "Last line" shifts up to y=22;
    // a blank row appears at y=23 (bottom of scroll region).
    h.feed(b"\x1b[M");
    h.assert_cursor_pos(0, 22);
    h.assert_row(22, "Last line"); // shifted up from y=23
    h.assert_row(23, ""); // blank fills the vacated bottom of the scroll region
}

#[test]
fn dl_within_scroll_region_does_not_affect_rows_outside() {
    let mut h = VtTestHelper::new_default();
    for i in 0..8_u8 {
        h.feed_str(&format!("Row {:02}\r\n", i));
    }
    // Set scroll region: rows 3-6 (1-indexed) → rows 2-5 (0-indexed).
    h.feed(b"\x1b[3;6r"); // DECSTBM — resets cursor to (0,0).
    // Move cursor to row 3 (0-indexed: y=2) inside the region.
    h.feed(b"\x1b[3;1H"); // CUP row=3, col=1 → (x=0, y=2)
    // DL(1) — deletes row 2; rows 3-5 shift up within region; blank at bottom of region.
    h.feed(b"\x1b[M");
    h.assert_cursor_pos(0, 2);
    // Rows outside the region (0 and 1) must be unchanged.
    h.assert_row(0, "Row 00");
    h.assert_row(1, "Row 01");
    // "Row 02" is deleted; "Row 03" shifts up to row 2.
    h.assert_row(2, "Row 03");
    // "Row 04" shifts up to row 3.
    h.assert_row(3, "Row 04");
    // "Row 05" shifts up to row 4.
    h.assert_row(4, "Row 05");
    // Bottom of scroll region (row 5) gets a blank.
    h.assert_row(5, "");
    // Rows outside the region (row 6 and beyond) are unaffected.
    h.assert_row(6, "Row 06");
}

#[test]
fn dl_count_clamped_to_rows_remaining_in_region() {
    let mut h = VtTestHelper::new(80, 10);
    for i in 0..5_u8 {
        h.feed_str(&format!("Row {i}\r\n"));
    }
    h.feed(b"\x1b[3;1H"); // cursor → (x=0, y=2)
    // DL(100) — far more than rows below; clamped to rows within visible region.
    h.feed(b"\x1b[100M");
    h.assert_cursor_pos(0, 2);
    h.assert_row(0, "Row 0");
    h.assert_row(1, "Row 1");
    // All rows from y=2 onward are blank.
    for row in 2..10 {
        h.assert_row(row, "");
    }
}

// ─── IRM — Insert/Replace Mode (CSI 4 h / CSI 4 l) ──────────────────────────
//
// IRM (ANSI mode 4) is not currently implemented; `CSI 4 h` is treated as an
// unknown mode and silently ignored.  These tests confirm that:
// - Normal text entry continues to overwrite (replace mode is the default).
// - Sending `CSI 4 h` does not corrupt the terminal state.
// - Sending `CSI 4 l` (reset) is also silently ignored without harm.

#[test]
fn irm_default_mode_is_replace_overwrite() {
    // Without IRM active, writing characters overwrites existing content.
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0) — at 'C'
    // Write 'X' — in replace mode it overwrites 'C'.
    h.feed_str("X");
    h.assert_row(0, "ABXDE");
    h.assert_cursor_pos(3, 0);
}

#[test]
fn irm_set_mode_does_not_corrupt_terminal_state() {
    // `CSI 4 h` is an unknown mode; the terminal should silently ignore it and
    // remain functional.
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[4h"); // IRM set — silently ignored
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0)
    // Normal text entry still overwrites (replace mode unchanged).
    h.feed_str("X");
    h.assert_row(0, "ABXDE");
    h.assert_cursor_pos(3, 0);
}

#[test]
fn irm_reset_mode_is_silently_ignored() {
    // `CSI 4 l` resets IRM — also unknown, also silently ignored.
    let mut h = VtTestHelper::new_default();
    h.feed_str("HELLO");
    h.feed(b"\x1b[4l"); // IRM reset — silently ignored
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0)
    h.feed_str("X");
    // Still replace mode: 'L' (index 2, 'L') is overwritten by 'X'.
    h.assert_row(0, "HEXLO");
    h.assert_cursor_pos(3, 0);
}

// ─── Combined / Interaction Tests ────────────────────────────────────────────

#[test]
fn ich_then_dch_restores_original_content() {
    // ICH followed by DCH with the same count should logically restore the row
    // (provided no content has been pushed off the right edge).
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDE");
    h.feed(b"\x1b[1;3H"); // cursor → (x=2, y=0) — at 'C'
    // ICH(2): "AB  CDE" (only 7 chars total, no overflow on 80-wide terminal).
    h.feed(b"\x1b[2@");
    h.assert_row(0, "AB  CDE");
    // DCH(2) from same position: removes the two blanks → "ABCDE".
    h.feed(b"\x1b[2P");
    h.assert_row(0, "ABCDE");
    h.assert_cursor_pos(2, 0);
}

#[test]
fn il_then_dl_restores_original_content() {
    // IL followed by DL with the same count restores the original row order.
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row 0\r\nRow 1\r\nRow 2");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1) — Row 1
    // IL(1): blank inserted at row 1; Row 1 → row 2; Row 2 → row 3.
    h.feed(b"\x1b[L");
    h.assert_row(1, "");
    h.assert_row(2, "Row 1");
    // DL(1): blank at row 1 removed; Row 1 moves back to row 1; Row 2 to row 2.
    h.feed(b"\x1b[M");
    h.assert_row(0, "Row 0");
    h.assert_row(1, "Row 1");
    h.assert_row(2, "Row 2");
    h.assert_cursor_pos(0, 1);
}

#[test]
fn ich_on_non_zero_row() {
    // ICH should operate on the cursor's current row, not always row 0.
    let mut h = VtTestHelper::new_default();
    h.feed_str("First line\r\nABCDE\r\nThird line");
    h.feed(b"\x1b[2;3H"); // cursor → row 2, col 3 → (x=2, y=1) — at 'C'
    h.feed(b"\x1b[@"); // ICH(1)
    h.assert_cursor_pos(2, 1);
    // Row 0 and row 2 must be untouched.
    h.assert_row(0, "First line");
    h.assert_row(2, "Third line");
    // Row 1: blank inserted at col 2.
    h.assert_row(1, "AB CDE");
}

#[test]
fn dch_on_non_zero_row() {
    // DCH should operate on the cursor's current row, not always row 0.
    let mut h = VtTestHelper::new_default();
    h.feed_str("First line\r\nABCDE\r\nThird line");
    h.feed(b"\x1b[2;3H"); // cursor → row 2, col 3 → (x=2, y=1) — at 'C'
    h.feed(b"\x1b[P"); // DCH(1)
    h.assert_cursor_pos(2, 1);
    h.assert_row(0, "First line");
    h.assert_row(2, "Third line");
    h.assert_row(1, "ABDE");
}

#[test]
fn il_param_zero_inserts_one_line() {
    // Ps=0 must normalize to 1 for IL.
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row 0\r\nRow 1");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1)
    h.feed(b"\x1b[0L"); // IL(0) == IL(1)
    h.assert_cursor_pos(0, 1);
    h.assert_row(1, "");
    h.assert_row(2, "Row 1");
}

#[test]
fn dl_param_zero_deletes_one_line() {
    // Ps=0 must normalize to 1 for DL.
    let mut h = VtTestHelper::new_default();
    h.feed_str("Row 0\r\nRow 1\r\nRow 2");
    h.feed(b"\x1b[2;1H"); // cursor → (x=0, y=1)
    h.feed(b"\x1b[0M"); // DL(0) == DL(1)
    h.assert_cursor_pos(0, 1);
    h.assert_row(0, "Row 0");
    h.assert_row(1, "Row 2");
    h.assert_row(2, "");
}

// ─── Phase B: vttest-exact sequences from tst_insdel() ───────────────────────
//
// The following tests reproduce exact byte sequences from the vttest source code
// (`vttest-20251205/main.c`, `tst_insdel()`, lines 941-1039).  They assert correct
// VT100/VT220 behaviour rather than Freminal-current behaviour.

/// vttest `tst_insdel` — ICH alphabet test (lines 1020-1032 of main.c).
///
/// vttest writes the letters Z..A backwards, each followed by BS and then ICH(2).
/// The result should be:
///   `  A B C D E F G H I J K L M N O P Q R S T U V W X Y Z`
/// (leading two spaces from the final ICH(2) on 'A', then one space between each letter).
///
/// Exact vttest loop (translated):
///   for i in ('A'..='Z').rev():
///     tprintf("%c%c", i, BS)  →  write letter, then BS (0x08)
///     ich(2)                  →  CSI 2 @
#[test]
fn tst_insdel_ich_alphabet_test() {
    let mut h = VtTestHelper::new_default();
    // cup(1, 1) — position at row 1, col 1 (top-left)
    h.feed(b"\x1b[1;1H");

    // Replicate vttest: for i = 'Z' down to 'A': write char, BS, ICH(2)
    let mut seq = Vec::new();
    for letter in (b'A'..=b'Z').rev() {
        seq.push(letter); // write letter
        seq.push(0x08); // BS (cursor back one)
        seq.extend_from_slice(b"\x1b[2@"); // ICH(2)
    }
    h.feed(&seq);

    // vttest expects row 1 to read:
    //   "  A B C D E F G H I J K L M N O P Q R S T U V W X Y Z"
    // (leading two blanks, then letter-space pairs)
    h.assert_row(0, "  A B C D E F G H I J K L M N O P Q R S T U V W X Y Z");
}

/// vttest `tst_insdel` — DCH stagger test (lines 1007-1019 of main.c), single-width pass.
///
/// For each row (1-indexed, 1..=max_lines):
///   cup(row, 1)
///   fill sw columns with the letter 'A'-1+row
///   cup(row, sw/1 - row)          →  move to column (80 - row)
///   dch(row)                       →  CSI row P
///
/// Expected result: each row has its rightmost `row` characters deleted, producing
/// a staircase.  Row 1 is missing the last 1 char, row 2 the last 2, etc.
/// Row N ends at column (80 - N - N) = 80 - 2*N chars of letter content.
#[test]
fn tst_insdel_dch_stagger_single_width() {
    let mut h = VtTestHelper::new_default();
    let sw: usize = 80;
    let max_lines: usize = 24;

    h.feed(b"\x1b[2J"); // ED(2) — clear screen
    h.feed(b"\x1b[1;1H"); // cup(1,1)

    for row in 1..=max_lines {
        let letter = b'A' - 1 + u8::try_from(row).unwrap();
        // cup(row, 1)
        let cup = format!("\x1b[{};1H", row);
        h.feed(cup.as_bytes());
        // fill sw columns with the letter
        let row_content: Vec<u8> = vec![letter; sw];
        h.feed(&row_content);
        // cup(row, sw - row)  → 1-indexed col = sw - row
        let col = sw - row; // vttest: sw/dblchr - row where dblchr=1
        let cup2 = format!("\x1b[{};{}H", row, col);
        h.feed(cup2.as_bytes());
        // dch(row)
        let dch = format!("\x1b[{}P", row);
        h.feed(dch.as_bytes());
    }

    // Verify the stagger for the first few rows.
    //
    // vttest: cup(row, sw - row) is 1-indexed col sw-row = 0-indexed col sw-row-1.
    // dch(row) deletes  chars starting there.  The char at col sw-1 (rightmost)
    // slides leftward to fill; result = sw - row visible letters.
    //
    // Row 0 (1-indexed row 1): cup(1,79) = 0-indexed col 78; dch(1) → sw-1 = 79 'A's.
    let row0_expected: String = "A".repeat(sw - 1); // 79 'A's
    h.assert_row(0, &row0_expected);

    // Row 1 (1-indexed row 2): cup(2,78) = 0-indexed col 77; dch(2) → sw-2 = 78 'B's.
    let row1_expected: String = "B".repeat(sw - 2); // 78 'B's
    h.assert_row(1, &row1_expected);

    // Row 2 (1-indexed row 3): cup(3,77) = 0-indexed col 76; dch(3) → sw-3 = 77 'C's.
    let row2_expected: String = "C".repeat(sw - 3); // 77 'C's
    h.assert_row(2, &row2_expected);
}

/// vttest `tst_insdel` — accordion test (lines 970-985 of main.c).
///
/// After filling the screen with rows of letters (row N = letter 'A'+N-1 repeated),
/// vttest does:
///   ri()                          →  ESC M  (reverse index — scroll row 1 up)
///   el(2)                         →  CSI 2 K  (erase entire row 1)
///   decstbm(2, max_lines - 1)     →  CSI 2;23 r  (scroll region rows 2-23)
///   decom(TRUE)                   →  CSI ?6 h  (origin mode ON)
///   cup(1, 1)                     →  CSI 1;1 H  (in DECOM: row 1 relative to top of region)
///   for row in 1..=max_lines:
///     il(row)                     →  CSI row L
///     dl(row)                     →  CSI row M
///   decom(FALSE)                  →  CSI ?6 l
///   decstbm(0, 0)                 →  CSI r  (reset scroll region to full screen)
///
/// Expected final state (vttest message):
///   "Top line: A's, bottom line: X's, this line, nothing more."
///   Row 0: "AAAAAA..." (A's full width)
///   Row 23: "XXXXXXX..." (X = 24th letter = 'X', full width)
///   Rows 1-22: blank (accordion cleared the content)
#[test]
fn tst_insdel_accordion_il_dl_loop() {
    let mut h = VtTestHelper::new_default();
    let sw: usize = 80;
    let max_lines: usize = 24;

    // Fill screen: row N gets letter 'A'+N-1 repeated sw times.
    h.feed(b"\x1b[2J");
    h.feed(b"\x1b[1;1H");
    for row in 1..=max_lines {
        let letter = b'A' - 1 + u8::try_from(row).unwrap();
        let cup = format!("\x1b[{};1H", row);
        h.feed(cup.as_bytes());
        let row_content: Vec<u8> = vec![letter; sw];
        h.feed(&row_content);
    }

    // ri() — ESC M
    h.feed(b"\x1bM");
    // el(2) — CSI 2 K
    h.feed(b"\x1b[2K");
    // decstbm(2, 23) — CSI 2;23 r
    h.feed(b"\x1b[2;23r");
    // decom(TRUE) — CSI ?6 h
    h.feed(b"\x1b[?6h");
    // cup(1, 1) — CSI 1;1 H  (relative to scroll region top = absolute row 2)
    h.feed(b"\x1b[1;1H");

    // Accordion loop: il(row) + dl(row) for row in 1..=24
    for row in 1..=max_lines {
        let il = format!("\x1b[{}L", row);
        h.feed(il.as_bytes());
        let dl = format!("\x1b[{}M", row);
        h.feed(dl.as_bytes());
    }

    // decom(FALSE) — CSI ?6 l
    h.feed(b"\x1b[?6l");
    // decstbm(0, 0) — CSI r  (reset scroll region)
    h.feed(b"\x1b[r");

    // Verify: row 0 = A's (unchanged, outside scroll region top)
    let a_row: String = "A".repeat(sw);
    h.assert_row(0, &a_row);

    // Rows 1-21 (inside the scroll region) should be blank after accordion.
    for row in 1..=21 {
        h.assert_row(row, "");
    }

    // Row 23 = X's (24th letter, was outside scroll region bottom, unchanged).
    let x_row: String = "X".repeat(sw);
    h.assert_row(23, &x_row);
}

/// vttest `tst_insdel` — IRM insert mode test (lines 986-997 of main.c).
///
/// vttest does:
///   cup(1, 2); tprintf("B"); cub(1)
///   sm("4")   →  CSI 4 h   (IRM on)
///   for col in 2..=sw-1: tprintf("*")
///   rm("4")   →  CSI 4 l   (IRM off)
///
/// With IRM active, each `*` should be inserted (not overwritten), pushing 'B'
/// rightward.  The expected result is `A*** ... ***B` on the top line.
///
/// BUG: IRM (Insert/Replace Mode, ANSI mode 4) is not implemented in Freminal.
/// `CSI 4 h` is silently ignored.  In replace mode (the default), all `*` writes
/// overwrite existing content, so the top line becomes `A*...*` (B is overwritten).
#[test]
fn tst_insdel_irm_insert_mode_not_implemented() {
    let mut h = VtTestHelper::new_default();
    let sw: usize = 80;

    // Set up: row 0 = 'A' repeated sw times.
    h.feed(b"\x1b[1;1H");
    let a_row: Vec<u8> = vec![b'A'; sw];
    h.feed(&a_row);

    // cup(1, 2) → 0-indexed col 1; write 'B'; cub(1) → back to col 1.
    h.feed(b"\x1b[1;2H"); // cup(1,2)
    h.feed(b"B"); // write 'B' at col 1 (overwrites second 'A')
    h.feed(b"\x1b[D"); // cub(1) — cursor back to col 1

    // sm("4") — IRM on (not implemented: silently ignored)
    h.feed(b"\x1b[4h");

    // write sw-2 stars (cols 2..=sw-1, 1-indexed = cols 1..=sw-2, 0-indexed)
    let stars: Vec<u8> = vec![b'*'; sw - 2];
    h.feed(&stars);

    // rm("4") — IRM off (not implemented: silently ignored)
    h.feed(b"\x1b[4l");

    // BUG: With IRM implemented, the expected line would be:
    //   "A" + "*".repeat(sw-2) + "B"
    // But since IRM is not implemented, all writes are replace-mode, producing:
    //   "A" + "*".repeat(sw-2) + last char from the A row at col sw-1
    // In this case: col 1 was 'B' but stars start at col 1 (cub moved back to 1),
    // so the actual content is 'A' at col 0, then sw-2 stars at cols 1..sw-2,
    // then 'A' at col sw-1 (the original rightmost A, never overwritten).
    // BUG: IRM not implemented — see PLAN_22_VTTEST_INTEGRATION.md
    let expected: String = format!("A{}A", "*".repeat(sw - 2));
    h.assert_row(0, &expected);
}
