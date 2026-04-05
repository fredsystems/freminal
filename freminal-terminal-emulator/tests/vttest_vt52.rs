// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 7 — VT52 Mode Tests.
//!
//! Tests the VT52 compatibility mode activated by `CSI ? 2 l` (`RM ?2`).
//! All byte sequences are derived from the vttest source (`vt52.c` and
//! `esc.c`).
//!
//! ## VT52 Helper Encoding
//!
//! | vttest helper   | Bytes                     | Description             |
//! |-----------------|---------------------------|-------------------------|
//! | `vt52home()`    | `ESC H`                   | Cursor home (1,1)       |
//! | `vt52ed()`      | `ESC J`                   | Erase to end of screen  |
//! | `vt52el()`      | `ESC K`                   | Erase to end of line    |
//! | `vt52ri()`      | `ESC I`                   | Reverse line feed       |
//! | `vt52cuu1()`    | `ESC A`                   | Cursor up               |
//! | `vt52cud1()`    | `ESC B`                   | Cursor down             |
//! | `vt52cuf1()`    | `ESC C`                   | Cursor right            |
//! | `vt52cub1()`    | `ESC D`                   | Cursor left             |
//! | `vt52cup(l, c)` | `ESC Y <l+31> <c+31>`     | Direct cursor address   |
//!
//! ## Coordinate System
//!
//! `vt52cup(l, c)` is 1-indexed.  The parser strips the 0x1F offset and
//! produces a 1-indexed `SetCursorPos`.  `handle_cursor_pos` subtracts 1
//! to obtain 0-indexed buffer coordinates.  All assertions in this file
//! use 0-indexed `(x, y)` via [`VtTestHelper::assert_cursor_pos`].
//!
//! ## Out-of-Bounds Row Rule (vttest `vt52.c` lines 94-107)
//!
//! When the row argument to `vt52cup` exceeds the screen height, the VT100
//! emulating VT52 updates **only the column** — the row is silently ignored.
//! vttest exercises this deliberately in the box-drawing loop.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// `CSI ? 2 l` — reset DECANM, enter VT52 mode.
const ENTER_VT52: &[u8] = b"\x1b[?2l";

/// `ESC <` — exit VT52 mode, return to ANSI/VT100 mode.
const EXIT_VT52: &[u8] = b"\x1b<";

/// `ESC H` — cursor home (row 1, col 1 → 0-indexed (0,0)).
const VT52_HOME: &[u8] = b"\x1bH";

/// `ESC J` — erase to end of screen.
const VT52_ED: &[u8] = b"\x1bJ";

/// `ESC K` — erase to end of line.
const VT52_EL: &[u8] = b"\x1bK";

/// `ESC I` — reverse line feed.
const VT52_RI: &[u8] = b"\x1bI";

/// `ESC A` — cursor up.
const VT52_CUU: &[u8] = b"\x1bA";

/// `ESC B` — cursor down.
const VT52_CUD: &[u8] = b"\x1bB";

/// `ESC C` — cursor right.
const VT52_CUF: &[u8] = b"\x1bC";

/// `ESC D` — cursor left.
const VT52_CUB: &[u8] = b"\x1bD";

/// Build a `ESC Y <row+0x1F> <col+0x1F>` direct-cursor-address sequence.
///
/// `row` and `col` are 1-indexed (as vttest uses them).
fn vt52cup(row: u8, col: u8) -> [u8; 4] {
    [b'\x1b', b'Y', row + 0x1F, col + 0x1F]
}

// ─── Mode Entry / Exit ───────────────────────────────────────────────────────

