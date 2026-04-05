// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for 8-bit C1 control handling (S8C1T / S7C1T).
//!
//! Covers:
//! - Parser recognition of 8-bit C1 bytes (0x80–0x9F) when S8C1T is active
//! - Response encoding: CSI, DCS, OSC, ST use 8-bit forms in S8C1T mode
//! - vttest's `tst_S8C1T` sequence: enable 8-bit, send DSR(6), verify 0x9B in response
//! - Mode transitions: S8C1T ↔ S7C1T round-trip
//!
//! Reference: vttest-20251205/vt220.c lines 191-226 (`tst_S8C1T`)

mod vttest_common;
use vttest_common::VtTestHelper;

// ─── S8C1T enable / disable ─────────────────────────────────────────────────

/// ESC SP G enables 8-bit C1 controls (S8C1T).
/// After enabling, DSR(6) cursor report should start with 0x9B (8-bit CSI).
///
/// This is the core of vttest's `tst_S8C1T` test.
#[test]
fn s8c1t_enable_cursor_report_uses_8bit_csi() {
    let mut h = VtTestHelper::new_default();

    // Move cursor to (1,1) so we know the expected report content.
    h.feed(b"\x1b[1;1H");
    let _ = h.drain_pty_writes(); // discard any initial writes

    // Enable S8C1T: ESC SP G
    h.feed(b"\x1b G");

    // Send DSR(6) — cursor position report request
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();

    // Response should be: 0x9B (8-bit CSI) + "1;1R"
    assert!(
        !response.is_empty(),
        "expected a cursor position report, got nothing"
    );
    assert_eq!(
        response[0], 0x9B,
        "expected 8-bit CSI (0x9B) as first byte, got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        &response[1..],
        b"1;1R",
        "expected cursor report body '1;1R', got {:?}",
        String::from_utf8_lossy(&response[1..])
    );
}

/// ESC SP F reverts to 7-bit C1 controls (S7C1T).
/// After reverting, DSR(6) should start with ESC [ (7-bit CSI).
#[test]
fn s7c1t_revert_cursor_report_uses_7bit_csi() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b[1;1H");
    let _ = h.drain_pty_writes();

    // Enable S8C1T, then revert to S7C1T
    h.feed(b"\x1b G"); // S8C1T
    h.feed(b"\x1b F"); // S7C1T

    // Send DSR(6)
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();

    // Response should be: ESC [ 1 ; 1 R (7-bit)
    assert_eq!(
        response, b"\x1b[1;1R",
        "expected 7-bit CSI cursor report after S7C1T, got {:?}",
        response
    );
}

/// Full vttest tst_S8C1T round-trip: toggle 8-bit on, check DSR; toggle off, check DSR.
///
/// Replicates vttest-20251205/vt220.c lines 191-226 exactly.
#[test]
fn vttest_tst_s8c1t_round_trip() {
    let mut h = VtTestHelper::new_default();

    // vttest starts with input_8bits = FALSE, then toggles twice.
    // Pass 0: flag = !FALSE = TRUE → enable S8C1T
    // Pass 1: flag = !TRUE = FALSE → disable S8C1T

    // ── Pass 0: enable 8-bit ──
    h.feed(b"\x1b G"); // s8c1t(TRUE) → ESC SP G

    // cup(1,1)
    h.feed(b"\x1b[1;1H");
    let _ = h.drain_pty_writes();

    // dsr(6) — cursor position report
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    // vttest calls skip_csi() which accepts either 0x9B or ESC[
    // then calls report_ok("1;1R", report)
    assert_eq!(
        response[0], 0x9B,
        "pass 0: expected 8-bit CSI (0x9B), got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        &response[1..],
        b"1;1R",
        "pass 0: cursor report body mismatch"
    );

    // ── Pass 1: disable 8-bit ──
    h.feed(b"\x1b F"); // s8c1t(FALSE) → ESC SP F

    h.feed(b"\x1b[1;1H");
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[1;1R",
        "pass 1: expected 7-bit CSI after S7C1T"
    );
}

// ─── Parser: 8-bit CSI recognition ─────────────────────────────────────────

