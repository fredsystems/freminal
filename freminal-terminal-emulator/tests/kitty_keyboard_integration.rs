// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end integration tests for the Kitty Keyboard Protocol.
//!
//! These tests feed raw PTY bytes through a [`TerminalState`] and assert on
//! the bytes the PTY receives back, following the pattern in
//! `terminal_state_tests.rs`.

#![allow(clippy::unwrap_used)]

use crossbeam_channel::unbounded;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

// ─── helpers ────────────────────────────────────────────────────────────────

/// Create a fresh [`TerminalState`] with a PTY write channel.
fn make_state() -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
    let (tx, rx) = unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Drain all pending messages and discard them (clears startup noise).
fn drain(rx: &crossbeam_channel::Receiver<PtyWrite>) {
    while rx.try_recv().is_ok() {}
}

/// Drain all pending `PtyWrite::Write` messages and concatenate the bytes.
fn drain_pty_bytes(rx: &crossbeam_channel::Receiver<PtyWrite>) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            out.extend_from_slice(&bytes);
        }
    }
    out
}

/// Feed raw bytes through the terminal state as if they arrived from the PTY.
fn feed(state: &mut TerminalState, data: &[u8]) {
    state.handle_incoming_data(data);
}

// ─── tests ──────────────────────────────────────────────────────────────────

/// Query without any push should respond `ESC[?0u`.
#[test]
fn query_without_push_responds_0u() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // CSI ? u — query current keyboard flags
    feed(&mut state, b"\x1b[?u");

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?0u", "empty stack should report flags=0");
}

/// Push flags=3, then query → should respond `ESC[?3u`.
#[test]
fn push_then_query_responds_with_flags() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // CSI > 3 u — push flags=3
    feed(&mut state, b"\x1b[>3u");
    // CSI ? u — query
    feed(&mut state, b"\x1b[?u");

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?3u", "stack top should be 3");
}

/// Push 3, push 5, pop 1, query → should respond `ESC[?3u`.
#[test]
fn push_push_pop_query() {
    let (mut state, rx) = make_state();
    drain(&rx);

    feed(&mut state, b"\x1b[>3u"); // push 3
    feed(&mut state, b"\x1b[>5u"); // push 5
    feed(&mut state, b"\x1b[<1u"); // pop 1
    feed(&mut state, b"\x1b[?u"); // query

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?3u", "after pop, stack top should be 3");
}

/// Pop on empty stack should not panic and query should return `ESC[?0u`.
#[test]
fn pop_empty_stack_does_not_panic() {
    let (mut state, rx) = make_state();
    drain(&rx);

    feed(&mut state, b"\x1b[<1u"); // pop on empty — should be safe
    feed(&mut state, b"\x1b[?u"); // query

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?0u", "empty stack should still report 0");
}

/// `CSI = 7 u` (set replace) on empty stack should create an entry.
#[test]
fn set_replace_on_empty_stack_creates_entry() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // CSI = 7 u — set flags=7, mode=1 (replace, default)
    feed(&mut state, b"\x1b[=7u");
    feed(&mut state, b"\x1b[?u");

    let response = drain_pty_bytes(&rx);
    assert_eq!(
        response, b"\x1b[?7u",
        "set-replace on empty should create entry with flags=7"
    );
}

/// Push 1, then `CSI = 4 ; 2 u` (set OR) → query should respond `ESC[?5u` (1|4=5).
#[test]
fn set_or_on_existing_flags() {
    let (mut state, rx) = make_state();
    drain(&rx);

    feed(&mut state, b"\x1b[>1u"); // push 1
    feed(&mut state, b"\x1b[=4;2u"); // set flags=4, mode=2 (OR)
    feed(&mut state, b"\x1b[?u"); // query

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?5u", "1 OR 4 = 5");
}

/// Push 7, then `CSI = 2 ; 3 u` (set AND-NOT) → query should respond `ESC[?5u` (7 & !2 = 5).
#[test]
fn set_and_not_clears_bits() {
    let (mut state, rx) = make_state();
    drain(&rx);

    feed(&mut state, b"\x1b[>7u"); // push 7
    feed(&mut state, b"\x1b[=2;3u"); // set flags=2, mode=3 (AND-NOT)
    feed(&mut state, b"\x1b[?u"); // query

    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?5u", "7 AND-NOT 2 = 5");
}

/// Pushing 257 times should not exceed MAX_STACK_DEPTH (256); query should
/// return the last pushed value.
#[test]
fn push_exceeds_max_depth_evicts_oldest() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Push 257 entries (0..=256).  Each push is a separate feed.
    for i in 0..=256u32 {
        let seq = format!("\x1b[>{i}u");
        feed(&mut state, seq.as_bytes());
    }
    feed(&mut state, b"\x1b[?u");

    let response = drain_pty_bytes(&rx);
    // The last pushed value is 256.
    assert_eq!(
        response, b"\x1b[?256u",
        "stack top should be the last pushed value (256)"
    );
}
