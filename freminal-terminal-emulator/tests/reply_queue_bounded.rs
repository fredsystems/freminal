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
//!
//! The queue is also bounded in bytes (`MAX_QUEUED_REPLY_BYTES`, 148.C9): the
//! byte-budget tests drive large clipboard replies through
//! `TerminalHandler::write_gui_reply` with a shared queued-bytes counter that
//! stands in for the writer thread.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crossbeam_channel::{bounded, unbounded};
use freminal_terminal_emulator::{
    interface::{MAX_QUEUED_REPLY_BYTES, REPLY_QUEUE_CAPACITY},
    io::GuiReply,
    state::internal::TerminalState,
};

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
            PtyWrite::Write(_) | PtyWrite::ApplyPendingResize => None,
        })
        .expect("DECCOLM must send a PtyWrite::Resize on the control channel");
    assert_eq!(resize, 132);
}

/// Repeated DECCOLM toggles against a child that never reads its tty must not
/// grow the unbounded control channel: with the shared pending-resize slot the
/// resizes coalesce to the latest size and at most two markers are queued.
#[test]
fn deccolm_toggle_flood_is_coalesced_to_the_latest_size() {
    use arc_swap::ArcSwapOption;
    use freminal_common::pty_write::PtyWrite;

    let (reply_tx, _reply_rx) = bounded(REPLY_QUEUE_CAPACITY);
    let (control_tx, control_rx) = unbounded();
    let slot = Arc::new(ArcSwapOption::const_empty());
    let mut state = TerminalState::new(reply_tx, None);
    state.handler.set_control_tx(control_tx);
    state.handler.set_pending_resize(Arc::clone(&slot));

    state.handle_incoming_data(b"\x1b[?40h");
    for _ in 0..FLOOD {
        state.handle_incoming_data(b"\x1b[?3h\x1b[?3l");
    }
    // Finish on a 132-column request so the last width is distinguishable.
    state.handle_incoming_data(b"\x1b[?3h");

    let queued: Vec<PtyWrite> = control_rx.try_iter().collect();
    assert!(
        queued.len() <= 2,
        "expected at most two queued markers, got {}",
        queued.len()
    );
    assert!(
        queued
            .iter()
            .all(|msg| matches!(msg, PtyWrite::ApplyPendingResize)),
        "with a pending-resize slot no direct Resize may be queued"
    );
    let pending = slot.load_full().expect("the latest resize is pending");
    assert_eq!(
        pending.width, 132,
        "the slot holds the last requested width"
    );
}

/// A clipboard reply whose framed length is about `payload_len` bytes.
fn clipboard_reply(payload_len: usize) -> GuiReply {
    GuiReply::Clipboard {
        selection: "c".to_owned(),
        base64_payload: "A".repeat(payload_len),
    }
}

const THREE_MIB: usize = 3 * 1024 * 1024;

/// Two 3 MiB replies exceed the 4 MiB budget: the first is accepted, the second
/// dropped; after the writer releases the first, a new reply is accepted.
#[test]
fn byte_budget_drops_a_reply_past_the_budget_until_the_writer_releases() {
    let (tx, rx) = bounded(REPLY_QUEUE_CAPACITY);
    let queued = Arc::new(AtomicUsize::new(0));
    let mut state = TerminalState::new(tx, None);
    state.handler.set_reply_budget(Arc::clone(&queued));

    state.handler.write_gui_reply(&clipboard_reply(THREE_MIB));
    assert_eq!(rx.len(), 1);
    let first_len = queued.load(Ordering::Relaxed);
    assert!(first_len > THREE_MIB, "the counter holds the framed length");

    state.handler.write_gui_reply(&clipboard_reply(THREE_MIB));
    assert_eq!(rx.len(), 1, "the second 3 MiB reply must be dropped");
    assert_eq!(state.handler.dropped_replies(), 1);
    assert_eq!(
        queued.load(Ordering::Relaxed),
        first_len,
        "a dropped reply must not be counted"
    );

    // Stand in for the writer: take the queued reply and release its bytes.
    let _ = rx.try_recv().expect("first reply queued");
    queued.fetch_sub(first_len, Ordering::Relaxed);

    state.handler.write_gui_reply(&clipboard_reply(THREE_MIB));
    assert_eq!(rx.len(), 1, "accepted again once the bytes were released");
    assert_eq!(state.handler.dropped_replies(), 1);
}