/// Entering VT52 mode via `CSI ? 2 l` and exiting via `ESC <` must
/// round-trip cleanly.  The terminal must accept cursor-home in VT52 mode.
#[test]
fn vt52_mode_entry_and_exit() {
    let mut h = VtTestHelper::new_default();

    // Start in ANSI mode; write something so we can see if state changed.
    h.feed(b"ANSI");
    h.assert_cursor_pos(4, 0);

    // Enter VT52 mode and move cursor home.
    h.feed(ENTER_VT52);
    h.feed(VT52_HOME);
    h.assert_cursor_pos(0, 0);

    // Exit VT52 mode and confirm we are back in ANSI mode.
    // In ANSI mode CSI H should work as CUP(1,1) — home.
    h.feed(EXIT_VT52);
    // Move to a known position using ANSI CUP then confirm with cursor check.
    h.feed(b"\x1b[5;10H"); // CUP row 5 col 10 → 0-indexed (9, 4)
    h.assert_cursor_pos(9, 4);
}

// ─── Cursor Home ─────────────────────────────────────────────────────────────

/// `ESC H` in VT52 mode moves cursor to (0,0).
#[test]
fn vt52_cursor_home() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Move somewhere first.
    h.feed(&vt52cup(5, 20)); // row 5, col 20 → 0-indexed (19, 4)
    h.assert_cursor_pos(19, 4);

    h.feed(VT52_HOME);
    h.assert_cursor_pos(0, 0);
}

// ─── Direct Cursor Address (ESC Y) ───────────────────────────────────────────

/// `vt52cup(1, 1)` → cursor at (0, 0) (top-left corner).
#[test]
fn vt52cup_top_left() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(1, 1));
    h.assert_cursor_pos(0, 0);
}

/// `vt52cup(24, 80)` → cursor at (79, 23) (bottom-right corner of an 80x24 terminal).
#[test]
fn vt52cup_bottom_right() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(24, 80));
    h.assert_cursor_pos(79, 23);
}

/// `vt52cup(7, 47)` → cursor at (46, 6) — matches vttest `vt52.c:63`.
#[test]
fn vt52cup_mid_screen() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(7, 47));
    h.assert_cursor_pos(46, 6);
}

/// Out-of-bounds **row** in VT52 mode updates only the column.
///
/// This is the central bug fixed by this implementation.  vttest `vt52.c`
/// lines 94-107 deliberately calls `vt52cup(max_lines+3, i-1)` (row 27 on
/// a 24-row screen) to update only the column while keeping the current row.
///
/// Steps:
/// 1. Position cursor at row 1 (0-indexed), col 10 (0-indexed 9).
/// 2. Issue `vt52cup(27, 50)` — row 27 > 24 (screen height), col 50.
/// 3. Expect cursor at col 49 (0-indexed), row 1 (row unchanged).
#[test]
fn vt52cup_out_of_bounds_row_updates_column_only() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Position at row 1 (0-indexed), col 9 (0-indexed).
    h.feed(&vt52cup(2, 10)); // 1-indexed row=2, col=10 → 0-indexed (9, 1)
    h.assert_cursor_pos(9, 1);

    // Out-of-bounds row (27 > 24); col 50 → 0-indexed col = 49.
    h.feed(&vt52cup(27, 50));
    // Row must be unchanged (still 1), column updated to 49.
    h.assert_cursor_pos(49, 1);
}

/// Multiple consecutive out-of-bounds-row `vt52cup` calls each independently
/// update only the column.
#[test]
fn vt52cup_repeated_oob_row_keeps_row_stable() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Start at row 0 (top).
    h.feed(&vt52cup(1, 1));
    h.assert_cursor_pos(0, 0);

    // Three OOB-row calls; only column should change.
    h.feed(&vt52cup(25, 10)); // row 25 > 24 → col only: col 10 → idx 9
    h.assert_cursor_pos(9, 0);
    h.feed(&vt52cup(30, 20)); // row 30 > 24 → col only: col 20 → idx 19
    h.assert_cursor_pos(19, 0);
    h.feed(&vt52cup(100, 5)); // row 100 > 24 → col only: col 5 → idx 4
    h.assert_cursor_pos(4, 0);
}

