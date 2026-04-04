// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 6 — Device Report Tests.
//!
//! These tests exercise the full parse → handle → PTY-write-back pipeline for
//! device report queries. Each test feeds a query escape sequence and asserts
//! the exact response bytes sent back to the PTY channel.
//!
//! Covered:
//! - DSR Ps=5 (device status report — "terminal OK")
//! - DSR Ps=6 (cursor position report)
//! - DA1 (primary device attributes)
//! - DA2 (secondary device attributes)
//! - DECRQM for a selection of implemented DEC private modes

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── DSR Ps=5 — Device Status Report ────────────────────────────────────────

#[test]
fn dsr_status_responds_terminal_ok() {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes(); // Clear any startup messages.

    h.feed(b"\x1b[5n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[0n",
        "DSR Ps=5 must respond CSI 0 n (terminal OK)"
    );
}

#[test]
fn dsr_status_dec_private_variant() {
    // CSI ? 5 n is also accepted as a status query.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[?5n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(response, b"\x1b[0n", "DSR ? Ps=5 must also respond CSI 0 n");
}

#[test]
fn dsr_default_param_is_status() {
    // CSI n with no parameter defaults to Ps=5.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[0n",
        "DSR with no Ps (default=5) must respond CSI 0 n"
    );
}

// ─── DSR Ps=6 — Cursor Position Report ──────────────────────────────────────

#[test]
fn cpr_at_origin() {
    // Cursor at (0, 0) screen → CPR responds with (1, 1) (1-indexed).
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[1;1R",
        "CPR at origin must respond CSI 1 ; 1 R"
    );
}

#[test]
fn cpr_after_cursor_move() {
    // Move cursor to row 5, col 10 (1-indexed), then query.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[5;10H"); // CUP to row 5, col 10
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[5;10R",
        "CPR after CUP(5,10) must respond CSI 5 ; 10 R"
    );
}

#[test]
fn cpr_after_text_output() {
    // Write "Hello" (5 chars) to row 1, then query.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"Hello");
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[1;6R",
        "CPR after 'Hello' must report row=1, col=6 (1-indexed)"
    );
}

#[test]
fn cpr_at_bottom_right() {
    // Move cursor to last row, last column of 80x24 terminal.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[24;80H"); // CUP to row 24, col 80
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[24;80R",
        "CPR at bottom-right corner must respond CSI 24 ; 80 R"
    );
}

#[test]
fn cpr_with_origin_mode_and_scroll_region() {
    // Set a scroll region (rows 5-20), enable DECOM, then query cursor position.
    // With DECOM active, CPR reports position relative to the scroll region origin.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    // Set scroll region to rows 5-20 (1-indexed).
    h.feed(b"\x1b[5;20r");
    // Enable origin mode.
    h.feed(b"\x1b[?6h");
    // Move cursor to relative position (3, 5) within the region.
    h.feed(b"\x1b[3;5H");
    h.feed(b"\x1b[6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[3;5R",
        "CPR with DECOM must report position relative to scroll region"
    );
}

#[test]
fn cpr_dec_private_variant() {
    // CSI ? 6 n should also produce a cursor position report.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[3;7H"); // CUP to row 3, col 7
    h.feed(b"\x1b[?6n");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[3;7R",
        "DEC private CPR must also report cursor position"
    );
}

// ─── DA1 — Primary Device Attributes ────────────────────────────────────────

#[test]
fn da1_standard_query() {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[c");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[?65;1;2;4;6;17;18;22c",
        "DA1 must respond with Freminal's device attributes"
    );
}

#[test]
fn da1_explicit_zero_param() {
    // CSI 0 c is equivalent to CSI c.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[0c");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[?65;1;2;4;6;17;18;22c",
        "DA1 with Ps=0 must produce the same response as default"
    );
}

// ─── DA2 — Secondary Device Attributes ──────────────────────────────────────

#[test]
fn da2_query() {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[>c");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[>65;0;0c",
        "DA2 must respond with secondary device attributes"
    );
}

#[test]
fn da2_explicit_zero_param() {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[>0c");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1b[>65;0;0c",
        "DA2 with Ps=0 must produce the same response"
    );
}

// ─── DECRQM — Request Mode ──────────────────────────────────────────────────

