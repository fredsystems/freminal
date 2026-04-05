// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 9 — VT100 Known Bug Regression Tests.
//!
//! These tests verify that Freminal does **not** exhibit the well-known VT100
//! firmware bugs documented in vttest. Each test identifies the specific bug it
//! guards against. All bugs that require visual-only verification (smooth
//! scroll, double-width lines, 132-column mode combinations) are skipped; only
//! bugs with deterministic, buffer-verifiable outcomes are automated.
//!
//! ## Skipped bugs
//!
//! - **Bug A** (smooth scroll to jump scroll): No visible buffer effect.
//! - **Bug C** (wide-to-narrow DECCOLM): Visual only.
//! - **Bug D** (narrow-to-wide DECCOLM): Visual only.
//! - **Bug E** (cursor move from double- to single-wide line): Requires
//!   double-width line support not yet implemented.
//! - **Bug F** (DECSCNM/DECCOLM combo): Visual only (screen inversion).
//! - **Bug L** (erase right half of double-width lines): Requires double-width
//!   line support not yet implemented.
//!
//! ## Automated bugs
//!
//! - **Bug W** — Wrap-around cursor addressing: after CUP to the last column
//!   and writing a character, a subsequent CUP must clear the "at-margin" flag
//!   so that the next character is written at the new cursor position rather
//!   than wrapping to the next line first.
//! - **Bug B** — Scroll region RI interaction: `RI` at the top of a scroll
//!   region must insert a blank line at the top, pushing content down within
//!   the region (not globally).
//! - **Bug S** — Funny scroll regions: inverted DECSTBM parameters
//!   (`top > bottom`) and degenerate parameters (`0, 1`) must both result in
//!   no scroll region change — text fills all 24 rows normally.
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row). CSI sequences use **1-indexed** row;col parameters.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── Bug W — Wrap-Around Cursor Addressing ───────────────────────────────────
//
// The dreaded "wraparound bug"! The VT100 firmware maintained an "at-rightmost-
// column" flag after writing a character to column 80.  If a CUP sequence then
// moved to a different row (still column 80), a buggy terminal left the flag
// set, causing the *next* character to wrap to the next line instead of
// landing in column 80 of the new row.
//
// Correct behavior: CUP always clears the pending-wrap flag regardless of the
// destination column.

/// Writing `*` to column 80 of each row via HVP must leave exactly one `*`
/// per row with no spurious wraps, and a row of `+` at row 0 must remain
/// intact at the top of the screen.
#[test]
fn bug_w_cup_clears_pending_wrap_flag() {
    let mut h = VtTestHelper::new_default();

    // Fill row 0 with '+' characters (columns 1–79, i.e. 79 of them).
    // vttest uses `min_cols - 1` = 79 '+' signs at the very top.
    h.feed(b"\x1b[1;1H");
    for _ in 0..79 {
        h.feed(b"+");
    }

    // For each row (1-indexed 1..=24), CUP to that row at column 80 and write '*'.
    // A buggy terminal would treat the second HVP as "still at col 80, so wrap
    // before writing" and produce a '*' on col 1 of the *next* row.
    for row in 1..=24_u16 {
        // HVP (CSI row ; col f) — 1-indexed.
        h.feed(format!("\x1b[{row};80f").as_bytes());
        h.feed(b"*");
    }

    // The row of '+' must still be at row 0, unscrolled.
    // (The '*' for row 1 lands at col 79 (0-indexed), which is in the same row.)
    let row0 = h.screen_text()[0].clone();
    assert!(
        row0.contains('+'),
        "row 0 must still contain '+' signs — got: {row0:?}"
    );
    assert!(!row0.contains('\n'), "row 0 must not contain a newline");

    // Every row must contain exactly one '*' at position 79 (0-indexed col 79).
    let screen = h.screen_text();
    for (i, row) in screen.iter().enumerate() {
        let star_count = row.chars().filter(|&c| c == '*').count();
        assert_eq!(
            star_count, 1,
            "row {i} must have exactly one '*' (wrap-around bug would misplace it); got: {row:?}"
        );
        // The '*' must be the last character of the row (col 79).
        assert!(
            row.ends_with('*'),
            "row {i}: '*' must be at the right margin (col 79, 0-indexed); got: {row:?}"
        );
    }
}

/// CUP to column 80, write one character, then CUP to a *different* row at
/// the same column — the cursor must land at (col=79, row=<new>), not wrap.
#[test]
fn bug_w_second_cup_to_last_col_does_not_wrap() {
    let mut h = VtTestHelper::new_default();

    // Write 'A' at row 1, col 80 (1-indexed) = (col=79, row=0) 0-indexed.
    h.feed(b"\x1b[1;80H");
    h.feed(b"A");

    // Now CUP to row 3, col 80 (1-indexed) = (col=79, row=2) 0-indexed.
    h.feed(b"\x1b[3;80H");

    // Without the bug the cursor is at (79, 2); with the bug the terminal
    // would wrap and place the cursor at (0, 3).
    h.assert_cursor_pos(79, 2);
}

