// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 1 — Cursor Movement Tests.
//!
//! Exercises every cursor movement operation covered by vttest Menu 1:
//!
//! - CUF (`CSI C`) — cursor forward
//! - CUB (`CSI D`) — cursor backward
//! - CUU (`CSI A`) — cursor up
//! - CUD (`CSI B`) — cursor down
//! - CUP (`CSI H`) — cursor position (absolute)
//! - HVP (`CSI f`) — horizontal and vertical position (same semantics as CUP)
//! - ED  (`CSI J`) — erase in display (Ps=0/1/2)
//! - EL  (`CSI K`) — erase in line (Ps=0/1/2)
//! - DECALN (`ESC # 8`) — screen alignment pattern (fills with 'E')
//! - DECAWM (`CSI ?7 h/l`) — auto-wrap mode on/off
//! - IND (`ESC D`) — index (scroll up at bottom margin)
//! - NEL (`ESC E`) — next line (CR + LF)
//! - RI  (`ESC M`) — reverse index (scroll down at top margin)
//! - DECSTBM (`CSI r`) — set top and bottom margins, then scroll within region
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row). CSI sequences use **1-indexed** row;col parameters.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── CUF — Cursor Forward ────────────────────────────────────────────────────

#[test]
fn cuf_default_moves_one_column_right() {
    let mut h = VtTestHelper::new_default();
    // Position cursor at col 5 first.
    h.feed(b"\x1b[1;6H"); // CUP row=1, col=6 → (x=5, y=0)
    h.feed(b"\x1b[C"); // CUF with no param — default is 1
    h.assert_cursor_pos(6, 0);
}

#[test]
fn cuf_explicit_count() {
    let mut h = VtTestHelper::new_default();
    // Start at column 0 (origin).
    h.feed(b"\x1b[10C"); // CUF 10 columns
    h.assert_cursor_pos(10, 0);
}

#[test]
fn cuf_clamped_at_right_margin() {
    let mut h = VtTestHelper::new_default();
    // Start at column 0, request a move far beyond the right margin (80).
    h.feed(b"\x1b[9999C"); // Should clamp at col 79 (0-indexed)
    h.assert_cursor_pos(79, 0);
}

#[test]
fn cuf_from_near_right_margin() {
    let mut h = VtTestHelper::new_default();
    // Move to col 77 (0-indexed), then forward 5 — should clamp at 79.
    h.feed(b"\x1b[1;78H"); // CUP row=1, col=78 → (x=77, y=0)
    h.feed(b"\x1b[5C");
    h.assert_cursor_pos(79, 0);
}

// ─── CUB — Cursor Backward ───────────────────────────────────────────────────

#[test]
fn cub_default_moves_one_column_left() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;10H"); // CUP row=1, col=10 → (x=9, y=0)
    h.feed(b"\x1b[D"); // CUB default 1
    h.assert_cursor_pos(8, 0);
}

#[test]
fn cub_explicit_count() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;20H"); // (x=19, y=0)
    h.feed(b"\x1b[5D"); // CUB 5
    h.assert_cursor_pos(14, 0);
}

#[test]
fn cub_clamped_at_left_margin() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;5H"); // (x=4, y=0)
    h.feed(b"\x1b[9999D"); // Should clamp at col 0
    h.assert_cursor_pos(0, 0);
}

// ─── CUU — Cursor Up ─────────────────────────────────────────────────────────

#[test]
fn cuu_default_moves_one_row_up() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;1H"); // row=5, col=1 → (x=0, y=4)
    h.feed(b"\x1b[A"); // CUU default 1
    h.assert_cursor_pos(0, 3);
}

#[test]
fn cuu_explicit_count() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[10;1H"); // (x=0, y=9)
    h.feed(b"\x1b[4A"); // CUU 4
    h.assert_cursor_pos(0, 5);
}

#[test]
fn cuu_clamped_at_top() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[3;1H"); // (x=0, y=2)
    h.feed(b"\x1b[9999A"); // Should clamp at row 0
    h.assert_cursor_pos(0, 0);
}

// ─── CUD — Cursor Down ───────────────────────────────────────────────────────

#[test]
fn cud_default_moves_one_row_down() {
    let mut h = VtTestHelper::new_default();
    // Cursor starts at (0, 0) — move down one row.
    h.feed(b"\x1b[B"); // CUD default 1
    h.assert_cursor_pos(0, 1);
}

#[test]
fn cud_explicit_count() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5B"); // CUD 5
    h.assert_cursor_pos(0, 5);
}

#[test]
fn cud_clamped_at_bottom() {
    let mut h = VtTestHelper::new_default();
    // The terminal is 80x24; bottom row is index 23.
    h.feed(b"\x1b[9999B"); // Should clamp at row 23
    h.assert_cursor_pos(0, 23);
}

// ─── CUP — Cursor Position ───────────────────────────────────────────────────

#[test]
fn cup_moves_to_origin_with_default_params() {
    let mut h = VtTestHelper::new_default();
    // Put cursor somewhere off-origin first.
    h.feed(b"\x1b[10;20H");
    // CSI H with no params defaults to (1,1) → (x=0, y=0).
    h.feed(b"\x1b[H");
    h.assert_cursor_pos(0, 0);
}

#[test]
fn cup_positions_to_middle_of_screen() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[12;40H"); // row=12, col=40 → (x=39, y=11)
    h.assert_cursor_pos(39, 11);
}

#[test]
fn cup_out_of_bounds_row_clamped() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[9999;1H"); // row clamped to 24 → y=23
    h.assert_cursor_pos(0, 23);
}

#[test]
fn cup_out_of_bounds_col_clamped() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[1;9999H"); // col clamped to 80 → x=79
    h.assert_cursor_pos(79, 0);
}

// ─── HVP — Horizontal and Vertical Position ──────────────────────────────────

#[test]
fn hvp_same_semantics_as_cup() {
    let mut h = VtTestHelper::new_default();
    // CSI f (HVP) is functionally identical to CSI H (CUP).
    h.feed(b"\x1b[5;10f"); // row=5, col=10 → (x=9, y=4)
    h.assert_cursor_pos(9, 4);
}

#[test]
fn hvp_default_params_move_to_origin() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[10;10H"); // Move somewhere first.
    h.feed(b"\x1b[f"); // HVP with no params → (x=0, y=0)
    h.assert_cursor_pos(0, 0);
}

// ─── ED — Erase in Display ───────────────────────────────────────────────────

#[test]
fn ed_ps0_erases_from_cursor_to_end() {
    let mut h = VtTestHelper::new_default();
    // Fill a few rows with 'X'.
    h.feed_str(
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\n",
    );
    h.feed_str(
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\n",
    );
    h.feed_str("XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX");
    // Move cursor to row 2, col 10 (0-indexed: y=1, x=9).
    h.feed(b"\x1b[2;10H");
    // ED Ps=0: erase from cursor to end of display.
    h.feed(b"\x1b[0J");
    // Cursor must remain in place.
    h.assert_cursor_pos(9, 1);
    // Row 0 is fully intact.
    h.assert_row(
        0,
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
    );
    // Row 1: cols 0-8 remain, cols 9-79 erased.
    h.assert_row(1, "XXXXXXXXX");
    // Row 2 is fully cleared.
    h.assert_row(2, "");
}

#[test]
fn ed_ps1_erases_from_start_to_cursor() {
    let mut h = VtTestHelper::new_default();
    // Fill three full rows (80 chars each) with 'X'.
    h.feed_str(
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\n",
    );
    h.feed_str(
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX\r\n",
    );
    h.feed_str("XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX");
    // Move cursor to row 2, col 10 (0-indexed: y=1, x=9).
    h.feed(b"\x1b[2;10H");
    // ED Ps=1: erase from start of display to cursor inclusive.
    // Row 0 is fully erased; on row 1 cols 0-9 are erased, cols 10-79 remain.
    h.feed(b"\x1b[1J");
    // Cursor stays in place.
    h.assert_cursor_pos(9, 1);
    // Row 0 is fully erased.
    h.assert_row(0, "");
    // Row 1: cols 0-9 erased (10 spaces), cols 10-79 = 70 Xs remaining.
    h.assert_row(
        1,
        "          XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
    );
    // Row 2 is untouched.
    h.assert_row(
        2,
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
    );
}

#[test]
fn ed_ps2_erases_entire_display() {
    let mut h = VtTestHelper::new_default();
    // Write to several rows.
    h.feed_str("Row zero\r\nRow one\r\nRow two");
    // Position cursor in the middle.
    h.feed(b"\x1b[3;5H");
    // ED Ps=2: erase entire display.
    h.feed(b"\x1b[2J");
    // Cursor stays where CUP put it.
    h.assert_cursor_pos(4, 2);
    // Every row should now be empty.
    for row in 0..24 {
        h.assert_row(row, "");
    }
}

