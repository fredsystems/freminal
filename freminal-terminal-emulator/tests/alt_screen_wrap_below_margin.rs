// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The alternate screen keeps exactly `height` rows when text auto-wraps below
//! the bottom margin on its last row (Task 148.7, issue #568).
//!
//! A DECSTBM region that stops above the last screen row leaves the rows under
//! it outside the region. Wrapping on the last of them used to append a row to
//! the alternate screen, which then tripped four buffer invariants: the
//! alternate row count, the cursor row, the parked alternate store (`?1049l`)
//! and, after the next `?1049h` cleared the screen, the cursor row again. See
//! `freminal-buffer/src/buffer/alt_wrap_below_margin_tests.rs` for the
//! buffer-level cases and the xterm source this follows.

use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

/// The minimised `terminal_state` fuzz reproducer (57 bytes): byte 0 is the
/// column seed (`1 + 0x20 % 300` = 33 columns), byte 1 the row seed
/// (`1 + 0xff % 120` = 16 rows), byte 2 the chunk seed, and the rest is the
/// stream. It is the file committed as
/// `fuzz/regressions/terminal_state/alt-wrap-below-margin-grows-screen`.
const FUZZ_REPRODUCER: &[u8] = &[
    0x20, 0xff, 0x04, 0x1b, 0x5b, 0x3b, 0x31, 0x30, 0x72, 0x1b, 0x5b, 0x33, 0x38, 0x66, 0x69, 0x3b,
    0x78, 0x78, 0x78, 0x75, 0x6e, 0x72, 0x6c, 0x6f, 0x72, 0x20, 0x66, 0x30, 0x6d, 0x63, 0x40, 0x1b,
    0x5b, 0x3f, 0x31, 0x30, 0x34, 0x39, 0x68, 0x78, 0x75, 0x69, 0x6c, 0x74, 0x6c, 0x79, 0x20, 0x72,
    0x61, 0x69, 0x65, 0x64, 0x20, 0x75, 0x6e, 0x30, 0x0a,
];

fn make_state(cols: usize, rows: usize) -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(cols, rows, 8, 16);
    state
}

/// Feed `stream` one byte at a time. After each, the cursor must be on an
/// existing row, and the alternate screen (when it is the active one) must
/// hold exactly `height` rows.
fn feed_checked(state: &mut TerminalState, stream: &[u8]) {
    for (i, byte) in stream.iter().enumerate() {
        state.handle_incoming_data(&[*byte]);
        let buffer = state.handler.buffer();
        assert!(
            buffer.cursor().pos.y < buffer.rows().len(),
            "byte {i} ({byte:#04x}): cursor.y {} past {} rows",
            buffer.cursor().pos.y,
            buffer.rows().len()
        );
        if buffer.is_alternate_screen() {
            assert_eq!(
                buffer.rows().len(),
                buffer.terminal_height(),
                "byte {i} ({byte:#04x}): alternate screen row count"
            );
        }
    }
}

#[test]
fn minimised_fuzz_reproducer_does_not_panic() {
    let [cols_byte, rows_byte, _seed, stream @ ..] = FUZZ_REPRODUCER else {
        unreachable!("the reproducer has a three byte header");
    };
    let cols = 1 + usize::from(*cols_byte) % 300;
    let rows = 1 + usize::from(*rows_byte) % 120;
    assert_eq!((cols, rows), (33, 16));

    let mut state = make_state(cols, rows);
    feed_checked(&mut state, stream);
}

#[test]
fn wrap_below_the_margin_then_leave_and_reenter_the_alternate_screen() {
    // Region on rows 1-3, cursor on the last row (8), 25 characters into a
    // 10-column screen: two wraps, both on the last row. Leaving parks the
    // alternate screen, and the second `?1049h` installs it and clears it.
    let mut state = make_state(10, 8);
    let mut stream = b"\x1b[?1049h\x1b[1;3r\x1b[8;1H".to_vec();
    stream.extend(std::iter::repeat_n(b'x', 25));
    stream.extend_from_slice(b"\x1b[?1049l\x1b[?1049h");
    feed_checked(&mut state, &stream);

    let buffer = state.handler.buffer();
    assert!(buffer.is_alternate_screen());
    assert_eq!(buffer.rows().len(), 8);
}

#[test]
fn wrap_below_the_margin_on_the_last_row_overwrites_it() {
    // 13 characters on a 10-column screen from the left edge of the last row:
    // the text wraps once and its tail overwrites the start of the same row.
    let mut state = make_state(10, 8);
    let mut stream = b"\x1b[?1049h\x1b[1;3r\x1b[8;1H".to_vec();
    stream.extend_from_slice(b"abcdefghijKLM");
    feed_checked(&mut state, &stream);

    let buffer = state.handler.buffer();
    assert_eq!(buffer.rows().len(), 8);
    assert_eq!(buffer.cursor().pos.y, 7);
    let last: String = buffer.rows()[7]
        .cells()
        .iter()
        .map(freminal_buffer::cell::Cell::into_utf8)
        .collect();
    assert_eq!(last.trim_end(), "KLMdefghij");
}