// Helper: send DECRQM for a DEC private mode and return the response.
fn decrqm_dec(mode_num: u32) -> Vec<u8> {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    let query = format!("\x1b[?{mode_num}$p");
    h.feed(query.as_bytes());

    h.drain_pty_writes_concatenated()
}

// Helper: send DECRQM for a standard ANSI mode (no `?` prefix) and return the response.
fn decrqm_ansi(mode_num: u32) -> Vec<u8> {
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    let query = format!("\x1b[{mode_num}$p");
    h.feed(query.as_bytes());

    h.drain_pty_writes_concatenated()
}

// Helper: format the expected DECRPM response for a DEC private mode.
fn decrpm_dec(mode_num: u32, status: u8) -> Vec<u8> {
    format!("\x1b[?{mode_num};{status}$y").into_bytes()
}

#[test]
fn decrqm_decckm_default_reset() {
    // ?1 DECCKM — default is reset (cursor keys send ANSI sequences).
    assert_eq!(
        decrqm_dec(1),
        decrpm_dec(1, 2),
        "DECCKM default must be reset (status=2)"
    );
}

#[test]
fn decrqm_decawm_default_set() {
    // ?7 DECAWM — default is set (auto-wrap enabled).
    assert_eq!(
        decrqm_dec(7),
        decrpm_dec(7, 1),
        "DECAWM default must be set (status=1)"
    );
}

#[test]
fn decrqm_dectcem_default_set() {
    // ?25 DECTCEM — default is set (cursor visible).
    assert_eq!(
        decrqm_dec(25),
        decrpm_dec(25, 1),
        "DECTCEM default must be set (status=1)"
    );
}

#[test]
fn decrqm_decom_default_reset() {
    // ?6 DECOM — default is reset (absolute cursor addressing).
    assert_eq!(
        decrqm_dec(6),
        decrpm_dec(6, 2),
        "DECOM default must be reset (status=2)"
    );
}

#[test]
fn decrqm_decscnm_default_reset() {
    // ?5 DECSCNM — default is reset (normal video, not inverse).
    assert_eq!(
        decrqm_dec(5),
        decrpm_dec(5, 2),
        "DECSCNM default must be reset (status=2)"
    );
}

#[test]
fn decrqm_bracketed_paste_default_reset() {
    // ?2004 Bracketed Paste — default is reset.
    assert_eq!(
        decrqm_dec(2004),
        decrpm_dec(2004, 2),
        "Bracketed paste default must be reset (status=2)"
    );
}

#[test]
fn decrqm_after_mode_set() {
    // Set DECCKM (?1), then query — should report set (status=1).
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[?1h"); // Set DECCKM
    h.feed(b"\x1b[?1$p"); // Query DECCKM

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response,
        decrpm_dec(1, 1),
        "DECCKM after set must report status=1"
    );
}

#[test]
fn decrqm_after_mode_reset() {
    // Set DECAWM, then reset, then query.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[?7l"); // Reset DECAWM
    h.feed(b"\x1b[?7$p"); // Query DECAWM

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response,
        decrpm_dec(7, 2),
        "DECAWM after reset must report status=2"
    );
}

#[test]
fn decrqm_unknown_mode_returns_not_recognized() {
    // An unknown mode number must return status=0 (not recognized).
    assert_eq!(
        decrqm_dec(9999),
        decrpm_dec(9999, 0),
        "Unknown DEC mode 9999 must return status=0"
    );
}

#[test]
fn decrqm_grapheme_clustering_permanently_set() {
    // ?2027 — grapheme clustering is permanently set (status=3).
    assert_eq!(
        decrqm_dec(2027),
        decrpm_dec(2027, 3),
        "Grapheme clustering (?2027) must return status=3 (permanently set)"
    );
}

#[test]
fn decrqm_lnm_default_reset() {
    // LNM (mode 20) is a standard ANSI mode, not a DEC private mode.
    // It must be queried via ANSI DECRQM (CSI 20 $p, no `?` prefix).
    // The response still uses the `?` prefix per DEC convention.
    assert_eq!(
        decrqm_ansi(20),
        decrpm_dec(20, 2),
        "LNM default must be reset (status=2)"
    );
}

#[test]
fn decrqm_decarm_default_set() {
    // ?8 DECARM — default is set (auto-repeat keys).
    assert_eq!(
        decrqm_dec(8),
        decrpm_dec(8, 1),
        "DECARM default must be set (status=1)"
    );
}