#[test]
fn ed_default_param_behaves_like_ps0() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("AAAAAAAAA\r\nBBBBBBBBB");
    // Position at row 1, col 4 (0-indexed).
    h.feed(b"\x1b[2;5H");
    // ED with no param — must behave like Ps=0 (erase to end of display).
    h.feed(b"\x1b[J");
    h.assert_cursor_pos(4, 1);
    h.assert_row(0, "AAAAAAAAA");
    h.assert_row(1, "BBBB");
}

// ─── EL — Erase in Line ──────────────────────────────────────────────────────

#[test]
fn el_ps0_erases_from_cursor_to_end_of_line() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    // Move to col 4 (0-indexed).
    h.feed(b"\x1b[1;5H");
    // EL Ps=0: erase from cursor to end of line.
    h.feed(b"\x1b[0K");
    h.assert_cursor_pos(4, 0);
    h.assert_row(0, "ABCD");
}

#[test]
fn el_ps1_erases_from_start_of_line_to_cursor() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    // Move to col 4 (0-indexed).
    h.feed(b"\x1b[1;5H");
    // EL Ps=1: erase from start to cursor (inclusive → 5 chars blanked: 0-4).
    h.feed(b"\x1b[1K");
    h.assert_cursor_pos(4, 0);
    // Cols 0-4 erased, cols 5-9 remain as FGHIJ.
    h.assert_row(0, "     FGHIJ");
}

#[test]
fn el_ps2_erases_entire_line() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    h.feed(b"\x1b[1;5H"); // cursor at col 4
    // EL Ps=2: erase entire line.
    h.feed(b"\x1b[2K");
    h.assert_cursor_pos(4, 0);
    h.assert_row(0, "");
}

#[test]
fn el_default_param_behaves_like_ps0() {
    let mut h = VtTestHelper::new_default();
    h.feed_str("ABCDEFGHIJ");
    h.feed(b"\x1b[1;4H"); // col 3 (0-indexed)
    h.feed(b"\x1b[K"); // EL no param → Ps=0
    h.assert_cursor_pos(3, 0);
    h.assert_row(0, "ABC");
}

// ─── DECALN — Screen Alignment Pattern ───────────────────────────────────────

#[test]
fn decaln_fills_entire_screen_with_e() {
    let mut h = VtTestHelper::new_default();
    // Put some content on screen first.
    h.feed_str("Some existing content\r\nSecond line");
    // DECALN: ESC # 8 — fills every cell with 'E' and moves cursor to (0,0).
    h.feed(b"\x1b#8");
    // Every row must be exactly 80 'E's.
    let expected_row = "E".repeat(80);
    for row in 0..24 {
        h.assert_row(row, &expected_row);
    }
}

// ─── DECAWM — Auto-Wrap Mode ─────────────────────────────────────────────────

#[test]
fn decawm_on_wraps_at_right_margin() {
    let mut h = VtTestHelper::new_default();
    // DECAWM is ON by default. Write exactly 81 characters — the 81st should
    // wrap to the next row.
    let line = "A".repeat(80);
    h.feed_str(&line);
    h.feed_str("B"); // 81st character — wraps to row 1, col 0.
    h.assert_row(0, &line);
    h.assert_row(1, "B");
    h.assert_cursor_pos(1, 1);
}

#[test]
fn decawm_off_cursor_stays_at_last_column() {
    let mut h = VtTestHelper::new_default();
    // Disable DECAWM.
    h.feed(b"\x1b[?7l");
    // Write exactly 80 characters — fills row 0 completely (cols 0-79).
    let line = "A".repeat(80);
    h.feed_str(&line);
    // With DECAWM off, writing past the right margin discards the overflow and
    // clamps the cursor to the last column (col 79). The 5 'X' characters are
    // all discarded; row 0 remains unchanged.
    h.feed_str("XXXXX");
    // Row 0 must still be 80 'A's; the 'X' writes were discarded.
    h.assert_row(0, &line);
    // Cursor remains at col 79.
    h.assert_cursor_pos(79, 0);
    // Row 1 must still be empty (no wrap occurred).
    h.assert_row(1, "");
}

// ─── IND — Index ─────────────────────────────────────────────────────────────

#[test]
fn ind_at_bottom_scrolls_content_up() {
    let mut h = VtTestHelper::new_default();
    // Fill the screen with numbered rows.
    for i in 0..24_u8 {
        h.feed_str(&format!("Row {i:02}\r\n"));
    }
    // After filling 24 rows the cursor is on row 23 (bottom). The last feed
    // caused a scroll, so row 0 now contains "Row 01". Verify row 0 first.
    // Send IND (ESC D) — should scroll the entire content up one line.
    h.feed(b"\x1bD");
    // The content that was on row 0 before IND is now gone; what was on row 1
    // is now on row 0.  Rather than hard-coding the exact text from the scroll
    // sequence, just confirm the cursor is still on row 23 (bottom) and that
    // row 23 is now empty (blanked by the scroll).
    h.assert_cursor_pos(0, 23);
    h.assert_row(23, "");
}

#[test]
fn ind_not_at_bottom_moves_cursor_down() {
    let mut h = VtTestHelper::new_default();
    // Place cursor in the middle of the screen; IND should move it down without
    // scrolling.
    h.feed(b"\x1b[5;1H"); // row=5, col=1 → (x=0, y=4)
    h.feed_str("Middle");
    h.feed(b"\x1b[5;1H"); // back to start of row 4
    h.feed(b"\x1bD"); // IND
    // Cursor should move to row 5 (0-indexed: y=5), same column.
    h.assert_cursor_pos(0, 5);
    // The text written to row 4 must still be there.
    h.assert_row(4, "Middle");
}

// ─── NEL — Next Line ─────────────────────────────────────────────────────────

#[test]
fn nel_acts_as_cr_plus_lf() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[3;10H"); // row=3, col=10 → (x=9, y=2)
    // NEL (ESC E) should move to col 0, row+1.
    h.feed(b"\x1bE");
    h.assert_cursor_pos(0, 3);
}

#[test]
fn nel_at_bottom_row_scrolls() {
    let mut h = VtTestHelper::new_default();
    // Move to bottom row.
    h.feed(b"\x1b[24;5H"); // row=24, col=5 → (x=4, y=23)
    h.feed_str("BOTTOM");
    h.feed(b"\x1b[24;5H"); // Reset to start of "BOTTOM"
    // NEL at the bottom should scroll up and place cursor at col 0, row 23.
    h.feed(b"\x1bE");
    h.assert_cursor_pos(0, 23);
    // The original bottom row's content scrolled up — row 23 is now blank.
    h.assert_row(23, "");
}

// ─── RI — Reverse Index ───────────────────────────────────────────────────────

#[test]
fn ri_not_at_top_moves_cursor_up() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[5;1H"); // (x=0, y=4)
    h.feed(b"\x1bM"); // RI
    h.assert_cursor_pos(0, 3);
}

#[test]
fn ri_at_top_scrolls_content_down() {
    let mut h = VtTestHelper::new_default();
    // Write some content to rows 0 and 1.
    h.feed_str("First line\r\nSecond line");
    // Move cursor to the top row.
    h.feed(b"\x1b[1;1H"); // (x=0, y=0)
    // RI (ESC M) at the top should insert a blank line at the top, pushing
    // existing content down.
    h.feed(b"\x1bM");
    // Cursor stays on row 0 (top).
    h.assert_cursor_pos(0, 0);
    // Row 0 is now blank (the newly inserted line).
    h.assert_row(0, "");
    // "First line" has been pushed to row 1.
    h.assert_row(1, "First line");
    // "Second line" has been pushed to row 2.
    h.assert_row(2, "Second line");
}

// ─── DECSTBM — Set Top and Bottom Margins ────────────────────────────────────

#[test]
fn decstbm_ind_scrolls_within_region_only() {
    let mut h = VtTestHelper::new_default();
    // Write content to rows 0-5.
    for i in 0..6_u8 {
        h.feed_str(&format!("Line {i:02}\r\n"));
    }
    // Set scroll region: rows 3-6 (1-indexed) → rows 2-5 (0-indexed).
    h.feed(b"\x1b[3;6r");
    // Move cursor to the bottom of the scroll region (row 6 = y=5).
    h.feed(b"\x1b[6;1H");
    // IND should scroll only within the region.
    h.feed(b"\x1bD");
    // Cursor stays at the bottom of the region.
    h.assert_cursor_pos(0, 5);
    // Row 5 (bottom of region) is now blank.
    h.assert_row(5, "");
    // Rows outside the region (0-1) are unchanged.
    h.assert_row(0, "Line 00");
    h.assert_row(1, "Line 01");
}