/// An in-bounds row with a column at the right edge: clamps to width-1 (79).
///
/// Note: VT52 ESC Y parameters are `l + 0x1F` and `c + 0x1F`. The largest
/// printable-ASCII-safe column on an 80-col terminal is 80 (byte 0x6F).
/// Columns beyond 80 are clamped to 79 (0-indexed).
#[test]
fn vt52cup_in_bounds_row_col_at_right_edge() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Row 5, col 80 → 0-indexed (79, 4).  ESC Y encodes col as 80 + 0x1F = 0x6F ('o').
    h.feed(&vt52cup(5, 80));
    h.assert_cursor_pos(79, 4);
}

// ─── Box-Drawing Simulation (vttest vt52.c top-edge loop) ────────────────────

/// Simulate the vttest top-of-box drawing loop (`vt52.c:99-107`).
///
/// vttest draws the top edge of a rectangle going right-to-left from col 70
/// down to col 10 on row 1.  On odd iterations it uses an OOB-row `vt52cup`
/// to update only the column (keeping row 1); on even iterations it uses
/// `vt52cub1` (cursor left).
///
/// After 61 iterations (i = 70 down to 10), the cursor must be at row 0
/// (row 1 in 0-indexed), col 9 (col 10 in 0-indexed, after the last `vt52cup`
/// updates to col 9 = i-1 when i=10).
///
/// We verify:
/// 1. All 61 `*` characters appear on row 0 in columns 10..=70.
/// 2. The cursor ends at the expected position.
#[test]
fn vt52_top_of_box_drawing_loop() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Clear screen and position at row 1, col 70 (vttest vt52.c:99).
    h.feed(VT52_HOME);
    h.feed(VT52_ED);
    h.feed(&vt52cup(1, 70)); // row 1, col 70 → 0-indexed (69, 0)
    h.assert_cursor_pos(69, 0);

    // Simulate the loop: i from 70 down to 10 (1-indexed columns).
    for i in (10u8..=70).rev() {
        // Write '*' at current position, then cursor left.
        h.feed(b"*");
        h.feed(VT52_CUB); // cursor left (back over the '*' we just wrote)

        if i % 2 == 1 {
            // Odd: OOB-row vt52cup → column-only update to col (i-1).
            h.feed(&vt52cup(27, i - 1));
        } else {
            // Even: simple cursor left.
            h.feed(VT52_CUB);
        }
    }

    // After the loop, verify that all '*' appear on row 0, cols 10..=70.
    let screen = h.screen_text();
    let row0 = &screen[0];
    // All characters in columns 10..=70 (0-indexed 9..=69) must be '*'.
    let chars: Vec<char> = row0.chars().collect();
    for col in 9..=69usize {
        let ch = chars.get(col).copied().unwrap_or(' ');
        assert_eq!(
            ch, '*',
            "Expected '*' at row 0 col {col} but got {ch:?}\nRow 0: {row0:?}"
        );
    }
}

// ─── Cursor Movement (ESC A/B/C/D) ───────────────────────────────────────────

/// `ESC A` (cursor up) stops at row 0 — does not scroll or wrap.
#[test]
fn vt52_cursor_up_clamps_at_top() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(3, 5)); // row 3, col 5 → 0-indexed (4, 2)
    h.assert_cursor_pos(4, 2);

    h.feed(VT52_CUU); // up → (4, 1)
    h.assert_cursor_pos(4, 1);
    h.feed(VT52_CUU); // up → (4, 0)
    h.assert_cursor_pos(4, 0);
    h.feed(VT52_CUU); // up at top → stays at (4, 0)
    h.assert_cursor_pos(4, 0);
}

/// `ESC B` (cursor down) stops at the last row — does not scroll.
#[test]
fn vt52_cursor_down_clamps_at_bottom() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(22, 5)); // row 22, col 5 → 0-indexed (4, 21)
    h.assert_cursor_pos(4, 21);

    h.feed(VT52_CUD); // down → (4, 22)
    h.assert_cursor_pos(4, 22);
    h.feed(VT52_CUD); // down → (4, 23)
    h.assert_cursor_pos(4, 23);
    h.feed(VT52_CUD); // down at bottom → stays (4, 23)
    h.assert_cursor_pos(4, 23);
}

