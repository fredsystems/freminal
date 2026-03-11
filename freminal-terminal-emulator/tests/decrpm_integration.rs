// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end DECRPM (DEC Private Mode Report) integration tests.
//!
//! These tests exercise the full pipeline: raw escape bytes → parser →
//! `TerminalState::handle_incoming_data` → DECRPM response on the PTY
//! write channel.  They cover modes owned by `TerminalState` (synced in
//! the mode-sync loop in `internal.rs`), complementing the handler-owned
//! mode query tests in `freminal-buffer/tests/terminal_handler_integration.rs`.

use crossbeam_channel::Receiver;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

/// Create a `TerminalState` and return it along with the PTY write receiver
/// so we can inspect DECRPM responses.
fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Drain all pending `PtyWrite::Write` messages from the channel, concatenate
/// the bytes, and return as a `String`.  Non-`Write` variants are ignored.
fn drain_pty_writes(rx: &Receiver<PtyWrite>) -> String {
    let mut buf = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    String::from_utf8(buf).expect("PTY responses must be valid UTF-8")
}

/// Feed raw bytes and return the concatenated PTY response.
fn feed_and_collect(state: &mut TerminalState, rx: &Receiver<PtyWrite>, input: &[u8]) -> String {
    // Drain any prior writes (e.g. from mode-set producing DA responses)
    let _ = drain_pty_writes(rx);
    state.handle_incoming_data(input);
    drain_pty_writes(rx)
}

// ═══════════════════════════════════════════════════════════════════════════
// DECCKM (?1)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decckm_default_is_reset() {
    let (mut state, rx) = make_state();
    // DECRQM for ?1: ESC[?1$p
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp, "\x1b[?1;2$y", "DECCKM default (Ansi) → Ps=2 (reset)");
}

#[test]
fn decrpm_decckm_after_enable() {
    let (mut state, rx) = make_state();
    // Enable DECCKM: ESC[?1h
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1h");
    // Query
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(
        resp, "\x1b[?1;1$y",
        "DECCKM after enable (Application) → Ps=1 (set)"
    );
}

#[test]
fn decrpm_decckm_enable_then_disable() {
    let (mut state, rx) = make_state();
    // Enable then disable
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1h");
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1l");
    // Query
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp, "\x1b[?1;2$y", "DECCKM after disable → Ps=2 (reset)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Bracketed Paste (?2004)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_bracketed_paste_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp, "\x1b[?2004;2$y",
        "Bracketed paste default → Ps=2 (reset)"
    );
}

#[test]
fn decrpm_bracketed_paste_after_enable() {
    let (mut state, rx) = make_state();
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2004h");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp, "\x1b[?2004;1$y",
        "Bracketed paste after enable → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DECSCNM (?5)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decscnm_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?5$p");
    assert_eq!(
        resp, "\x1b[?5;2$y",
        "DECSCNM default (normal display) → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DECARM (?8)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decarm_default_is_set() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?8$p");
    assert_eq!(
        resp, "\x1b[?8;1$y",
        "DECARM default (repeat keys) → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// XtMseWin / Focus Events (?1004)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_xtmsewin_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1004$p");
    assert_eq!(
        resp, "\x1b[?1004;2$y",
        "XtMseWin default (disabled) → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Reverse Wrap Around (?45)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_reverse_wrap_around_default_is_set() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?45$p");
    assert_eq!(
        resp, "\x1b[?45;1$y",
        "Reverse wrap around default → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Synchronized Updates (?2026)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_synchronized_updates_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2026$p");
    assert_eq!(
        resp, "\x1b[?2026;2$y",
        "Synchronized updates default → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: set mode, query, verify response
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_integration_set_then_query_multiple_modes() {
    let (mut state, rx) = make_state();

    // Enable bracketed paste and DECCKM
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2004h\x1b[?1h");

    // Query both — send as separate sequences
    let resp1 = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp1, "\x1b[?2004;1$y",
        "Bracketed paste must report set after enable"
    );

    let resp2 = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp2, "\x1b[?1;1$y", "DECCKM must report set after enable");
}
