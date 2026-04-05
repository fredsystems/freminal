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

// ═══════════════════════════════════════════════════════════════════════════════
// Phase B tests — byte-exact sequences derived from vttest-20251205/reports.c
// ═══════════════════════════════════════════════════════════════════════════════

// ─── DA3 — Tertiary Device Attributes ────────────────────────────────────────
//
// From vttest reports.c tst_DA_3():
//   do_csi("=c");   →   CSI = c   →   \x1b[=c
// vttest accepts: DCS ! | <8 hex digits> ST
//   i.e. skip_dcs(report) yields "!|<hex8>" and strip_terminator succeeds.
// Freminal responds: DCS ! | 00000000 ST  =  \x1bP!|00000000\x1b\

#[test]
fn da3_query_standard() {
    // CSI = c  (intermediates = ['='], no param)
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[=c");

    let response = h.drain_pty_writes_concatenated();
    // DCS ! | 00000000 ST
    assert_eq!(
        response, b"\x1bP!|00000000\x1b\\",
        "DA3 must respond with DCS !|<8 hex digits> ST"
    );
}

#[test]
fn da3_query_explicit_zero_param() {
    // CSI = 0 c — same as CSI = c
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    // The '=' can also appear as a param prefix (older programs).
    h.feed(b"\x1b[=0c");

    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x1bP!|00000000\x1b\\",
        "DA3 with explicit Ps=0 must produce the same response"
    );
}

#[test]
fn da3_response_is_valid_dcs_unit_id() {
    // Verify the response matches vttest's structural check:
    //   strip_dcs → starts with "!|" → 8 hex chars follow.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[=c");

    let response = h.drain_pty_writes_concatenated();
    // Must start with DCS introducer ESC P
    assert!(
        response.starts_with(b"\x1bP"),
        "DA3 response must start with DCS ESC P"
    );
    // Must end with ST = ESC \
    assert!(
        response.ends_with(b"\x1b\\"),
        "DA3 response must end with ST (ESC \\)"
    );
    // Inner content (between \x1bP and \x1b\) must be "!|" + 8 hex chars
    let inner = &response[2..response.len() - 2];
    assert!(
        inner.starts_with(b"!|"),
        "DA3 inner content must start with '!|', got: {inner:?}"
    );
    let hex_part = &inner[2..];
    assert_eq!(hex_part.len(), 8, "DA3 unit ID must be exactly 8 hex chars");
    assert!(
        hex_part.iter().all(|b| b.is_ascii_hexdigit()),
        "DA3 unit ID must be 8 valid hex digits, got: {hex_part:?}"
    );
}

// ─── DECREQTPARM — Request Terminal Parameters ───────────────────────────────
//
// From vttest reports.c tst_DECREQTPARM():
//   decreqtparm(0) → brc(0, 'x') → do_csi("0x") → CSI 0 x
//   decreqtparm(1) → brc(1, 'x') → do_csi("1x") → CSI 1 x
//
// vttest validation for Ps=0 response:
//   - After skip_csi: must start with "2;"  (strlen >= 14)
//   - Parse: "2;<parity>;<nbits>;<xspeed>;<rspeed>;<clkmul>;<flags>x"
//   - parity > 0, nbits > 0, clkmul > 0
//
// vttest validation for Ps=1 response:
//   - After skip_csi: must start with "3;"
//   - Replace leading '3' with '2'; result must equal the Ps=0 response body

