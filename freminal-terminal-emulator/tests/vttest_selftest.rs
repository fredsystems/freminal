// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Self-tests for the vttest golden buffer comparison framework.
//!
//! These tests verify that [`VtTestHelper`] correctly:
//! - Constructs an 80x24 terminal.
//! - Feeds data and extracts screen text.
//! - Compares against golden reference files.
//! - Reports clear diffs on mismatch.
//! - Asserts cursor position.

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── Basic construction and screen extraction ───────────────────────────────

#[test]
fn helper_creates_80x24_terminal() {
    let helper = VtTestHelper::new_default();
    assert_eq!(helper.width, 80);
    assert_eq!(helper.height, 24);

    let screen = helper.screen_text();
    assert_eq!(screen.len(), 24, "screen must have exactly 24 rows");

    // All rows should be empty (trimmed trailing spaces).
    for (i, row) in screen.iter().enumerate() {
        assert!(
            row.is_empty(),
            "row {i} of a fresh terminal should be empty, got: {row:?}"
        );
    }
}

#[test]
fn helper_feeds_plain_text() {
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"Hello, World!");

    let screen = helper.screen_text();
    assert_eq!(screen[0], "Hello, World!");
    // Cursor should be right after the text.
    helper.assert_cursor_pos(13, 0);
}

#[test]
fn helper_feeds_text_with_newlines() {
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"Line 1\r\nLine 2\r\nLine 3");

    let screen = helper.screen_text();
    assert_eq!(screen[0], "Line 1");
    assert_eq!(screen[1], "Line 2");
    assert_eq!(screen[2], "Line 3");
    helper.assert_cursor_pos(6, 2);
}

#[test]
fn helper_feeds_escape_sequences() {
    let mut helper = VtTestHelper::new_default();
    // CUP to row 5, col 10 (1-indexed in escape sequence)
    helper.feed(b"\x1b[5;10HX");

    let screen = helper.screen_text();
    assert_eq!(screen[4], "         X");
    helper.assert_cursor_pos(10, 4);
}

#[test]
fn helper_custom_size() {
    let helper = VtTestHelper::new(40, 12);
    assert_eq!(helper.width, 40);
    assert_eq!(helper.height, 12);

    let screen = helper.screen_text();
    assert_eq!(screen.len(), 12);
}

// ─── Screen text extraction edge cases ──────────────────────────────────────

#[test]
fn screen_text_trims_trailing_whitespace() {
    let mut helper = VtTestHelper::new_default();
    // Write text then move cursor far right — the row should still be trimmed.
    helper.feed(b"ABC");
    helper.feed(b"\x1b[1;80H"); // Move cursor to column 80

    let screen = helper.screen_text();
    // Row 0 should be "ABC" not "ABC" followed by 77 spaces.
    assert_eq!(screen[0], "ABC");
}

#[test]
fn screen_text_preserves_internal_spaces() {
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"A   B   C");

    let screen = helper.screen_text();
    assert_eq!(screen[0], "A   B   C");
}

#[test]
fn screen_text_handles_full_width_row() {
    let mut helper = VtTestHelper::new_default();
    // Fill row 0 with 80 'X' characters.
    let row = "X".repeat(80);
    helper.feed(row.as_bytes());

    let screen = helper.screen_text();
    assert_eq!(screen[0], row);
}

// ─── Cursor position assertions ─────────────────────────────────────────────

#[test]
fn cursor_starts_at_origin() {
    let helper = VtTestHelper::new_default();
    helper.assert_cursor_pos(0, 0);
}

#[test]
#[should_panic(expected = "cursor position mismatch")]
fn cursor_assert_fails_on_mismatch() {
    let helper = VtTestHelper::new_default();
    helper.assert_cursor_pos(5, 5); // Should panic — cursor is at (0, 0).
}

// ─── Row assertion ──────────────────────────────────────────────────────────

#[test]
fn assert_row_passes_on_match() {
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"Hello");
    helper.assert_row(0, "Hello");
}

#[test]
#[should_panic(expected = "row 0 content mismatch")]
fn assert_row_fails_on_mismatch() {
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"Hello");
    helper.assert_row(0, "World");
}

// ─── Golden file comparison ─────────────────────────────────────────────────

#[test]
fn golden_comparison_trivial() {
    // This test uses a pre-created golden file for a trivial "Hello" screen.
    // The golden file must exist at tests/golden/self_test_hello.txt.
    let mut helper = VtTestHelper::new_default();
    helper.feed(b"Hello");
    helper.assert_screen("self_test_hello");
}

// ─── PTY write-back drain ───────────────────────────────────────────────────

#[test]
fn drain_pty_writes_captures_da1_response() {
    let mut helper = VtTestHelper::new_default();
    // Drain any startup messages.
    let _ = helper.drain_pty_writes();

    // Send DA1 query: CSI c
    helper.feed(b"\x1b[c");

    let responses = helper.drain_pty_writes_concatenated();
    // DA1 response should start with CSI ? and end with c.
    assert!(
        !responses.is_empty(),
        "DA1 query must produce a PTY write-back"
    );
    let response_str = String::from_utf8_lossy(&responses);
    assert!(
        response_str.starts_with("\x1b[?"),
        "DA1 response must start with CSI ?, got: {response_str:?}"
    );
    assert!(
        response_str.ends_with('c'),
        "DA1 response must end with 'c', got: {response_str:?}"
    );
}

#[test]
fn drain_pty_writes_captures_dsr_response() {
    let mut helper = VtTestHelper::new_default();
    let _ = helper.drain_pty_writes();

    // Send DSR status query: CSI 5 n
    helper.feed(b"\x1b[5n");

    let responses = helper.drain_pty_writes_concatenated();
    // Response should be CSI 0 n (terminal OK).
    assert_eq!(responses, b"\x1b[0n", "DSR Ps=5 must respond with CSI 0 n");
}