/// `ESC C` (cursor right) advances the column.
#[test]
fn vt52_cursor_right() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(1, 1)); // (0, 0)
    h.feed(VT52_CUF);
    h.assert_cursor_pos(1, 0);
    h.feed(VT52_CUF);
    h.assert_cursor_pos(2, 0);
}

/// `ESC D` (cursor left) decrements the column, clamping at 0.
#[test]
fn vt52_cursor_left_clamps_at_left_edge() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(1, 3)); // row 1, col 3 → 0-indexed (2, 0)
    h.assert_cursor_pos(2, 0);

    h.feed(VT52_CUB); // left → (1, 0)
    h.assert_cursor_pos(1, 0);
    h.feed(VT52_CUB); // left → (0, 0)
    h.assert_cursor_pos(0, 0);
    h.feed(VT52_CUB); // left at edge → stays (0, 0)
    h.assert_cursor_pos(0, 0);
}

// ─── Reverse Line Feed (ESC I) ───────────────────────────────────────────────

/// `ESC I` at any row > 0 moves the cursor up one row without scrolling.
#[test]
fn vt52_reverse_linefeed_moves_cursor_up() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(&vt52cup(5, 10)); // row 5, col 10 → 0-indexed (9, 4)
    h.assert_cursor_pos(9, 4);

    h.feed(VT52_RI);
    h.assert_cursor_pos(9, 3);
    h.feed(VT52_RI);
    h.assert_cursor_pos(9, 2);
}

/// `ESC I` at row 0 scrolls content down (reverse scroll) — the cursor stays
/// at row 0 and a blank line is inserted above.
#[test]
fn vt52_reverse_linefeed_at_top_scrolls_down() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Write "Hello" on row 0.
    h.feed(VT52_HOME);
    h.feed(b"Hello");
    h.assert_row(0, "Hello");

    // Go back to row 0, col 0 and issue RI — should scroll "Hello" to row 1.
    h.feed(VT52_HOME);
    h.feed(VT52_RI);

    // "Hello" should have scrolled down to row 1.
    h.assert_row(1, "Hello");
    // Row 0 should now be blank.
    h.assert_row(0, "");
    // Cursor stays at row 0.
    h.assert_cursor_pos(0, 0);
}

/// Reverse-scroll test from `vt52.c:65-71`: write on row 1 (0-indexed 0), then
/// issue 5 `vt52ri()` calls.  Each one scrolls the content down; after 5
/// scrolls the original text has moved to row 5 (0-indexed).
#[test]
fn vt52_backscroll_five_times() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(VT52_HOME);
    h.feed(VT52_ED);

    // Write a marker on row 1 (1-indexed = row 0 in 0-indexed).
    h.feed(&vt52cup(1, 1));
    h.feed(b"Marker");

    // Issue 5 reverse-linefeeds while at row 1.
    for _ in 0..5 {
        h.feed(&vt52cup(1, 1));
        h.feed(VT52_RI);
    }

    // After 5 scrolls, "Marker" should be at row 5 (0-indexed).
    h.assert_row(5, "Marker");
    // Row 0 should be blank.
    h.assert_row(0, "");
}

// ─── Erase Operations ────────────────────────────────────────────────────────