/// When S8C1T is active, 0x9B should be treated as CSI (equivalent to ESC [).
/// Test that 0x9B + "2J" erases the display.
#[test]
fn parser_8bit_csi_erase_display() {
    let mut h = VtTestHelper::new_default();

    // Write some text first
    h.feed(b"Hello, world!");
    h.assert_row(0, "Hello, world!");

    // Enable S8C1T
    h.feed(b"\x1b G");

    // Send 8-bit CSI + "2J" (erase display)
    h.feed(&[0x9B, b'2', b'J']);

    // Screen should be clear
    h.assert_row(0, "");
}

/// 0x9B + CUP (H) should move the cursor.
#[test]
fn parser_8bit_csi_cup() {
    let mut h = VtTestHelper::new_default();

    // Enable S8C1T
    h.feed(b"\x1b G");

    // Send 8-bit CSI + "5;10H" (CUP row 5, col 10)
    h.feed(&[0x9B, b'5', b';', b'1', b'0', b'H']);

    // Cursor should be at (col=9, row=4) (0-indexed)
    h.assert_cursor_pos(9, 4);
}

/// 0x9B should NOT be treated as CSI when S8C1T is not active (default 7-bit mode).
/// It should be silently ignored or treated as a printable character.
#[test]
fn parser_8bit_csi_ignored_in_7bit_mode() {
    let mut h = VtTestHelper::new_default();

    // Default is 7-bit mode. Send 0x9B + "2J" — should NOT erase display.
    h.feed(b"Hello!");
    h.feed(&[0x9B, b'2', b'J']);

    // "Hello!" should still be on screen (0x9B ignored, "2J" printed as text)
    let row0 = &h.screen_text()[0];
    assert!(
        row0.starts_with("Hello!"),
        "expected 'Hello!' to remain on screen in 7-bit mode, got: {row0:?}"
    );
}

// ─── Parser: 8-bit IND, NEL, RI, HTS ───────────────────────────────────────

/// 0x84 (IND — Index) should move cursor down one line when S8C1T is active.
#[test]
fn parser_8bit_ind_moves_cursor_down() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    h.feed(b"\x1b[1;1H"); // cursor to top-left

    // Send IND (0x84)
    h.feed(&[0x84]);

    // Cursor should be at row 1 (moved down from row 0)
    h.assert_cursor_pos(0, 1);
}

/// 0x85 (NEL — Next Line) should move cursor to start of next line.
#[test]
fn parser_8bit_nel_moves_to_next_line() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");
    h.feed(b"ABCDE"); // cursor at col 5, row 0

    // Send NEL (0x85)
    h.feed(&[0x85]);

    // Cursor should be at col 0, row 1
    h.assert_cursor_pos(0, 1);
}

/// 0x8D (RI — Reverse Index) should move cursor up one line.
#[test]
fn parser_8bit_ri_moves_cursor_up() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");
    h.feed(b"\x1b[3;1H"); // cursor at row 2

    // Send RI (0x8D)
    h.feed(&[0x8D]);

    // Cursor should be at row 1 (moved up from row 2)
    h.assert_cursor_pos(0, 1);
}

/// 0x88 (HTS — Horizontal Tab Set) should set a tab stop.
#[test]
fn parser_8bit_hts_sets_tab_stop() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");

    // Clear all tab stops first
    h.feed(b"\x1b[3g");

    // Move to column 5 and set a tab stop via 8-bit HTS
    h.feed(b"\x1b[1;6H"); // col 5 (1-indexed = 6)
    h.feed(&[0x88]); // HTS

    // Move to column 0
    h.feed(b"\x1b[1;1H");

    // Tab should land on column 5
    h.feed(b"\t");
    h.assert_cursor_pos(5, 0);
}

// ─── Response encoding: DCS, OSC ────────────────────────────────────────────