#[test]
fn decstbm_cup_resets_on_margin_set() {
    let mut h = VtTestHelper::new_default();
    // Write text, move cursor to some arbitrary position.
    h.feed(b"\x1b[10;10H");
    // Setting DECSTBM resets the cursor to (0, 0).
    h.feed(b"\x1b[5;15r");
    h.assert_cursor_pos(0, 0);
}

#[test]
fn decstbm_reset_restores_full_screen_scroll() {
    let mut h = VtTestHelper::new_default();
    // Set a restricted region.
    h.feed(b"\x1b[5;10r");
    // Now clear the region (reset to full screen by sending default params).
    h.feed(b"\x1b[r");
    h.assert_cursor_pos(0, 0);
    // Verify scroll happens across the full screen now.
    // Move to bottom row and send IND.
    h.feed(b"\x1b[24;1H");
    h.feed_str("Bottom");
    h.feed(b"\x1b[24;1H");
    h.feed(b"\x1bD");
    // After IND from the bottom of the full screen, cursor remains on row 23.
    h.assert_cursor_pos(0, 23);
}

// ─── Combined / Regression ───────────────────────────────────────────────────

#[test]
fn cup_then_cuf_cub_cuu_cud_sequence() {
    // Chain several relative cursor moves to verify they compose correctly.
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[12;40H"); // (x=39, y=11) — middle of screen
    h.feed(b"\x1b[5C"); // CUF 5 → (x=44, y=11)
    h.assert_cursor_pos(44, 11);
    h.feed(b"\x1b[3A"); // CUU 3 → (x=44, y=8)
    h.assert_cursor_pos(44, 8);
    h.feed(b"\x1b[2B"); // CUD 2 → (x=44, y=10)
    h.assert_cursor_pos(44, 10);
    h.feed(b"\x1b[10D"); // CUB 10 → (x=34, y=10)
    h.assert_cursor_pos(34, 10);
}

#[test]
fn decaln_then_ed_clears_screen() {
    let mut h = VtTestHelper::new_default();
    // Fill with 'E' using DECALN.
    h.feed(b"\x1b#8");
    // ED Ps=2: erase entire display.
    h.feed(b"\x1b[2J");
    for row in 0..24 {
        h.assert_row(row, "");
    }
}

// ─── Minimal autowrap-at-region-bottom reproduction ──────────────────────────

/// Minimal test: autowrap at the bottom of a DECSTBM scroll region.
///
/// Setup: 80x24, scroll region rows 3-5 (3-row region), DECOM on.
/// Sequence: CUP to last row, col 80, write two chars.
/// Expected: first char fills col 79, second char autowraps to next row at
///           col 0 — since we're at the bottom margin, the region scrolls up.
#[test]
fn autowrap_at_scroll_region_bottom_minimal() {
    let mut h = VtTestHelper::new_default();

    // DECSTBM 3;5 → scroll region rows 3-5 (1-indexed screen coords)
    h.feed(b"\x1b[3;5r");
    // DECOM on → cursor homes to top of scroll region
    h.feed(b"\x1b[?6h");

    // Fill the 3 rows of the scroll region with identifiable content:
    // Row 3 (region row 1): "AAAA..."
    // Row 4 (region row 2): "BBBB..."
    // Row 5 (region row 3): "CCCC..."
    h.feed(b"\x1b[1;1H"); // CUP(1,1) relative to region = row 3 screen
    h.feed(b"AAAAAAAAAA");
    h.feed(b"\x1b[2;1H"); // CUP(2,1) relative to region = row 4 screen
    h.feed(b"BBBBBBBBBB");
    h.feed(b"\x1b[3;1H"); // CUP(3,1) relative to region = row 5 screen
    h.feed(b"CCCCCCCCCC");

    // Now: CUP to region row 3, col 80 (last column of bottom row)
    h.feed(b"\x1b[3;80H");
    // Write two characters: 'X' fills col 79, 'Y' triggers autowrap
    h.feed(b"XY");

    // Expected result:
    // - Region scrolled up: row A scrolled off, B moved up, C moved up
    // - The bottom row (region row 3 = screen row 5) was blanked by scroll,
    //   then 'Y' was written at col 0 of that row.
    // Screen row 3 (region 1): "BBBBBBBBBB"
    // Screen row 4 (region 2): "CCCCCCCCC" + 'X' at col 79
    // Screen row 5 (region 3): "Y" at col 0

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate() {
        eprintln!("row {:2}: {:?}", i, line);
    }
    let cursor = h.cursor_pos();
    eprintln!("cursor: ({}, {})", cursor.x, cursor.y);

    // Row 2 (screen, 0-indexed) = scroll region row 1 → was "BBBB..."
    let row2 = &screen[2];
    assert!(
        row2.starts_with("BBBBBBBBBB"),
        "region row 1 should now have B's (scrolled up from row 2): got {:?}",
        row2
    );

    // Row 3 (screen, 0-indexed) = scroll region row 2 → was "CCCC..." + X at col 79
    let row3 = &screen[3];
    assert!(
        row3.starts_with("CCCCCCCCCC"),
        "region row 2 should start with C's: got {:?}",
        row3
    );
    // Col 79 should be 'X'
    let row3_chars: Vec<char> = row3.chars().collect();
    assert_eq!(
        row3_chars.get(79).copied(),
        Some('X'),
        "col 79 of region row 2 should be 'X': row = {:?}",
        row3
    );

    // Row 4 (screen, 0-indexed) = scroll region row 3 → blanked, then Y at col 0
    let row4 = &screen[4];
    assert!(
        row4.starts_with('Y'),
        "region row 3 should start with 'Y' (autowrapped): got {:?}",
        row4
    );

    // Cursor should be at col 1 (just wrote Y at col 0), row 4 (screen)
    assert_eq!(cursor.x, 1, "cursor x after writing Y");
}

// ─── vttest Section 3 — Cursor-Control Characters Inside ESC Sequences ──────
//
// vttest main.c lines 499-529.
// Tests that BS, CR, and VT embedded inside CSI parameter strings are handled
// correctly by the terminal. The expected output is four identical lines:
//   "A B C D E F G H I"
//
// Line 1 (reference): plain text "A B C D E F G H I"
// Line 2 (BS-in-CUF): Each char is printed, then CSI "2<BS>C" is sent.
//     The BS inside the CSI is processed as a C0 control (moves cursor back),
//     while the CSI parameter "2" followed by "C" moves forward by 2. Net
//     effect: print char, BS moves back 1, CUF(2) moves forward 2 = advance 2.
// Line 3 (CR-in-CUF): "A " then for each subsequent char, CSI with embedded CR
//     resets column to 0, then CUF(2*i-2) positions correctly.
// Line 4 (VT-in-CUU): Each "X " pair printed, then CSI "1<VT>A" — the VT
//     inside the CSI moves cursor down one line (VT = vertical tab = line feed
//     in most terminals), then CUU(1) moves back up. Net effect: no vertical
//     movement, characters appear on the same line.

/// Build the exact byte sequence for vttest cursor-control-inside-ESC test.
fn build_cursor_control_in_esc_bytes() -> Vec<u8> {
    let mut out = Vec::new();

    // vt_clear(2) → ESC[2J
    out.extend_from_slice(b"\x1b[2J");
    // vt_move(1,1) → ESC[1;1H
    out.extend_from_slice(b"\x1b[1;1H");

    // println("Test of cursor-control characters inside ESC sequences.")
    out.extend_from_slice(b"Test of cursor-control characters inside ESC sequences.\r\n");
    // println("Below should be four identical lines:")
    out.extend_from_slice(b"Below should be four identical lines:\r\n");
    // println("")
    out.extend_from_slice(b"\r\n");
    // println("A B C D E F G H I")  — the reference line
    out.extend_from_slice(b"A B C D E F G H I\r\n");

    // Line 2: BS embedded in CUF
    // for (i = 1; i < 10; i++) { tprintf("%c", '@'+i); do_csi("2%cC", BS); }
    for i in 1..10u8 {
        out.push(b'@' + i); // 'A', 'B', ..., 'I'
        // do_csi("2%cC", BS) → ESC [ 2 BS C
        out.extend_from_slice(b"\x1b[2");
        out.push(0x08); // BS
        out.push(b'C');
    }
    // println("")
    out.extend_from_slice(b"\r\n");

    // Line 3: CR embedded in CUF
    // tprintf("A ");
    out.extend_from_slice(b"A ");
    // for (i = 2; i < 10; i++) {
    //   cprintf("%s%c%dC", csi_output(), CR, 2*i-2);
    //   tprintf("%c", '@'+i);
    // }
    for i in 2..10u8 {
        // ESC [ CR <2*i-2> C
        out.extend_from_slice(b"\x1b[");
        out.push(0x0D); // CR
        out.extend_from_slice(format!("{}C", 2 * u16::from(i) - 2).as_bytes());
        out.push(b'@' + i); // 'B', 'C', ..., 'I'
    }
    // println("")
    out.extend_from_slice(b"\r\n");

    // Line 4: VT in CUU
    // rm("20") → ESC [ 20l   (reset LNM — line feed/new line mode)
    out.extend_from_slice(b"\x1b[20l");
    // for (i = 1; i < 10; i++) {
    //   tprintf("%c ", '@'+i);
    //   do_csi("1\013A");   ← \013 = 0x0B = VT
    // }
    for i in 1..10u8 {
        out.push(b'@' + i); // 'A', ..., 'I'
        out.push(b' ');
        // do_csi("1\013A") → ESC [ 1 VT A
        out.extend_from_slice(b"\x1b[1");
        out.push(0x0B); // VT
        out.push(b'A');
    }
    // println("")
    out.extend_from_slice(b"\r\n");
    // println("")
    out.extend_from_slice(b"\r\n");

    out
}

#[test]
fn cursor_control_characters_inside_esc_sequences() {
    let mut h = VtTestHelper::new_default();
    let bytes = build_cursor_control_in_esc_bytes();
    h.feed(&bytes);

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate().take(10) {
        eprintln!("row {:2}: {:?}", i, line);
    }

    // Row 0: header
    h.assert_row(0, "Test of cursor-control characters inside ESC sequences.");
    // Row 1: header
    h.assert_row(1, "Below should be four identical lines:");
    // Row 2: blank
    h.assert_row(2, "");
    // Rows 3-6: four identical lines "A B C D E F G H I"
    let expected_line = "A B C D E F G H I";
    h.assert_row(3, expected_line);
    h.assert_row(4, expected_line);
    h.assert_row(5, expected_line);
    h.assert_row(6, expected_line);
}

// ─── vttest Section 4 — Leading Zeros in ESC Sequences ──────────────────────
//
// vttest main.c lines 533-546.
// Tests that leading zeros in CSI numeric parameters are parsed correctly.
// The sequence "ESC [ 00000000004 ; 00000000<col> H" should position the cursor
// at row 4, column <col>, just as "ESC [ 4 ; <col> H" would.

/// Build the exact byte sequence for vttest leading-zeros test.
fn build_leading_zeros_test_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let ctext = b"This is a correct sentence";

    // vt_clear(2) → ESC[2J
    out.extend_from_slice(b"\x1b[2J");
    // vt_move(1,1) → ESC[1;1H
    out.extend_from_slice(b"\x1b[1;1H");

    // println("Test of leading zeros in ESC sequences.")
    out.extend_from_slice(b"Test of leading zeros in ESC sequences.\r\n");
    // printxx("Two lines below you should see the sentence \"%s\".", ctext)
    // Note: printxx does NOT append CR LF — it's like printf
    out.extend_from_slice(
        b"Two lines below you should see the sentence \"This is a correct sentence\".",
    );

    // for (col = 1; *ctext; col++) {
    //   cprintf("%s00000000004;00000000%dH", csi_output(), col);
    //   tprintf("%c", *ctext++);
    // }
    for (idx, &ch) in ctext.iter().enumerate() {
        let col = idx + 1;
        // ESC [ 00000000004 ; 00000000<col> H
        out.extend_from_slice(format!("\x1b[00000000004;00000000{col}H").as_bytes());
        out.push(ch);
    }

    // cup(20, 1) → ESC[20;1H
    out.extend_from_slice(b"\x1b[20;1H");

    out
}

