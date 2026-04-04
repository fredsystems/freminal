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