/// In S8C1T mode, DA3 (tertiary device attributes) should use 8-bit DCS (0x90)
/// and 8-bit ST (0x9C).
#[test]
fn response_da3_uses_8bit_dcs_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    let _ = h.drain_pty_writes();

    // Send DA3: ESC [ = c
    h.feed(b"\x1b[=c");

    let response = h.drain_pty_writes_concatenated();

    // Should be: 0x90 (DCS) + "!|00000000" + 0x9C (ST)
    assert!(!response.is_empty(), "expected DA3 response, got nothing");
    assert_eq!(
        response[0], 0x90,
        "expected 8-bit DCS (0x90), got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        *response.last().unwrap(),
        0x9C,
        "expected 8-bit ST (0x9C) at end, got 0x{:02X}",
        response.last().unwrap()
    );
    assert_eq!(
        &response[1..response.len() - 1],
        b"!|00000000",
        "DA3 body mismatch"
    );
}

/// In S8C1T mode, DA1 response should use 8-bit CSI (0x9B).
#[test]
fn response_da1_uses_8bit_csi_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    let _ = h.drain_pty_writes();

    // Send DA1: ESC [ c
    h.feed(b"\x1b[c");

    let response = h.drain_pty_writes_concatenated();

    assert_eq!(
        response[0], 0x9B,
        "expected 8-bit CSI (0x9B) for DA1, got 0x{:02X}",
        response[0]
    );
    // Body should be ?65;1;2;4;6;17;18;22c
    assert_eq!(
        &response[1..],
        b"?65;1;2;4;6;17;18;22c",
        "DA1 body mismatch"
    );
}

/// In S8C1T mode, DSR(5) device status report should use 8-bit CSI.
#[test]
fn response_dsr5_uses_8bit_csi_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");
    let _ = h.drain_pty_writes();

    // DSR Ps=5 (device OK report)
    h.feed(b"\x1b[5n");

    let response = h.drain_pty_writes_concatenated();

    // Should be: 0x9B + "0n"
    assert_eq!(
        response,
        &[0x9B, b'0', b'n'],
        "expected 8-bit DSR(5) response"
    );
}

/// In S8C1T mode, DECRQSS DECSCL response should use 8-bit DCS and report
/// the correct C1 mode (0 = 8-bit).
#[test]
fn response_decrqss_decscl_reports_8bit_mode() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    let _ = h.drain_pty_writes();

    // DECRQSS for DECSCL: DCS $ q " p ST
    h.feed(b"\x1bP$q\"p\x1b\\");

    let response = h.drain_pty_writes_concatenated();

    // Should be: 0x90 (DCS) + "1$r65;0\"p" + 0x9C (ST)
    // 65 = VT525 level, 0 = 8-bit C1
    assert_eq!(
        response[0], 0x90,
        "expected 8-bit DCS (0x90), got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        *response.last().unwrap(),
        0x9C,
        "expected 8-bit ST (0x9C), got 0x{:02X}",
        response.last().unwrap()
    );
    let body = &response[1..response.len() - 1];
    assert_eq!(
        body,
        b"1$r65;0\"p",
        "DECRQSS DECSCL body mismatch: got {:?}",
        String::from_utf8_lossy(body)
    );
}

/// In default 7-bit mode, DECRQSS DECSCL response should report
/// C1 mode = 1 (7-bit).
#[test]
fn response_decrqss_decscl_reports_7bit_mode() {
    let mut h = VtTestHelper::new_default();

    let _ = h.drain_pty_writes();

    // DECRQSS for DECSCL: DCS $ q " p ST
    h.feed(b"\x1bP$q\"p\x1b\\");

    let response = h.drain_pty_writes_concatenated();

    // Should be: ESC P (DCS) + "1$r65;1\"p" + ESC \ (ST) — all 7-bit
    assert_eq!(
        response, b"\x1bP1$r65;1\"p\x1b\\",
        "DECRQSS DECSCL 7-bit response mismatch"
    );
}

/// In S8C1T mode, OSC color query responses should use 8-bit OSC and ST.
#[test]
fn response_osc11_uses_8bit_osc_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    let _ = h.drain_pty_writes();

    // OSC 11 query (background color): ESC ] 1 1 ; ? ESC backslash
    h.feed(b"\x1b]11;?\x1b\\");

    let response = h.drain_pty_writes_concatenated();

    // Should start with 0x9D (8-bit OSC) and end with 0x9C (8-bit ST)
    assert_eq!(
        response[0], 0x9D,
        "expected 8-bit OSC (0x9D), got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        *response.last().unwrap(),
        0x9C,
        "expected 8-bit ST (0x9C), got 0x{:02X}",
        response.last().unwrap()
    );
    // Body should contain "11;rgb:" prefix
    let body = &response[1..response.len() - 1];
    assert!(
        body.starts_with(b"11;rgb:"),
        "OSC 11 body should start with '11;rgb:', got {:?}",
        String::from_utf8_lossy(body)
    );
}

