// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! LF below the bottom margin on the primary screen's last row does not
//! scroll (Task 148.C3).
//!
//! xterm's `xtermIndex` (cursor.c) treats indexing below the scrolling region
//! as cursor down, and `CursorDown` clamps at the last screen row, so nothing
//! scrolls and nothing is pushed into scrollback. See
//! `freminal-buffer/src/buffer/primary_wrap_below_margin_tests.rs` for the
//! buffer-level cases.

use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

fn make_state(cols: usize, rows: usize) -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(cols, rows, 8, 16);
    state
}

#[test]
fn lf_below_the_margin_on_the_last_row_does_not_scroll() {
    let mut state = make_state(10, 5);
    // Fill the screen so the rows exist, then set a 3-row region and park the
    // cursor on the last row.
    state.handle_incoming_data(b"r0\r\nr1\r\nr2\r\nr3\r\nr4");
    state.handle_incoming_data(b"\x1b[1;3r\x1b[5;1H");
    let rows_before = state.handler.buffer().rows().len();
    let y_before = state.handler.buffer().cursor().pos.y;
    assert_eq!(rows_before, 5, "no scrollback yet");

    state.handle_incoming_data(b"\n\n\n");

    let buffer = state.handler.buffer();
    assert_eq!(buffer.rows().len(), rows_before, "nothing scrolled");
    assert_eq!(buffer.cursor().pos.y, y_before);
    let first: String = buffer.rows()[0]
        .cells()
        .iter()
        .map(freminal_buffer::cell::Cell::into_utf8)
        .collect();
    assert_eq!(first.trim_end(), "r0", "the region content is untouched");
}