/// One reply larger than the whole budget is still accepted into an empty
/// queue, so a large legitimate clipboard answer is not made impossible.
#[test]
fn byte_budget_admits_a_single_oversized_reply_into_an_empty_queue() {
    let (tx, rx) = bounded(REPLY_QUEUE_CAPACITY);
    let queued = Arc::new(AtomicUsize::new(0));
    let mut state = TerminalState::new(tx, None);
    state.handler.set_reply_budget(Arc::clone(&queued));

    state
        .handler
        .write_gui_reply(&clipboard_reply(2 * THREE_MIB));
    assert_eq!(rx.len(), 1);
    assert!(queued.load(Ordering::Relaxed) > MAX_QUEUED_REPLY_BYTES);
    assert_eq!(state.handler.dropped_replies(), 0);

    // With that reply still queued, even a tiny one is over budget.
    state.handler.write_gui_reply(&clipboard_reply(1));
    assert_eq!(rx.len(), 1);
    assert_eq!(state.handler.dropped_replies(), 1);
}

/// A reply refused by a full message queue gives its bytes back, so the
/// counter cannot drift upward and starve later replies.
#[test]
fn byte_budget_is_refunded_when_the_message_queue_is_full() {
    let (tx, rx) = bounded(REPLY_QUEUE_CAPACITY);
    let queued = Arc::new(AtomicUsize::new(0));
    let mut state = TerminalState::new(tx, None);
    state.handler.set_reply_budget(Arc::clone(&queued));

    state.handle_incoming_data(&b"\x1b[c".repeat(REPLY_QUEUE_CAPACITY + 10));
    assert_eq!(rx.len(), REPLY_QUEUE_CAPACITY);

    // Every queued reply is counted; the ten refused ones are not.
    let total: usize = rx
        .try_iter()
        .map(|w| match w {
            freminal_common::pty_write::PtyWrite::Write(data) => data.len(),
            freminal_common::pty_write::PtyWrite::Resize(_)
            | freminal_common::pty_write::PtyWrite::ApplyPendingResize => 0,
        })
        .sum();
    assert_eq!(queued.load(Ordering::Relaxed), total);
}

/// PR #572 review: a DECCOLM resize and a GUI resize interleaved while the
/// writer is behind must leave the PTY at the emulator's final width. With
/// DECCOLM resizes coalesced through the slot but GUI resizes sent directly,
/// an early marker could apply a later DECCOLM size after a newer GUI resize
/// had been queued behind it. All runtime resizes now share the slot.
#[test]
fn deccolm_and_gui_resizes_stay_ordered_through_the_slot() {
    use arc_swap::ArcSwapOption;
    use freminal_common::pty_write::PtyWrite;
    use freminal_terminal_emulator::interface::TerminalEmulator;

    for gui_width in [100usize, 60] {
        let (mut emulator, rx) = TerminalEmulator::new_headless(None);
        let slot = Arc::new(ArcSwapOption::const_empty());
        emulator
            .internal
            .handler
            .set_pending_resize(Arc::clone(&slot));
        emulator.set_win_size(80, 24, 8, 16).expect("initial size");
        emulator.handle_incoming_data(b"\x1b[?40h");

        // Nothing drains the writer between these three steps.
        emulator.handle_incoming_data(b"\x1b[?3h");
        emulator.handle_resize_event(gui_width, 24, 8, 16);
        emulator.handle_incoming_data(b"\x1b[?3l");

        // Replay the queue as the writer would.
        let mut applied = None;
        for msg in rx.try_iter() {
            match msg {
                PtyWrite::Resize(size) => applied = Some(size.width),
                PtyWrite::ApplyPendingResize => {
                    if let Some(size) = slot.swap(None) {
                        applied = Some(size.width);
                    }
                }
                PtyWrite::Write(_) => {}
            }
        }
        assert_eq!(
            applied,
            Some(emulator.internal.handler.buffer().terminal_width()),
            "PTY width must match the emulator after DECCOLM / GUI {gui_width} / DECCOLM"
        );
    }
}