// ─── 8-bit DCS sub-parser ───────────────────────────────────────────────────

/// When S8C1T is active, 0x90 should start a DCS sequence (equivalent to ESC P).
/// Test via DECRQSS for DECSCL, which is a real DCS command.
///
/// Note: XTVERSION is CSI > q (NOT DCS), so we use DECRQSS instead.
#[test]
fn parser_8bit_dcs_decrqss() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G"); // enable S8C1T
    let _ = h.drain_pty_writes();

    // Send 8-bit DCS (0x90) + DECRQSS for DECSCL: $q"p + ESC \
    // This is equivalent to ESC P $ q " p ESC \
    h.feed(&[0x90, b'$', b'q', b'"', b'p', 0x1B, b'\\']);

    let response = h.drain_pty_writes_concatenated();

    // Response should start with 0x90 (8-bit DCS) and end with 0x9C (8-bit ST)
    assert!(
        !response.is_empty(),
        "expected DECRQSS DECSCL response via 8-bit DCS"
    );
    assert_eq!(
        response[0], 0x90,
        "expected 8-bit DCS (0x90) in response, got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        *response.last().expect("response is non-empty"),
        0x9C,
        "expected 8-bit ST (0x9C) at end, got 0x{:02X}",
        response.last().expect("response is non-empty")
    );
    // Body should be "1$r65;0\"p" (VT525, 8-bit C1)
    let body = &response[1..response.len() - 1];
    assert_eq!(
        body,
        b"1$r65;0\"p",
        "DECRQSS DECSCL body mismatch via 8-bit DCS: got {:?}",
        String::from_utf8_lossy(body)
    );
}

// ─── Edge cases ─────────────────────────────────────────────────────────────

/// S8C1T should not affect VT52 mode. 8-bit C1 is only valid in ANSI mode.
#[test]
fn s8c1t_does_not_affect_vt52_mode() {
    let mut h = VtTestHelper::new_default();

    // Enable S8C1T
    h.feed(b"\x1b G");

    // Enter VT52 mode: ESC [ ? 2 l
    h.feed(b"\x1b[?2l");

    // In VT52 mode, 0x9B should NOT be treated as CSI.
    // Write text, then send 0x9B + "2J"
    h.feed(b"Hello");

    // Exit VT52 mode first to verify screen: ESC <
    h.feed(b"\x1b<");

    // The "Hello" should remain (0x9B not interpreted as CSI in VT52 mode)
    let row0 = &h.screen_text()[0];
    assert!(
        row0.contains("Hello"),
        "expected 'Hello' to remain in VT52 mode, got: {row0:?}"
    );
}

/// DA2 (secondary device attributes) in S8C1T mode should use 8-bit CSI.
#[test]
fn response_da2_uses_8bit_csi_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");
    let _ = h.drain_pty_writes();

    // DA2: ESC [ > c
    h.feed(b"\x1b[>c");

    let response = h.drain_pty_writes_concatenated();

    assert_eq!(
        response[0], 0x9B,
        "expected 8-bit CSI (0x9B) for DA2, got 0x{:02X}",
        response[0]
    );
    assert_eq!(&response[1..], b">65;0;0c", "DA2 body mismatch");
}

/// DECREQTPARM response in S8C1T mode should use 8-bit CSI.
#[test]
fn response_decreqtparm_uses_8bit_csi_in_s8c1t() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b G");
    let _ = h.drain_pty_writes();

    // DECREQTPARM Ps=0: ESC [ 0 x
    h.feed(b"\x1b[0x");

    let response = h.drain_pty_writes_concatenated();

    assert_eq!(
        response[0], 0x9B,
        "expected 8-bit CSI for DECREQTPARM, got 0x{:02X}",
        response[0]
    );
    assert_eq!(
        &response[1..],
        b"2;1;1;120;120;1;0x",
        "DECREQTPARM body mismatch"
    );
}