#[test]
fn decreqtparm_ps0_responds_with_code_2() {
    // CSI 0 x — vttest sends this via decreqtparm(0).
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[0x");

    let response = h.drain_pty_writes_concatenated();
    // Must start with ESC [ 2 ;
    assert!(
        response.starts_with(b"\x1b[2;"),
        "DECREQTPARM Ps=0 must respond CSI 2;<params>x, got: {:?}",
        String::from_utf8_lossy(&response)
    );
    // Must end with 'x'
    assert_eq!(
        response.last().copied(),
        Some(b'x'),
        "DECREQTPARM response must end with 'x'"
    );
}

#[test]
fn decreqtparm_ps0_response_is_valid_format() {
    // Full vttest structural validation for Ps=0:
    //   After skip_csi → "2;<parity>;<nbits>;<xspeed>;<rspeed>;<clkmul>;<flags>x"
    //   parity > 0, nbits > 0, clkmul > 0, length >= 14 chars (after CSI).
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[0x");

    let response = h.drain_pty_writes_concatenated();

    // Strip "ESC [" prefix
    assert!(response.starts_with(b"\x1b["), "Must start with CSI");
    let after_csi = &response[2..];

    let s = std::str::from_utf8(after_csi).expect("Response must be ASCII");
    // vttest checks strlen(report) >= 14 *after* skip_csi
    assert!(
        s.len() >= 14,
        "DECREQTPARM response body must be >= 14 chars, got: {s:?}"
    );
    // Must start with "2;"
    assert!(
        s.starts_with("2;"),
        "Ps=0 response must start with '2;', got: {s:?}"
    );
    // Must end with 'x'
    assert!(
        s.ends_with('x'),
        "Ps=0 response must end with 'x', got: {s:?}"
    );

    // Parse fields: 2;<parity>;<nbits>;<xspeed>;<rspeed>;<clkmul>;<flags>x
    let body = s.trim_end_matches('x');
    let parts: Vec<u32> = body
        .split(';')
        .map(|p| p.parse::<u32>().expect("All fields must be integers"))
        .collect();
    assert_eq!(
        parts.len(),
        7,
        "Must have 7 semicolon-separated fields, got: {parts:?}"
    );
    let (parity, nbits, _xspeed, _rspeed, clkmul, _flags) =
        (parts[1], parts[2], parts[3], parts[4], parts[5], parts[6]);
    assert!(
        parity > 0,
        "Parity field must be > 0 (vttest requirement), got: {parity}"
    );
    assert!(
        nbits > 0,
        "Nbits field must be > 0 (vttest requirement), got: {nbits}"
    );
    assert!(
        clkmul > 0,
        "Clkmul field must be > 0 (vttest requirement), got: {clkmul}"
    );
}

#[test]
fn decreqtparm_ps1_responds_with_code_3() {
    // CSI 1 x — vttest sends this via decreqtparm(1); expects "3;" prefix.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[1x");

    let response = h.drain_pty_writes_concatenated();
    assert!(
        response.starts_with(b"\x1b[3;"),
        "DECREQTPARM Ps=1 must respond CSI 3;<params>x, got: {:?}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(response.last().copied(), Some(b'x'), "Must end with 'x'");
}

#[test]
fn decreqtparm_ps0_and_ps1_bodies_match() {
    // vttest validation: replace leading '3' with '2' in Ps=1 response;
    // result must equal the Ps=0 response body.
    let mut h0 = VtTestHelper::new_default();
    let _ = h0.drain_pty_writes();
    h0.feed(b"\x1b[0x");
    let resp0 = h0.drain_pty_writes_concatenated();

    let mut h1 = VtTestHelper::new_default();
    let _ = h1.drain_pty_writes();
    h1.feed(b"\x1b[1x");
    let resp1 = h1.drain_pty_writes_concatenated();

    // Strip CSI prefix from both
    let body0 = &resp0[2..]; // "2;<params>x"
    let mut body1 = resp1[2..].to_vec(); // "3;<params>x"
    // Replace leading '3' with '2' to match vttest logic
    if body1.first() == Some(&b'3') {
        body1[0] = b'2';
    }
    assert_eq!(
        body0,
        body1.as_slice(),
        "DECREQTPARM Ps=0 and Ps=1 must have identical parameter bodies"
    );
}

#[test]
fn decreqtparm_no_param_treated_as_ps0() {
    // CSI x (no param) — should default to Ps=0 → "2;" prefix.
    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    h.feed(b"\x1b[x");

    let response = h.drain_pty_writes_concatenated();
    assert!(
        response.starts_with(b"\x1b[2;"),
        "DECREQTPARM with no param must default to Ps=0 (code 2), got: {:?}",
        String::from_utf8_lossy(&response)
    );
}

// ─── DA1 extension code audit (vttest extensions[] table cross-reference) ────
//
// Freminal responds to DA1 with CSI ? 65 ; 1 ; 2 ; 4 ; 6 ; 17 ; 18 ; 22 c.
// From vttest reports.c extensions[] table, these codes mean:
//   1  = "132 columns"
//   2  = "printer port"
//   4  = "Sixel graphics"
//   6  = "selective erase"
//   17 = "terminal state reports"
//   18 = "user windows"
//   22 = "color"
// All are listed in the extensions[] table so vttest will not print "BAD VALUE".
// This test documents the expected response and validates each extension code
// is known to vttest.

#[test]
fn da1_response_extension_codes_are_vttest_known() {
    // Parse Freminal's DA1 response and verify every extension code is in the
    // vttest-recognized set (extensions[] table from reports.c).
    let vttest_known: std::collections::HashSet<u32> = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
        26, 27, 28, 29, 30, 32, 33, 34, 35, 36, 37, 38, 42, 44, 45, 46,
    ]
    .iter()
    .copied()
    .collect();

    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();
    h.feed(b"\x1b[c");
    let response = h.drain_pty_writes_concatenated();

    // Response: ESC [ ? 65 ; 1 ; 2 ; ... c
    let s = std::str::from_utf8(&response).expect("DA1 response must be ASCII");
    // Strip "ESC[?" prefix and trailing "c"
    let body = s
        .strip_prefix("\x1b[?")
        .expect("Must start with CSI ?")
        .strip_suffix('c')
        .expect("Must end with 'c'");
    let params: Vec<u32> = body
        .split(';')
        .map(|p| p.parse::<u32>().expect("Must be integers"))
        .collect();
    // First param is the terminal level (65 = VT500 family) — not an extension code.
    // Remaining params are extension codes.
    let extensions = &params[1..];
    for &code in extensions {
        assert!(
            vttest_known.contains(&code),
            "DA1 extension code {code} is not in vttest's known extensions[] table"
        );
    }
}

// ─── LNM (Line Feed / New Line Mode) — vttest Menu 6.2 ─────────────────────

/// vttest 6.2 (`tst_NLM`): CSI 20 h enables LNM; CSI 20 l disables it.
///
/// When LNM is set, the terminal must send CR+LF for Enter. When reset
/// (default), the terminal must send bare CR. This test verifies the mode
/// set/reset via the `write()` method which calls `to_payload()` internally.
#[test]
fn lnm_mode_set_and_reset_via_write() {
    use freminal_terminal_emulator::input::TerminalInput;

    let mut h = VtTestHelper::new_default();
    let _ = h.drain_pty_writes();

    // Default state: LNM reset. Enter should produce bare CR.
    h.write_terminal_input(&TerminalInput::Enter);
    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x0d",
        "Default LNM (reset): Enter must send bare CR"
    );

    // Set LNM: CSI 20 h
    h.feed(b"\x1b[20h");
    let _ = h.drain_pty_writes();

    // Now Enter should produce CR+LF.
    h.write_terminal_input(&TerminalInput::Enter);
    let response = h.drain_pty_writes_concatenated();
    assert_eq!(response, b"\x0d\x0a", "LNM set: Enter must send CR+LF");

    // Reset LNM: CSI 20 l
    h.feed(b"\x1b[20l");
    let _ = h.drain_pty_writes();

    // Enter should again produce bare CR.
    h.write_terminal_input(&TerminalInput::Enter);
    let response = h.drain_pty_writes_concatenated();
    assert_eq!(
        response, b"\x0d",
        "LNM reset: Enter must send bare CR again"
    );
}
