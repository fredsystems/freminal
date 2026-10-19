// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A narrowing resize must not strand the cursor past the right edge, and CBT
//! must survive one if it ever happens (Task 148.3, GHSA-8f32-mj37-fw6v).
//!
//! The terminal starts at 100 columns and the fuzz harness resizes it to the
//! target width. DECLRMM (`?69`) was on with its default margins, which stayed
//! at column 99 through the resize, so text written afterwards wrapped (or
//! was clamped) at column 99 instead of the real right edge and left
//! `cursor.x` at 99 on a 28-column buffer. `CSI Z` then sliced the 28-entry
//! tab-stop table at 99 and panicked in release builds.
//!
//! `x == width` is the legitimate pending-wrap position; anything above it is
//! a bug.

use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

/// The minimised `terminal_state` fuzz reproducer (46 bytes): byte 0 is the
/// column seed (`1 + 0x1b % 300` = 28 columns), byte 1 the row seed, byte 2
/// the chunk seed, and the rest is the stream. It is the file committed as
/// `fuzz/regressions/terminal_state/cbt-stale-column-ghsa-8f32`.
const FUZZ_REPRODUCER: &[u8] = &[
    0x1b, 0x75, 0x5b, 0x64, 0x65, 0x1b, 0x5b, 0x3f, 0x36, 0x39, 0x68, 0x5b, 0x3f, 0x32, 0x35, 0x6c,
    0x1b, 0x5b, 0x3f, 0x37, 0x6c, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78, 0x78, 0x6b, 0x34,
    0x31, 0x68, 0x74, 0x74, 0x70, 0x5a, 0x31, 0x38, 0x5a, 0x5a, 0x5a, 0x1b, 0x5b, 0x5a,
];

fn make_state(cols: usize, rows: usize) -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(cols, rows, 8, 16);
    state
}

/// Feed `stream` one byte at a time, asserting after each that the cursor
/// column is within `0..=width`.
fn feed_checked(state: &mut TerminalState, stream: &[u8]) {
    for (i, byte) in stream.iter().enumerate() {
        state.handle_incoming_data(&[*byte]);
        let buffer = state.handler.buffer();
        let x = buffer.cursor().pos.x;
        assert!(
            x <= buffer.terminal_width(),
            "byte {i} ({byte:#04x}): cursor.x {x} past width {}",
            buffer.terminal_width()
        );
    }
}

#[test]
fn minimised_fuzz_reproducer_does_not_panic() {
    let [cols_byte, rows_byte, _seed, stream @ ..] = FUZZ_REPRODUCER else {
        unreachable!("the reproducer has a three byte header");
    };
    let cols = 1 + usize::from(*cols_byte) % 300;
    let rows = 1 + usize::from(*rows_byte) % 120;
    assert_eq!(cols, 28);

    let mut state = make_state(cols, rows);
    feed_checked(&mut state, stream);
}

#[test]
fn declrmm_then_overflow_then_cbt_at_33_columns() {
    // The advisory's size. `?69h` with default margins, autowrap off, more
    // text than a row holds, then CBT.
    let mut state = make_state(33, 24);
    let mut stream = b"\x1b[?69h\x1b[?7l".to_vec();
    stream.extend(std::iter::repeat_n(b'x', 120));
    stream.extend_from_slice(b"\x1b[Z");
    feed_checked(&mut state, &stream);

    let buffer = state.handler.buffer();
    assert!(buffer.cursor().pos.x < 33);
    assert_eq!(buffer.left_right_margins(), (0, 32));
}

#[test]
fn declrmm_then_overflow_then_cbt_with_autowrap_at_33_columns() {
    let mut state = make_state(33, 24);
    let mut stream = b"\x1b[?69h".to_vec();
    stream.extend(std::iter::repeat_n(b'x', 300));
    stream.extend_from_slice(b"\x1b[3Z");
    feed_checked(&mut state, &stream);
}

#[test]
fn cbt_after_every_overflow_width_is_in_bounds() {
    for cols in [1, 2, 7, 8, 9, 33, 80] {
        let mut state = make_state(cols, 5);
        let mut stream = b"\x1b[?69h\x1b[?7l".to_vec();
        stream.extend(std::iter::repeat_n(b'y', 150));
        stream.extend_from_slice(b"\x1b[Z\x1b[I\x1b[Z");
        feed_checked(&mut state, &stream);
    }
}