#[test]
fn leading_zeros_in_esc_sequences() {
    let mut h = VtTestHelper::new_default();
    let bytes = build_leading_zeros_test_bytes();
    h.feed(&bytes);

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate().take(6) {
        eprintln!("row {:2}: {:?}", i, line);
    }
    let cursor = h.cursor_pos();
    eprintln!("cursor: ({}, {})", cursor.x, cursor.y);

    // Row 0: header
    h.assert_row(0, "Test of leading zeros in ESC sequences.");
    // Row 1: description (printed with printxx, no CR LF at end — but the
    // leading-zeros CUP sequences move cursor to row 4 before it wraps)
    h.assert_row(
        1,
        "Two lines below you should see the sentence \"This is a correct sentence\".",
    );
    // Row 2: blank
    h.assert_row(2, "");
    // Row 3 (screen row 4, 1-indexed): the sentence written char-by-char
    h.assert_row(3, "This is a correct sentence");
    // Cursor should be at row 19 (0-indexed), col 0 — from cup(20, 1)
    h.assert_cursor_pos(0, 19);
}

// ─── vttest Section 1 — Box-Drawing Test (80-col) ───────────────────────────
//
// vttest main.c lines 320-433, pass=0 (80 columns).
// Draws a box made of '*' and '+' characters around the screen border, with a
// frame of 'E's (from DECALN) in the middle. Exercises:
// - DECALN (fill screen with 'E')
// - ED Ps=0/1 (erase in display)
// - EL Ps=0/1/2 (erase in line)
// - HVP (horizontal and vertical position)
// - CUP (cursor position)
// - CUB/CUF/CUU/CUD (relative cursor movement)
// - IND (index / scroll up)
// - RI (reverse index / scroll down)
// - NEL (next line)
//
// The expected result is a screen with:
// - Row 1 and row 24: all '*' characters (80 wide)
// - Rows 2-23: '*' at col 1 and col 80, '+' at col 2 and col 79
// - Rows 9-16 inner area: mix of blanks and 'E's forming a frame
// - Row 17: fully cleared by EL(2)
// - Rows 10-15 inner area: descriptive text