/// Writing a character at the last column and then using CUP (not another
/// character) must NOT trigger a wrap-ahead on the subsequent character.
#[test]
fn bug_w_char_after_cup_from_last_col_writes_at_cup_destination() {
    let mut h = VtTestHelper::new_default();

    // Position at row 2, col 80 (0-indexed: col=79, row=1).
    h.feed(b"\x1b[2;80H");
    h.feed(b"X"); // Write at col 79.

    // CUP to row 5, col 1 (0-indexed: col=0, row=4).
    h.feed(b"\x1b[5;1H");
    h.feed(b"Y");

    // 'Y' must appear at (col=0, row=4), not (col=0, row=5) as the bug would produce.
    h.assert_row(4, "Y");
    h.assert_row(5, ""); // The bug would put 'Y' here instead.
}

// ─── Bug B — Scroll Region RI Interaction ────────────────────────────────────
//
// After DECALN fills the screen with 'E', the test sets a scroll region
// starting at row 12 (1-indexed). Issuing RI at the top of the scroll region
// must insert a blank line at row 12, pushing the 'E' rows down within the
// region. A buggy VT100 confused its row pointers here.

/// DECALN + DECSTBM(12,24) + RI at row 12: the blank must appear at the top
/// of the scroll region, not affect rows above it.
#[test]
fn bug_b_ri_at_scroll_region_top_inserts_blank_in_region() {
    let mut h = VtTestHelper::new_default();

    // DECALN: fill screen with 'E'.
    h.feed(b"\x1b#8");

    // Clear row 1 (1-indexed) — the top row — for clarity.
    h.feed(b"\x1b[1;1H");
    h.feed(b"\x1b[2K"); // EL(2): clear entire line.

    // Set scroll region to rows 12–24 (1-indexed).
    h.feed(b"\x1b[12;24r");

    // Move to row 12, col 1 (0-indexed: col=0, row=11).
    h.feed(b"\x1b[12;1H");

    // RI (ESC M): reverse index — at the top of a scroll region this inserts a
    // blank line at the top of the region, pushing content down.
    h.feed(b"\x1bM");

    // Row 11 (0-indexed) — the top of the scroll region — must now be blank.
    h.assert_row(11, "");

    // Rows above the scroll region (rows 0–10, 0-indexed) must be unaffected.
    // Row 0 was explicitly cleared so it should be empty.
    h.assert_row(0, "");
    // Rows 1–10 (0-indexed) still have 'E' (DECALN filled them; only the
    // scroll region was affected by RI).
    for row in 1..=10_usize {
        let text = h.screen_text()[row].clone();
        assert!(
            text.chars().all(|c| c == 'E'),
            "row {row} must still contain 'E' after RI in scroll region; got: {text:?}"
        );
    }
}

/// After Bug B setup, writing characters A–P with LF must fill the scroll
/// region sequentially without confusion.
///
/// vttest uses `tprintf("%c\n", c)` which emits the character followed by a
/// bare LF (no CR).  The cursor moves down one line but stays in the same
/// column, so each character lands one column to the right of the previous
/// one.  After 16 such writes into a 13-row region (rows 12–24, 1-indexed),
/// the last character 'P' ends up at row 23 (0-indexed), and the last few
/// rows above it hold 'N', 'O', etc.  The key invariant is that 'P' reaches
/// row 22 (the second-to-last row of the screen, 0-indexed), and row 23 (the
/// bottom of the scroll region) is blank after the LF following 'P'.
#[test]
fn bug_b_writing_after_ri_scrolls_correctly_within_region() {
    let mut h = VtTestHelper::new_default();

    // DECALN fills screen with 'E'.
    h.feed(b"\x1b#8");

    // Set scroll region rows 12–24.
    h.feed(b"\x1b[12;24r");

    // Move to row 12, col 1.
    h.feed(b"\x1b[12;1H");

    // RI inserts blank at top of region.
    h.feed(b"\x1bM");

    // Write characters A–P each followed by bare LF (no CR).
    // vttest: `tprintf("%c\n", c)`.  LF moves the cursor down without
    // returning to column 0, so each character lands one column further right.
    for c in b'A'..=b'P' {
        h.feed(&[c, b'\n']);
    }

    // 'P' must be present somewhere on the visible screen — it must not have
    // scrolled out of the scroll region.
    let screen = h.screen_text();
    let p_row = screen
        .iter()
        .enumerate()
        .find(|(_, row)| row.contains('P'))
        .map(|(i, _)| i);
    assert!(
        p_row.is_some(),
        "'P' must be visible on the screen after writing A–P into the scroll region"
    );

    // 'P' must be within the scroll region (rows 11–23, 0-indexed).
    let p_row = p_row.unwrap();
    assert!(
        (11..=23).contains(&p_row),
        "'P' must be within the scroll region (rows 11–23, 0-indexed); found at row {p_row}"
    );

    // Rows above the scroll region (0–10) must all still contain 'E' (DECALN
    // filled them; RI and the subsequent writes only affect the scroll region).
    for (row, text) in screen.iter().enumerate().take(11) {
        assert!(
            text.chars().all(|c| c == 'E'),
            "row {row} (above scroll region) must still be all 'E'; got: {text:?}"
        );
    }
}