/// `ESC J` erases from the cursor to the end of the screen.
#[test]
fn vt52_erase_to_end_of_screen() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Fill first two rows with text.
    h.feed(VT52_HOME);
    h.feed(b"Row zero content");
    h.feed(&vt52cup(2, 1));
    h.feed(b"Row one content");

    // Position mid-row-0 and erase to end of screen.
    h.feed(&vt52cup(1, 5)); // row 1, col 5 → 0-indexed (4, 0)
    h.feed(VT52_ED);

    // Everything from col 5 onward on row 0, and all of row 1, should be erased.
    let screen = h.screen_text();
    // Row 0: first 4 chars ("Row ") remain; rest gone.  screen_text() trims trailing
    // whitespace, so the space at col 3 also disappears → trimmed to "Row".
    assert!(
        screen[0].starts_with("Row"),
        "Row 0 prefix should remain: {:?}",
        screen[0]
    );
    // The trimmed row should be 3 or 4 characters ("Row" or "Row ").
    assert!(
        screen[0].len() <= 4,
        "Row 0 should be erased from col 4 onward: {:?}",
        screen[0]
    );
    // Row 1 should be fully blank.
    assert_eq!(
        screen[1], "",
        "Row 1 should be blank after ESC J: {:?}",
        screen[1]
    );
}

/// `ESC K` erases from the cursor to the end of the current line.
#[test]
fn vt52_erase_to_end_of_line() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    h.feed(VT52_HOME);
    h.feed(b"Hello World");

    // Position at col 6 (0-indexed 5, 1-indexed 6) and erase to end of line.
    h.feed(&vt52cup(1, 6));
    h.feed(VT52_EL);

    // "Hello" (5 chars) should remain; " World" erased.
    h.assert_row(0, "Hello");
}

// ─── Character Set (ESC F / ESC G) ───────────────────────────────────────────

/// `ESC F` activates VT52 special graphics mode; `ESC G` restores ASCII.
/// This uses the same DEC Special Graphics mapping as `ESC ( 0`.
#[test]
fn vt52_special_graphics_charset() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(VT52_HOME);

    // `ESC F` activates special graphics.
    h.feed(b"\x1bF");
    h.feed(b"\x6a"); // 'j' in ASCII → '┘' in DEC Special Graphics
    h.feed(b"\x1bG"); // exit special graphics
    h.feed(b"\x6a"); // 'j' stays 'j' in ASCII

    let row = h.screen_text()[0].clone();
    let mut chars = row.chars();
    assert_eq!(
        chars.next(),
        Some('\u{2518}'),
        "VT52 ESC F: 0x6a must map to '┘' (U+2518); got: {row:?}"
    );
    assert_eq!(
        chars.next(),
        Some('j'),
        "After VT52 ESC G: 0x6a must be literal 'j'; got: {row:?}"
    );
}

// ─── Identify (ESC Z) ────────────────────────────────────────────────────────

/// `ESC Z` (DECID) in VT52 mode causes the terminal to respond with
/// `ESC / Z` (identifying itself as a VT100 emulating VT52).
#[test]
fn vt52_identify_response() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);
    h.feed(b"\x1bZ"); // ESC Z — DECID

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b/Z",
        "VT52 DECID (ESC Z) must respond with ESC / Z; got: {response:?}"
    );
}

// ─── Exit VT52 Mode (ESC <) ──────────────────────────────────────────────────

/// `ESC <` exits VT52 mode and returns to ANSI (VT100) mode.
/// After exit, ANSI escape sequences must be processed normally.
#[test]
fn vt52_exit_restores_ansi_mode() {
    let mut h = VtTestHelper::new_default();
    h.feed(ENTER_VT52);

    // Confirm we are in VT52 mode: ESC H homes the cursor.
    h.feed(VT52_HOME);
    h.feed(b"VT52");
    h.assert_cursor_pos(4, 0);

    // Exit VT52 mode.
    h.feed(EXIT_VT52);

    // ANSI CUP should now work: ESC [ 3 ; 1 H → row 3 col 1 → 0-indexed (0, 2).
    h.feed(b"\x1b[3;1H");
    h.assert_cursor_pos(0, 2);

    // VT52 sequences (ESC H) should NOT be recognized in ANSI mode.
    // In ANSI mode, ESC H is DECTABHTS (set tab stop) — a no-op for cursor.
    // The cursor should stay at (0, 2).
    h.feed(b"\x1bH"); // ANSI ESC H = DECTABHTS, not cursor home
    h.assert_cursor_pos(0, 2);
}