/// Build the exact byte sequence for vttest box test (pass=0, 80 columns).
fn build_box_test_bytes_80col() -> Vec<u8> {
    let mut out = Vec::new();
    let width: usize = 80;
    let max_lines: usize = 24;
    let inner_l: usize = (width - 60) / 2; // = 10
    let inner_r: usize = 61 + inner_l; // = 71
    let hlfxtra: usize = (width - 80) / 2; // = 0

    // deccolm(FALSE) → ESC[?3l (80 cols, clears screen)
    out.extend_from_slice(b"\x1b[?3l");

    // decaln() → ESC#8
    out.extend_from_slice(b"\x1b#8");

    // cup(9, inner_l) → ESC[9;10H
    out.extend_from_slice(format!("\x1b[9;{inner_l}H").as_bytes());
    // ed(1) → ESC[1J
    out.extend_from_slice(b"\x1b[1J");

    // cup(18, 60 + hlfxtra) → ESC[18;60H
    out.extend_from_slice(format!("\x1b[18;{}H", 60 + hlfxtra).as_bytes());
    // ed(0) → ESC[0J
    out.extend_from_slice(b"\x1b[0J");
    // el(1) → ESC[1K
    out.extend_from_slice(b"\x1b[1K");

    // cup(9, inner_r) → ESC[9;71H
    out.extend_from_slice(format!("\x1b[9;{inner_r}H").as_bytes());
    // el(0) → ESC[0K
    out.extend_from_slice(b"\x1b[0K");

    // for row = 10..16
    for row in 10..=16 {
        // cup(row, inner_l) → ESC[row;10H
        out.extend_from_slice(format!("\x1b[{row};{inner_l}H").as_bytes());
        // el(1) → ESC[1K
        out.extend_from_slice(b"\x1b[1K");
        // cup(row, inner_r) → ESC[row;71H
        out.extend_from_slice(format!("\x1b[{row};{inner_r}H").as_bytes());
        // el(0) → ESC[0K
        out.extend_from_slice(b"\x1b[0K");
    }

    // cup(17, 30) → ESC[17;30H
    out.extend_from_slice(b"\x1b[17;30H");
    // el(2) → ESC[2K
    out.extend_from_slice(b"\x1b[2K");

    // Draw top and bottom rows of '*'
    // for col = 1..width
    for col in 1..=width {
        // hvp(max_lines, col) → ESC[24;<col>f
        out.extend_from_slice(format!("\x1b[{max_lines};{col}f").as_bytes());
        out.push(b'*');
        // hvp(1, col) → ESC[1;<col>f
        out.extend_from_slice(format!("\x1b[1;{col}f").as_bytes());
        out.push(b'*');
    }

    // Draw left border with '+' using IND
    // cup(2, 2) → ESC[2;2H
    out.extend_from_slice(b"\x1b[2;2H");
    for _row in 2..=max_lines - 1 {
        out.push(b'+');
        // cub(1) → ESC[1D
        out.extend_from_slice(b"\x1b[1D");
        // ind() → ESC D
        out.extend_from_slice(b"\x1bD");
    }

    // Draw right border with '+' using RI
    // cup(max_lines-1, width-1) → ESC[23;79H
    out.extend_from_slice(format!("\x1b[{};{}H", max_lines - 1, width - 1).as_bytes());
    for _row in (2..=max_lines - 1).rev() {
        out.push(b'+');
        // cub(1) → ESC[1D
        out.extend_from_slice(b"\x1b[1D");
        // ri() → ESC M
        out.extend_from_slice(b"\x1bM");
    }

    // Draw left/right '*' on each row and navigate down
    // cup(2, 1) → ESC[2;1H
    out.extend_from_slice(b"\x1b[2;1H");
    for row in 2..=max_lines - 1 {
        out.push(b'*');
        // cup(row, width) → ESC[row;80H
        out.extend_from_slice(format!("\x1b[{row};{width}H").as_bytes());
        out.push(b'*');
        // cub(10) → ESC[10D
        out.extend_from_slice(b"\x1b[10D");
        if row < 10 {
            // nel() → ESC E
            out.extend_from_slice(b"\x1bE");
        } else {
            // tprintf("\n") with set_tty_crmod(TRUE) → PTY line discipline
            // converts LF to CR+LF. In raw byte tests we must emit CR+LF explicitly.
            out.extend_from_slice(b"\r\n");
        }
    }

    // Draw top border '+' row
    // cup(2, 10) → ESC[2;10H
    out.extend_from_slice(b"\x1b[2;10H");
    // cub(42 + hlfxtra) → ESC[42D
    out.extend_from_slice(format!("\x1b[{}D", 42 + hlfxtra).as_bytes());
    // cuf(2) → ESC[2C
    out.extend_from_slice(b"\x1b[2C");
    for _col in 3..=width - 2 {
        out.push(b'+');
        // cuf(0) → ESC[0C
        out.extend_from_slice(b"\x1b[0C");
        // cub(2) → ESC[2D
        out.extend_from_slice(b"\x1b[2D");
        // cuf(1) → ESC[1C
        out.extend_from_slice(b"\x1b[1C");
    }

    // Draw bottom border '+' row
    // cup(max_lines-1, inner_r-1) → ESC[23;70H
    out.extend_from_slice(format!("\x1b[{};{}H", max_lines - 1, inner_r - 1).as_bytes());
    // cuf(42 + hlfxtra) → ESC[42C
    out.extend_from_slice(format!("\x1b[{}C", 42 + hlfxtra).as_bytes());
    // cub(2) → ESC[2D
    out.extend_from_slice(b"\x1b[2D");
    for _col in (3..=width - 2).rev() {
        out.push(b'+');
        // cub(1) → ESC[1D
        out.extend_from_slice(b"\x1b[1D");
        // cuf(1) → ESC[1C
        out.extend_from_slice(b"\x1b[1C");
        // cub(0) → ESC[0D
        out.extend_from_slice(b"\x1b[0D");
        // BS → 0x08
        out.push(0x08);
    }

    // CUU/CUD clamping tests
    // cup(1, 1) → ESC[1;1H
    out.extend_from_slice(b"\x1b[1;1H");
    // cuu(10) → ESC[10A
    out.extend_from_slice(b"\x1b[10A");
    // cuu(1) → ESC[1A
    out.extend_from_slice(b"\x1b[1A");
    // cuu(0) → ESC[0A
    out.extend_from_slice(b"\x1b[0A");
    // cup(max_lines, width) → ESC[24;80H
    out.extend_from_slice(format!("\x1b[{max_lines};{width}H").as_bytes());
    // cud(10) → ESC[10B
    out.extend_from_slice(b"\x1b[10B");
    // cud(1) → ESC[1B
    out.extend_from_slice(b"\x1b[1B");
    // cud(0) → ESC[0B
    out.extend_from_slice(b"\x1b[0B");

    // Clear inner area and write descriptive text
    // cup(10, 2 + inner_l) → ESC[10;12H
    out.extend_from_slice(format!("\x1b[10;{}H", 2 + inner_l).as_bytes());
    for _row in 10..=15 {
        for _col in (2 + inner_l)..=(inner_r - 2) {
            out.push(b' ');
        }
        // cud(1) → ESC[1B
        out.extend_from_slice(b"\x1b[1B");
        // cub(58) → ESC[58D
        out.extend_from_slice(b"\x1b[58D");
    }
    // cuu(5) → ESC[5A
    out.extend_from_slice(b"\x1b[5A");
    // cuf(1) → ESC[1C
    out.extend_from_slice(b"\x1b[1C");
    // printxx("The screen should be cleared,  and have an unbroken bor-")
    out.extend_from_slice(b"The screen should be cleared,  and have an unbroken bor-");
    // cup(12, inner_l + 3) → ESC[12;13H
    out.extend_from_slice(format!("\x1b[12;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"der of *'s and +'s around the edge,   and exactly in the");
    // cup(13, inner_l + 3) → ESC[13;13H
    out.extend_from_slice(format!("\x1b[13;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"middle  there should be a frame of E's around this  text");
    // cup(14, inner_l + 3) → ESC[14;13H
    out.extend_from_slice(format!("\x1b[14;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"with  one (1) free position around it.    ");

    out
}

#[test]
#[allow(clippy::needless_range_loop)]
fn box_drawing_test_80col() {
    let mut h = VtTestHelper::new_default();
    let bytes = build_box_test_bytes_80col();
    h.feed(&bytes);

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate() {
        eprintln!("row {:2}: {:?}", i, line);
    }
    let cursor = h.cursor_pos();
    eprintln!("cursor: ({}, {})", cursor.x, cursor.y);

    // Row 0 (screen row 1): all '*' — 80 characters
    h.assert_row(0, &"*".repeat(80));
    // Row 23 (screen row 24): all '*' — 80 characters
    h.assert_row(23, &"*".repeat(80));

    // Rows 1-22: first and last characters should be '*'
    for row in 1..=22 {
        let text = &screen[row];
        let chars: Vec<char> = text.chars().collect();
        assert!(!chars.is_empty(), "row {row} should not be empty");
        assert_eq!(
            chars[0], '*',
            "row {row} col 0 should be '*', got {:?}",
            chars[0]
        );
        // The last character (col 79) should be '*'
        assert_eq!(
            chars.len(),
            80,
            "row {row} should be exactly 80 chars, got {}",
            chars.len()
        );
        assert_eq!(
            chars[79], '*',
            "row {row} col 79 should be '*', got {:?}",
            chars[79]
        );
    }

    // Rows 1-22: col 1 and col 78 should be '+'
    for row in 1..=22 {
        let text = &screen[row];
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(
            chars[1], '+',
            "row {row} col 1 should be '+', got {:?}",
            chars[1]
        );
        assert_eq!(
            chars[78], '+',
            "row {row} col 78 should be '+', got {:?}",
            chars[78]
        );
    }

    // Row 1 (screen row 2): should be "*+...+*" with '+' border along the top
    // Cols 2-77 should all be '+'
    let row1_chars: Vec<char> = screen[1].chars().collect();
    for col in 2..=77 {
        assert_eq!(
            row1_chars[col], '+',
            "row 1 col {col} should be '+', got {:?}",
            row1_chars[col]
        );
    }

    // Row 22 (screen row 23): should be "*+...+*" with '+' border along bottom
    let row22_chars: Vec<char> = screen[22].chars().collect();
    for col in 2..=77 {
        assert_eq!(
            row22_chars[col], '+',
            "row 22 col {col} should be '+', got {:?}",
            row22_chars[col]
        );
    }

    // After DECALN + erases, the E-frame structure (0-indexed) is:
    //   Rows 0-7: fully erased by ED(1) at cup(9,10) — only border chars remain
    //   Row 8: E's in cols 10-69 (inner_l..inner_r-1, 0-indexed 9..70)
    //   Row 9: E at col 9 and col 70 only (inner clear wiped cols 11-68)
    //   Rows 10-13: E at col 9 and col 70 (text written in middle)
    //   Row 14: E at col 9 and col 70 only
    //   Row 15: E's in cols 10-69 (bottom E-bar)
    //   Row 16: fully erased by EL(2)
    //   Rows 17-22: erased by ED(0) at cup(18,60)

    // Rows 2-7 should be blank between borders (cleared by ED(1))
    for row in 2..=7 {
        let chars: Vec<char> = screen[row].chars().collect();
        for col in 2..=77 {
            assert_eq!(
                chars[col], ' ',
                "row {row} col {col} should be blank (cleared by ED(1)), got {:?}",
                chars[col]
            );
        }
    }

    // Row 8: E-frame top bar — E's from col 9 to col 70 (0-indexed)
    // inner_l=10 (1-indexed) = col 9 (0-indexed); inner_r=71 (1-indexed) = col 70 (0-indexed)
    // ED(1) cleared cols 0-9 (0-indexed), EL(0) cleared cols 70-79 (0-indexed)
    // So E's remain at 0-indexed cols 10..69
    let row8_chars: Vec<char> = screen[8].chars().collect();
    for col in 10..=69 {
        assert_eq!(
            row8_chars[col], 'E',
            "row 8 col {col} should be 'E' (E-frame top), got {:?}",
            row8_chars[col]
        );
    }

    // Row 9: E-frame sides — E at col 10 and col 69 (0-indexed)
    // ED(1) cleared 0-indexed cols 0-9; EL(0) cleared cols 70-79.
    // Inner clear loop cleared cols 11-68. So E remains at cols 10 and 69.
    let row9_chars: Vec<char> = screen[9].chars().collect();
    assert_eq!(
        row9_chars[10], 'E',
        "row 9 col 10 should be 'E' (E-frame left)"
    );
    assert_eq!(
        row9_chars[69], 'E',
        "row 9 col 69 should be 'E' (E-frame right)"
    );
    // Verify inner area is blank
    for col in 11..=68 {
        assert_eq!(
            row9_chars[col], ' ',
            "row 9 col {col} should be blank (inner area cleared), got {:?}",
            row9_chars[col]
        );
    }

    // Row 15: E-frame bottom bar — E's from col 10 to col 69 (0-indexed)
    // (Row 16 vttest = row 15 0-indexed; not cleared by inner loop which stops at row 15 vttest)
    let row15_chars: Vec<char> = screen[15].chars().collect();
    for col in 10..=69 {
        assert_eq!(
            row15_chars[col], 'E',
            "row 15 col {col} should be 'E' (E-frame bottom), got {:?}",
            row15_chars[col]
        );
    }

    // Row 16 (screen row 17): fully blank except borders (EL(2))
    let row16_chars: Vec<char> = screen[16].chars().collect();
    assert_eq!(row16_chars[0], '*', "row 16 col 0 should be '*'");
    assert_eq!(row16_chars[1], '+', "row 16 col 1 should be '+'");
    for col in 2..=77 {
        assert_eq!(
            row16_chars[col], ' ',
            "row 16 col {col} should be blank (EL(2)), got {:?}",
            row16_chars[col]
        );
    }

    // Rows 17-21: blank between borders (cleared by ED(0) at cup(18,60))
    for row in 17..=21 {
        let chars: Vec<char> = screen[row].chars().collect();
        for col in 2..=77 {
            assert_eq!(
                chars[col], ' ',
                "row {row} col {col} should be blank (cleared by ED(0)), got {:?}",
                chars[col]
            );
        }
    }

    // Check the descriptive text in the middle
    let row10 = &screen[10];
    assert!(
        row10.contains("The screen should be cleared"),
        "row 10 should contain descriptive text, got: {:?}",
        row10
    );
    let row11 = &screen[11];
    assert!(
        row11.contains("der of *'s and +'s around the edge"),
        "row 11 should contain border description, got: {:?}",
        row11
    );
    let row12 = &screen[12];
    assert!(
        row12.contains("middle  there should be a frame of E's"),
        "row 12 should contain E-frame description, got: {:?}",
        row12
    );
    let row13 = &screen[13];
    assert!(
        row13.contains("with  one (1) free position around it"),
        "row 13 should contain position description, got: {:?}",
        row13
    );

    // Cursor should be at the end of the last text line
    // cup(14, inner_l + 3 = 13) → "with  one (1) free position around it.    " (42 chars)
    // Cursor ends at 1-indexed col 13 + 42 = 55, 0-indexed col 54. Row 13 (0-indexed).
    h.assert_cursor_pos(54, 13);
}

// ─── vttest Section 1 — Box-Drawing Test (132-col) ──────────────────────────
//
// vttest main.c lines 320-433, pass=1 (132 columns).
// Same box-drawing test but at 132-column width.

/// Build the exact byte sequence for vttest box test (pass=1, 132 columns).
fn build_box_test_bytes_132col() -> Vec<u8> {
    let mut out = Vec::new();
    let width: usize = 132;
    let max_lines: usize = 24;
    let inner_l: usize = (width - 60) / 2; // = 36
    let inner_r: usize = 61 + inner_l; // = 97
    let hlfxtra: usize = (width - 80) / 2; // = 26

    // deccolm(TRUE) → ESC[?3h (132 cols, clears screen)
    out.extend_from_slice(b"\x1b[?3h");

    // decaln() → ESC#8
    out.extend_from_slice(b"\x1b#8");

    // cup(9, inner_l) → ESC[9;36H
    out.extend_from_slice(format!("\x1b[9;{inner_l}H").as_bytes());
    // ed(1) → ESC[1J
    out.extend_from_slice(b"\x1b[1J");

    // cup(18, 60 + hlfxtra) → ESC[18;86H
    out.extend_from_slice(format!("\x1b[18;{}H", 60 + hlfxtra).as_bytes());
    // ed(0) → ESC[0J
    out.extend_from_slice(b"\x1b[0J");
    // el(1) → ESC[1K
    out.extend_from_slice(b"\x1b[1K");

    // cup(9, inner_r) → ESC[9;97H
    out.extend_from_slice(format!("\x1b[9;{inner_r}H").as_bytes());
    // el(0) → ESC[0K
    out.extend_from_slice(b"\x1b[0K");

    // for row = 10..16
    for row in 10..=16 {
        out.extend_from_slice(format!("\x1b[{row};{inner_l}H").as_bytes());
        out.extend_from_slice(b"\x1b[1K");
        out.extend_from_slice(format!("\x1b[{row};{inner_r}H").as_bytes());
        out.extend_from_slice(b"\x1b[0K");
    }

    // cup(17, 30)
    out.extend_from_slice(b"\x1b[17;30H");
    // el(2) → ESC[2K
    out.extend_from_slice(b"\x1b[2K");

    // Draw top and bottom rows of '*'
    for col in 1..=width {
        out.extend_from_slice(format!("\x1b[{max_lines};{col}f").as_bytes());
        out.push(b'*');
        out.extend_from_slice(format!("\x1b[1;{col}f").as_bytes());
        out.push(b'*');
    }

    // Draw left border with '+' using IND
    out.extend_from_slice(b"\x1b[2;2H");
    for _row in 2..=max_lines - 1 {
        out.push(b'+');
        out.extend_from_slice(b"\x1b[1D");
        out.extend_from_slice(b"\x1bD");
    }

    // Draw right border with '+' using RI
    out.extend_from_slice(format!("\x1b[{};{}H", max_lines - 1, width - 1).as_bytes());
    for _row in (2..=max_lines - 1).rev() {
        out.push(b'+');
        out.extend_from_slice(b"\x1b[1D");
        out.extend_from_slice(b"\x1bM");
    }

    // Draw left/right '*' on each row
    out.extend_from_slice(b"\x1b[2;1H");
    for row in 2..=max_lines - 1 {
        out.push(b'*');
        out.extend_from_slice(format!("\x1b[{row};{width}H").as_bytes());
        out.push(b'*');
        out.extend_from_slice(b"\x1b[10D");
        if row < 10 {
            out.extend_from_slice(b"\x1bE");
        } else {
            // tprintf("\n") with set_tty_crmod(TRUE) → PTY line discipline
            // converts LF to CR+LF. In raw byte tests we must emit CR+LF explicitly.
            out.extend_from_slice(b"\r\n");
        }
    }

    // Draw top border '+' row
    out.extend_from_slice(b"\x1b[2;10H");
    out.extend_from_slice(format!("\x1b[{}D", 42 + hlfxtra).as_bytes());
    out.extend_from_slice(b"\x1b[2C");
    for _col in 3..=width - 2 {
        out.push(b'+');
        out.extend_from_slice(b"\x1b[0C");
        out.extend_from_slice(b"\x1b[2D");
        out.extend_from_slice(b"\x1b[1C");
    }

    // Draw bottom border '+' row
    out.extend_from_slice(format!("\x1b[{};{}H", max_lines - 1, inner_r - 1).as_bytes());
    out.extend_from_slice(format!("\x1b[{}C", 42 + hlfxtra).as_bytes());
    out.extend_from_slice(b"\x1b[2D");
    for _col in (3..=width - 2).rev() {
        out.push(b'+');
        out.extend_from_slice(b"\x1b[1D");
        out.extend_from_slice(b"\x1b[1C");
        out.extend_from_slice(b"\x1b[0D");
        out.push(0x08);
    }

    // CUU/CUD clamping
    out.extend_from_slice(b"\x1b[1;1H");
    out.extend_from_slice(b"\x1b[10A");
    out.extend_from_slice(b"\x1b[1A");
    out.extend_from_slice(b"\x1b[0A");
    out.extend_from_slice(format!("\x1b[{max_lines};{width}H").as_bytes());
    out.extend_from_slice(b"\x1b[10B");
    out.extend_from_slice(b"\x1b[1B");
    out.extend_from_slice(b"\x1b[0B");

    // Clear inner area and write descriptive text
    out.extend_from_slice(format!("\x1b[10;{}H", 2 + inner_l).as_bytes());
    for _row in 10..=15 {
        for _col in (2 + inner_l)..=(inner_r - 2) {
            out.push(b' ');
        }
        out.extend_from_slice(b"\x1b[1B");
        out.extend_from_slice(b"\x1b[58D");
    }
    out.extend_from_slice(b"\x1b[5A");
    out.extend_from_slice(b"\x1b[1C");
    out.extend_from_slice(b"The screen should be cleared,  and have an unbroken bor-");
    out.extend_from_slice(format!("\x1b[12;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"der of *'s and +'s around the edge,   and exactly in the");
    out.extend_from_slice(format!("\x1b[13;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"middle  there should be a frame of E's around this  text");
    out.extend_from_slice(format!("\x1b[14;{}H", inner_l + 3).as_bytes());
    out.extend_from_slice(b"with  one (1) free position around it.    ");

    out
}

#[test]
#[allow(clippy::needless_range_loop)]
fn box_drawing_test_132col() {
    // NOTE: Freminal must support DECCOLM (ESC[?3h) to switch to 132 columns.
    // If DECCOLM is not supported, the terminal remains at 80 columns and
    // this test will fail — document as a BUG.
    let mut h = VtTestHelper::new(132, 24);
    let bytes = build_box_test_bytes_132col();
    h.feed(&bytes);

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate() {
        eprintln!("row {:2}: {:?}", i, line);
    }
    let cursor = h.cursor_pos();
    eprintln!("cursor: ({}, {})", cursor.x, cursor.y);

    // Row 0: all '*' — 132 characters
    h.assert_row(0, &"*".repeat(132));
    // Row 23: all '*' — 132 characters
    h.assert_row(23, &"*".repeat(132));

    // Rows 1-22: first and last characters should be '*'
    for row in 1..=22 {
        let text = &screen[row];
        let chars: Vec<char> = text.chars().collect();
        assert!(
            chars.len() >= 132,
            "row {row} should be at least 132 chars, got {}",
            chars.len()
        );
        assert_eq!(chars[0], '*', "row {row} col 0 should be '*'");
        assert_eq!(chars[131], '*', "row {row} col 131 should be '*'");
        assert_eq!(chars[1], '+', "row {row} col 1 should be '+'");
        assert_eq!(chars[130], '+', "row {row} col 130 should be '+'");
    }

    // Row 1: all '+' between the borders
    let row1_chars: Vec<char> = screen[1].chars().collect();
    for col in 2..=129 {
        assert_eq!(
            row1_chars[col], '+',
            "row 1 col {col} should be '+', got {:?}",
            row1_chars[col]
        );
    }

    // Check descriptive text
    let row10 = &screen[10];
    assert!(
        row10.contains("The screen should be cleared"),
        "row 10 should contain descriptive text, got: {:?}",
        row10
    );
}

// ─── DECAWM — Autowrap 132-col Variant ──────────────────────────────────────
//
// vttest main.c lines 436-496, pass=1 (132 columns).
// Same autowrap test but at 132-column width.

/// Build the exact byte sequence vttest sends for the autowrap test (pass=1, 132 cols).
fn build_autowrap_test_bytes_132col() -> Vec<u8> {
    let mut out = Vec::new();
    let on_left = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let on_right = b"abcdefghijklmnopqrstuvwxyz";
    let width: usize = 132;
    let region: usize = 18; // max_lines(24) - 6

    // deccolm(TRUE) → ESC[?3h  (sets to 132 cols, clears screen)
    out.extend_from_slice(b"\x1b[?3h");

    // println("Test of autowrap, mixing control and print characters.")
    out.extend_from_slice(b"Test of autowrap, mixing control and print characters.\r\n");
    // println("The left/right margins should have letters in order:")
    out.extend_from_slice(b"The left/right margins should have letters in order:\r\n");

    // decstbm(3, region+3) → ESC[3;21r
    out.extend_from_slice(b"\x1b[3;21r");

    // decom(TRUE) → ESC[?6h
    out.extend_from_slice(b"\x1b[?6h");

    for i in 0..26usize {
        match i % 4 {
            0 => {
                out.extend_from_slice(format!("\x1b[{};1H", region + 1).as_bytes());
                out.push(on_left[i]);
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_right[i]);
                out.push(b'\n');
            }
            1 => {
                out.extend_from_slice(format!("\x1b[{};{}H", region, width).as_bytes());
                out.push(on_right[i - 1]);
                out.push(on_left[i]);
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_left[i]);
                out.push(0x08); // BS
                out.push(b' ');
                out.push(on_right[i]);
                out.push(b'\n');
            }
            2 => {
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_left[i]);
                out.push(0x08); // BS
                out.push(0x08); // BS
                out.push(0x09); // TAB
                out.push(0x09); // TAB
                out.push(on_right[i]);
                out.extend_from_slice(format!("\x1b[{};2H", region + 1).as_bytes());
                out.push(0x08); // BS
                out.push(on_left[i]);
                out.push(b'\n');
            }
            _ => {
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(b'\n');
                out.extend_from_slice(format!("\x1b[{};1H", region).as_bytes());
                out.push(on_left[i]);
                out.extend_from_slice(format!("\x1b[{};{}H", region, width).as_bytes());
                out.push(on_right[i]);
            }
        }
    }

    // decom(FALSE) → ESC[?6l
    out.extend_from_slice(b"\x1b[?6l");
    // decstbm(0,0) → ESC[r
    out.extend_from_slice(b"\x1b[r");
    // cup(max_lines-2, 1) → ESC[22;1H
    out.extend_from_slice(b"\x1b[22;1H");

    out
}

#[test]
#[allow(clippy::needless_range_loop)]
fn decawm_mixing_control_and_print_characters_132col() {
    let mut h = VtTestHelper::new(132, 24);
    let bytes = build_autowrap_test_bytes_132col();
    h.feed(&bytes);

    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate() {
        eprintln!("row {:2}: {:?}", i, line);
    }

    // Check header lines (above scroll region)
    h.assert_row(0, "Test of autowrap, mixing control and print characters.");
    h.assert_row(1, "The left/right margins should have letters in order:");

    // Collect left and right margin characters from the scroll region (rows 2-20)
    let mut left_chars = Vec::new();
    let mut right_chars = Vec::new();
    for row_idx in 2..21 {
        let row_text = &screen[row_idx];
        if row_text.is_empty() {
            left_chars.push(' ');
            right_chars.push(' ');
        } else {
            left_chars.push(row_text.chars().next().unwrap_or(' '));
            // Right margin is at col 131 (132-col mode)
            let chars: Vec<char> = row_text.chars().collect();
            right_chars.push(if chars.len() >= 132 {
                chars[131]
            } else {
                chars.last().copied().unwrap_or(' ')
            });
        }
    }
    let left_str: String = left_chars.iter().collect();
    let right_str: String = right_chars.iter().collect();
    eprintln!("left  margin chars: {:?}", left_str);
    eprintln!("right margin chars: {:?}", right_str);

    assert_eq!(
        left_str.trim(),
        "IJKLMNOPQRSTUVWXYZ",
        "left margin letters should be I through Z in order (132-col)",
    );
    assert_eq!(
        right_str.trim(),
        "ijklmnopqrstuvwxyz",
        "right margin letters should be i through z in order (132-col)",
    );
}

// ─── DECAWM — Autowrap Mixing Control and Print Characters ──────────────────
//
// vttest Menu 1 "Test of autowrap, mixing control and print characters."
//
// This test reproduces the exact byte sequence from vttest main.c lines 436-496
// (pass=0, 80 column mode). It exercises:
// - Case 0: Direct write at left margin (col 1) and right margin (col 80)
// - Case 1: Autowrap by writing at col 80 then printing one more char
// - Case 2: TAB clamping at right margin, BS navigation
// - Case 3: LF at right margin (scroll without character write)
//
// The expected result is letters in alphabetical order on both left and right
// margins of the scroll region. With region=18, the 19-row scroll region
// (rows 3-21) shows the last 18 letter pairs (I/i through Z/z) after all 26
// iterations scroll through, plus one blank row at the bottom.

/// Build the exact byte sequence vttest sends for the autowrap test (pass=0).
fn build_autowrap_test_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    let on_left = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let on_right = b"abcdefghijklmnopqrstuvwxyz";
    let width: usize = 80;
    let region: usize = 18; // max_lines(24) - 6

    // deccolm(FALSE) → ESC[?3l  (resets to 80 cols, clears screen)
    out.extend_from_slice(b"\x1b[?3l");

    // println("Test of autowrap, mixing control and print characters.")
    out.extend_from_slice(b"Test of autowrap, mixing control and print characters.\r\n");
    // println("The left/right margins should have letters in order:")
    out.extend_from_slice(b"The left/right margins should have letters in order:\r\n");

    // decstbm(3, region+3) → ESC[3;21r
    out.extend_from_slice(b"\x1b[3;21r");

    // decom(TRUE) → ESC[?6h  (origin mode, homes cursor)
    out.extend_from_slice(b"\x1b[?6h");

    for i in 0..26usize {
        match i % 4 {
            0 => {
                // case 0: draw characters as-is
                // cup(region+1, 1) → ESC[19;1H  then on_left[i]
                out.extend_from_slice(format!("\x1b[{};1H", region + 1).as_bytes());
                out.push(on_left[i]);
                // cup(region+1, width) → ESC[19;80H  then on_right[i]
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_right[i]);
                // LF
                out.push(b'\n');
            }
            1 => {
                // case 1: simple wrapping
                // cup(region, width) → ESC[18;80H  then on_right[i-1] on_left[i]
                out.extend_from_slice(format!("\x1b[{};{}H", region, width).as_bytes());
                out.push(on_right[i - 1]);
                out.push(on_left[i]);
                // cup(region+1, width) → ESC[19;80H
                // then on_left[i] BS SP on_right[i]
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_left[i]);
                out.push(0x08); // BS
                out.push(b' ');
                out.push(on_right[i]);
                // LF
                out.push(b'\n');
            }
            2 => {
                // case 2: tab to right margin
                // cup(region+1, width) → ESC[19;80H
                // then on_left[i] BS BS TAB TAB on_right[i]
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(on_left[i]);
                out.push(0x08); // BS
                out.push(0x08); // BS
                out.push(0x09); // TAB
                out.push(0x09); // TAB
                out.push(on_right[i]);
                // cup(region+1, 2) → ESC[19;2H
                // then BS on_left[i] LF
                out.extend_from_slice(format!("\x1b[{};2H", region + 1).as_bytes());
                out.push(0x08); // BS
                out.push(on_left[i]);
                out.push(b'\n');
            }
            _ => {
                // case 3: newline at right margin
                // cup(region+1, width) → ESC[19;80H  then LF
                out.extend_from_slice(format!("\x1b[{};{}H", region + 1, width).as_bytes());
                out.push(b'\n');
                // cup(region, 1) → ESC[18;1H  then on_left[i]
                out.extend_from_slice(format!("\x1b[{};1H", region).as_bytes());
                out.push(on_left[i]);
                // cup(region, width) → ESC[18;80H  then on_right[i]
                out.extend_from_slice(format!("\x1b[{};{}H", region, width).as_bytes());
                out.push(on_right[i]);
            }
        }
    }

    // decom(FALSE) → ESC[?6l
    out.extend_from_slice(b"\x1b[?6l");
    // decstbm(0,0) → ESC[r
    out.extend_from_slice(b"\x1b[r");
    // cup(max_lines-2, 1) → ESC[22;1H
    out.extend_from_slice(b"\x1b[22;1H");

    out
}

#[test]
fn decawm_mixing_control_and_print_characters() {
    let mut h = VtTestHelper::new_default();
    let bytes = build_autowrap_test_bytes();
    h.feed(&bytes);

    // After the test, the screen should show:
    // Row 0 (screen row 1): "Test of autowrap, mixing control and print characters."
    // Row 1 (screen row 2): "The left/right margins should have letters in order:"
    // Rows 2-20 (screen rows 3-21): scroll region content
    //   After 26 iterations each scrolling the region, the first 8 letter pairs
    //   (A/a through H/h) have scrolled off. The remaining visible content in
    //   the 19-row scroll region (rows 2-20, 0-indexed) should be:
    //
    //   Row  2 (SR row 1):  I ... i    (i=8,  case 0)
    //   Row  3 (SR row 2):  J ... j    (i=9,  case 1)
    //   Row  4 (SR row 3):  K ... k    (i=10, case 2)
    //   Row  5 (SR row 4):  L ... l    (i=11, case 3)
    //   Row  6 (SR row 5):  M ... m    (i=12, case 0)
    //   Row  7 (SR row 6):  N ... n    (i=13, case 1)
    //   Row  8 (SR row 7):  O ... o    (i=14, case 2)
    //   Row  9 (SR row 8):  P ... p    (i=15, case 3)
    //   Row 10 (SR row 9):  Q ... q    (i=16, case 0)
    //   Row 11 (SR row 10): R ... r    (i=17, case 1)
    //   Row 12 (SR row 11): S ... s    (i=18, case 2)
    //   Row 13 (SR row 12): T ... t    (i=19, case 3)
    //   Row 14 (SR row 13): U ... u    (i=20, case 0)
    //   Row 15 (SR row 14): V ... v    (i=21, case 1)
    //   Row 16 (SR row 15): W ... w    (i=22, case 2)
    //   Row 17 (SR row 16): X ... x    (i=23, case 3)
    //   Row 18 (SR row 17): Y ... y    (i=24, case 0)
    //   Row 19 (SR row 18): Z ... z    (i=25, case 1)
    //   Row 20 (SR row 19): (blank — scrolled in by last LF)

    // Debug: print the full screen
    let screen = h.screen_text();
    for (i, line) in screen.iter().enumerate() {
        eprintln!("row {:2}: {:?}", i, line);
    }
    let cursor = h.cursor_pos();
    eprintln!("cursor: ({}, {})", cursor.x, cursor.y);

    // Check the header lines survived (above scroll region)
    h.assert_row(0, "Test of autowrap, mixing control and print characters.");
    h.assert_row(1, "The left/right margins should have letters in order:");

    // Check each letter pair in the scroll region.
    // After 26 iterations through the scroll region, the expected visible
    // content depends on exactly how many scroll-up operations occurred.
    // The correct VT100 behavior produces letters I-Z on left margin and
    // i-z on right margin (rows 2-19), with row 20 blank.
    //
    // For now, just verify the pattern is correct by checking that all
    // case-1 letters appear on the left margin (this catches the autowrap bug).

    // Collect left-margin and right-margin characters from the scroll region
    let mut left_chars = Vec::new();
    let mut right_chars = Vec::new();
    for row_text in screen.iter().take(21).skip(2) {
        if row_text.is_empty() {
            left_chars.push(' ');
            right_chars.push(' ');
        } else {
            left_chars.push(row_text.chars().next().unwrap_or(' '));
            right_chars.push(row_text.chars().last().unwrap_or(' '));
        }
    }
    let left_str: String = left_chars.iter().collect();
    let right_str: String = right_chars.iter().collect();
    eprintln!("left  margin chars: {:?}", left_str);
    eprintln!("right margin chars: {:?}", right_str);

    // The correct output has all letters in strict alphabetical order with
    // no gaps. Every letter from the visible range must appear.
    assert_eq!(
        left_str.trim(),
        "IJKLMNOPQRSTUVWXYZ",
        "left margin letters should be I through Z in order (no gaps)",
    );
    assert_eq!(
        right_str.trim(),
        "ijklmnopqrstuvwxyz",
        "right margin letters should be i through z in order (no gaps)",
    );
}
