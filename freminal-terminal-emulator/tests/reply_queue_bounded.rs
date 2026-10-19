// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A child that does not read its tty must not make terminal replies queue
//! without bound (GHSA-6cxq-f7hx-92v2).
//!
//! The real PTY path gives the handler a bounded sender
//! (`REPLY_QUEUE_CAPACITY`); these tests bind a `TerminalState` to such a
//! sender with nobody reading the other end, then flood it with queries.

use crossbeam_channel::{bounded, unbounded};
use freminal_terminal_emulator::{interface::REPLY_QUEUE_CAPACITY, state::internal::TerminalState};

const FLOOD: usize = 10_000;

#[test]
fn da1_flood_with_no_reader_stays_bounded_and_completes() {
    let (tx, rx) = bounded(REPLY_QUEUE_CAPACITY);
    let mut state = TerminalState::new(tx, None);

    state.handle_incoming_data(&b"\x1b[c".repeat(FLOOD));

    assert!(
        rx.len() <= REPLY_QUEUE_CAPACITY,
        "reply queue holds {} messages, capacity is {REPLY_QUEUE_CAPACITY}",
        rx.len()
    );
    assert_eq!(rx.len(), REPLY_QUEUE_CAPACITY);
    assert!(
        state.handler.dropped_replies() > 0,
        "an unread bounded queue must drop replies"
    );
    assert_eq!(
        state.handler.dropped_replies(),
        u64::try_from(FLOOD - REPLY_QUEUE_CAPACITY).expect("fits in u64"),
    );
}

#[test]
fn xtgettcap_flood_with_no_reader_stays_bounded() {
    let (tx, rx) = bounded(REPLY_QUEUE_CAPACITY);
    let mut state = TerminalState::new(tx, None);

    // Each request asks for the cap's worth of names; 100 requests would queue
    // 6400 replies without a bounded queue.
    let names = vec!["524742"; 64].join(";");
    let request = format!("\x1bP+q{names}\x1b\\");
    state.handle_incoming_data(request.repeat(100).as_bytes());

    assert_eq!(rx.len(), REPLY_QUEUE_CAPACITY);
    assert!(state.handler.dropped_replies() > 0);
}

#[test]
fn unbounded_sender_never_drops() {
    let (tx, rx) = unbounded();
    let mut state = TerminalState::new(tx, None);

    state.handle_incoming_data(&b"\x1b[c".repeat(1000));

    assert_eq!(rx.len(), 1000);
    assert_eq!(state.handler.dropped_replies(), 0);
}

/// A DECCOLM resize is not a reply: with the reply queue full and unread, it
/// must still go out (on the control channel) without blocking the PTY thread.
/// Before the control channel existed it used the reply sender's blocking
/// `send`, which would park this test forever on a full bounded queue.
#[test]
fn deccolm_resize_bypasses_a_full_reply_queue() {
    use freminal_common::pty_write::PtyWrite;

    let (reply_tx, reply_rx) = bounded(REPLY_QUEUE_CAPACITY);
    let (control_tx, control_rx) = unbounded();
    let mut state = TerminalState::new(reply_tx, None);
    state.handler.set_control_tx(control_tx);

    // Fill the reply queue, enable DECCOLM switching (?40h), then switch to
    // 132 columns (?3h).
    state.handle_incoming_data(&b"\x1b[c".repeat(REPLY_QUEUE_CAPACITY + 10));
    assert_eq!(reply_rx.len(), REPLY_QUEUE_CAPACITY);
    state.handle_incoming_data(b"\x1b[?40h\x1b[?3h");

    let resize = control_rx
        .try_iter()
        .find_map(|w| match w {
            PtyWrite::Resize(size) => Some(size.width),
            PtyWrite::Write(_) => None,
        })
        .expect("DECCOLM must send a PtyWrite::Resize on the control channel");
    assert_eq!(resize, 132);
}