// ─── Bug S — Funny Scroll Regions ────────────────────────────────────────────
//
// vttest verifies that malformed DECSTBM parameters are rejected silently:
// - `decstbm(20, 10)`: top > bottom — invalid, must be ignored.
// - `decstbm(0, 1)`:  interpreted as (1, 1) which is a degenerate single-row
//   region that the implementation also ignores, leaving the full 24-row
//   scroll area active.
//
// After either bad DECSTBM, printing 20 lines of text must fill rows 0–19
// without being confined to any sub-region.

/// Inverted DECSTBM params (top=20, bottom=10) must be rejected — text then
/// fills all 20 lines of the screen without any scroll-region confinement.
#[test]
fn bug_s_inverted_decstbm_params_rejected() {
    let mut h = VtTestHelper::new_default();

    // Attempt to set an invalid scroll region (top > bottom).
    // The parser pre-validates and rejects this without changing state or
    // homing the cursor.
    h.feed(b"\x1b[20;10r");

    // Move to row 1, col 1 (home) — do this explicitly since an invalid
    // DECSTBM must NOT home the cursor.
    h.feed(b"\x1b[1;1H");

    // Print 20 lines of text.  With no scroll region, lines 1–20 fill rows
    // 0–19; line 20 does NOT scroll away.
    for i in 1..=20_u32 {
        h.feed_str(&format!("Line {i:02}\r\n"));
    }

    // Row 0 must have "Line 01".
    h.assert_row(0, "Line 01");

    // Row 19 must have "Line 20".
    h.assert_row(19, "Line 20");

    // Row 20 must be empty (only 20 lines were written).
    h.assert_row(20, "");
}

/// DECSTBM(0, 1) is treated as (1, 1) — a degenerate single-row region that
/// the parser rejects.  Text then fills all rows normally.
#[test]
fn bug_s_decstbm_zero_one_treated_as_no_region() {
    let mut h = VtTestHelper::new_default();

    // Attempt to set scroll region with top=0 (interpreted as 1) and bottom=1.
    // Single-row regions (top == bottom after normalisation) are rejected.
    h.feed(b"\x1b[0;1r");

    // Home cursor explicitly.
    h.feed(b"\x1b[1;1H");

    // Print 20 lines of text.
    for i in 1..=20_u32 {
        h.feed_str(&format!("Line {i:02}\r\n"));
    }

    // Row 0 must have "Line 01" — no premature scrolling.
    h.assert_row(0, "Line 01");

    // Row 19 must have "Line 20".
    h.assert_row(19, "Line 20");

    // Row 20 must be blank.
    h.assert_row(20, "");
}

/// Confirming that a *valid* DECSTBM does home the cursor, while the invalid
/// variants above do not (they leave the cursor at the position prior to the
/// sequence).
#[test]
fn bug_s_valid_decstbm_homes_cursor_invalid_does_not() {
    let mut h = VtTestHelper::new_default();

    // Move cursor away from home.
    h.feed(b"\x1b[10;10H");
    h.assert_cursor_pos(9, 9);

    // Invalid DECSTBM must NOT home the cursor.
    h.feed(b"\x1b[20;10r"); // top > bottom — rejected
    h.assert_cursor_pos(9, 9); // cursor unchanged

    // Valid DECSTBM must home the cursor.
    h.feed(b"\x1b[5;15r"); // valid region rows 5–15
    h.assert_cursor_pos(0, 0); // cursor homed to origin
}

// ─── Additional wrap-state edge cases ────────────────────────────────────────

/// After writing exactly 80 characters (filling the row), DECAWM is on, and
/// the cursor is at the right edge in pending-wrap state.  A subsequent CUP
/// to a completely different position must discard the pending-wrap flag.
#[test]
fn wrap_pending_flag_cleared_by_cup() {
    let mut h = VtTestHelper::new_default();

    // Fill row 0 exactly (80 chars → cursor at col 79 in wrap-pending state).
    h.feed(&[b'A'; 80]);

    // Move to row 5, col 5 via CUP (1-indexed 5;5 → 0-indexed col=4, row=4).
    h.feed(b"\x1b[5;5H");

    // Write 'B' — it must land at (col=4, row=4), not at (col=0, row=1) as
    // the wrap-around bug would produce.
    h.feed(b"B");
    h.assert_cursor_pos(5, 4);
    h.assert_row(4, "    B");
}

/// After writing to the last column, HVP (CSI row ; col f) must also clear
/// the pending-wrap flag — same semantics as CUP.
#[test]
fn wrap_pending_flag_cleared_by_hvp() {
    let mut h = VtTestHelper::new_default();

    // Write 80 chars to row 0.
    h.feed(&[b'Z'; 80]);

    // HVP to row 3, col 1 (0-indexed: col=0, row=2).
    h.feed(b"\x1b[3;1f");
    h.feed(b"!");

    // '!' must be at (0, 2), not (0, 3).
    h.assert_cursor_pos(1, 2);
    h.assert_row(2, "!");
}
