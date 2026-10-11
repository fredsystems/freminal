// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for the Kitty Keyboard Protocol mode stack in
//! `TerminalHandler`.

use freminal_common::buffer_states::modes::kitty_keyboard::KittyKeyboardFlags;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use freminal_terminal_emulator::terminal_handler::TerminalHandler;

/// Create a handler with a write channel and return `(handler, receiver)`.
fn handler_with_pty() -> (TerminalHandler, crossbeam_channel::Receiver<PtyWrite>) {
    let mut handler = TerminalHandler::new(80, 24);
    let (tx, rx) = crossbeam_channel::unbounded();
    handler.set_write_tx(tx);
    (handler, rx)
}

/// Drain all pending `PtyWrite::Write` responses from the receiver and
/// concatenate their byte payloads into a single `Vec<u8>`.
fn drain_pty_bytes(rx: &crossbeam_channel::Receiver<PtyWrite>) -> Vec<u8> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            out.extend_from_slice(&bytes);
        }
    }
    out
}

#[test]
fn push_sets_active_flags() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(3)]);
    assert_eq!(handler.kitty_keyboard_flags(), 3);
}

#[test]
fn push_stack_top_wins() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(1)]);
    handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(2)]);
    assert_eq!(handler.kitty_keyboard_flags(), 2);
}

#[test]
fn pop_restores_previous() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(1),
        TerminalOutput::KittyKeyboardPush(2),
        TerminalOutput::KittyKeyboardPop(1),
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 1);
}

#[test]
fn pop_clears_stack() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(1),
        TerminalOutput::KittyKeyboardPop(1),
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 0);
}

#[test]
fn pop_more_than_stack_does_not_panic() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(1),
        TerminalOutput::KittyKeyboardPop(5),
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 0);
}

#[test]
fn set_replace_updates_top() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(3),
        TerminalOutput::KittyKeyboardSet { flags: 5, mode: 1 },
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 5);
}

#[test]
fn set_or_merges() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(3),
        TerminalOutput::KittyKeyboardSet { flags: 4, mode: 2 },
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 7);
}

#[test]
fn set_and_not_clears_bits() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(7),
        TerminalOutput::KittyKeyboardSet { flags: 2, mode: 3 },
    ]);
    assert_eq!(handler.kitty_keyboard_flags(), 5);
}

#[test]
fn set_on_empty_stack_creates_entry() {
    let mut handler = TerminalHandler::new(80, 24);
    handler.process_outputs(&[TerminalOutput::KittyKeyboardSet { flags: 3, mode: 1 }]);
    assert_eq!(handler.kitty_keyboard_flags(), 3);
}

#[test]
fn query_responds_with_current_flags() {
    let (mut handler, rx) = handler_with_pty();
    handler.process_outputs(&[
        TerminalOutput::KittyKeyboardPush(3),
        TerminalOutput::KittyKeyboardQuery,
    ]);
    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?3u");
}

#[test]
fn query_responds_zero_when_empty() {
    let (mut handler, rx) = handler_with_pty();
    handler.process_outputs(&[TerminalOutput::KittyKeyboardQuery]);
    let response = drain_pty_bytes(&rx);
    assert_eq!(response, b"\x1b[?0u");
}

#[test]
fn max_stack_depth_evicts_oldest() {
    let mut handler = TerminalHandler::new(80, 24);
    for i in 0..=KittyKeyboardFlags::MAX_STACK_DEPTH as u32 {
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(i)]);
    }
    // Stack should be capped at MAX_STACK_DEPTH.  The oldest entry (0) was
    // evicted; the top should be MAX_STACK_DEPTH (the last pushed value).
    assert_eq!(
        handler.kitty_keyboard_flags(),
        KittyKeyboardFlags::MAX_STACK_DEPTH as u32
    );
    // Verify by popping MAX_STACK_DEPTH - 1 times and checking the bottom entry.
    for _ in 0..KittyKeyboardFlags::MAX_STACK_DEPTH - 1 {
        handler.process_outputs(&[TerminalOutput::KittyKeyboardPop(1)]);
    }
    // After popping all but one, the remaining entry should be 1 (the entry
    // after the evicted 0).
    assert_eq!(handler.kitty_keyboard_flags(), 1);
}

#[test]
fn alternate_screen_gets_independent_stack() {
    let mut handler = TerminalHandler::new(80, 24);
    // Push flags on the main screen.
    handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(5)]);
    assert_eq!(handler.kitty_keyboard_flags(), 5);

    // Enter alternate screen -- it has its own (empty) stack.
    feed(&mut handler, b"\x1b[?1049h");
    assert_eq!(handler.kitty_keyboard_flags(), 0);

    // Push flags on the alternate screen.
    handler.process_outputs(&[TerminalOutput::KittyKeyboardPush(3)]);
    assert_eq!(handler.kitty_keyboard_flags(), 3);

    // Leave alternate screen -- the main stack is what it was.
    feed(&mut handler, b"\x1b[?1049l");
    assert_eq!(handler.kitty_keyboard_flags(), 5);

    // The alternate stack is NOT discarded on leave: it persists.
    feed(&mut handler, b"\x1b[?1049h");
    assert_eq!(handler.kitty_keyboard_flags(), 3);
}

/// Feed raw bytes through the parser into the handler.
fn feed(handler: &mut TerminalHandler, bytes: &[u8]) {
    let mut parser = FreminalAnsiParser::default();
    let outputs = parser.push(bytes);
    handler.process_outputs(&outputs);
}

#[test]
fn b15_reentering_alternate_does_not_clobber_main_stack() {
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, b"\x1b[>1u");
    assert_eq!(handler.kitty_keyboard_flags(), 1);
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[>5u");
    // A redundant second entry must not swap anything.
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[?1049l");
    assert_eq!(handler.kitty_keyboard_flags(), 1);
}

#[test]
fn alternate_stack_persists_between_alternate_sessions() {
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[>3u");
    feed(&mut handler, b"\x1b[?1049l");
    feed(&mut handler, b"\x1b[?1049h");
    assert_eq!(handler.kitty_keyboard_flags(), 3);
}

#[test]
fn main_stack_is_unaffected_by_alternate_pushes() {
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, b"\x1b[>2u");
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[>4u\x1b[>8u\x1b[<1u");
    assert_eq!(handler.kitty_keyboard_flags(), 4);
    feed(&mut handler, b"\x1b[?1049l");
    assert_eq!(handler.kitty_keyboard_flags(), 2);
}

#[test]
fn decstr_clears_both_stacks() {
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, b"\x1b[>1u");
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[>2u");
    feed(&mut handler, b"\x1b[!p");
    assert_eq!(handler.kitty_keyboard_flags(), 0);
    feed(&mut handler, b"\x1b[?1049l");
    assert_eq!(handler.kitty_keyboard_flags(), 0);
}

#[test]
fn ris_clears_both_stacks() {
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, b"\x1b[>1u");
    feed(&mut handler, b"\x1b[?1049h");
    feed(&mut handler, b"\x1b[>2u");
    feed(&mut handler, b"\x1bc");
    assert_eq!(handler.kitty_keyboard_flags(), 0);
    feed(&mut handler, b"\x1b[?1049h");
    assert_eq!(handler.kitty_keyboard_flags(), 0);
    feed(&mut handler, b"\x1b[?1049l");
    assert_eq!(handler.kitty_keyboard_flags(), 0);
}
