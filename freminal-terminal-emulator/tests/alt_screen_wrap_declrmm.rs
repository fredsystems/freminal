// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Auto-wrap at the bottom of the scroll region on the alternate screen honours
//! DECLRMM (Task 148.C12), driven end to end through the escape-sequence
//! parser.
//!
//! xterm's `xtermScroll` (`util.c`) confines the scroll to the left/right
//! margins unless they span the screen, on both screens. See
//! `freminal-buffer/tests/alt_wrap_declrmm.rs` for the buffer-level cases.

use freminal_buffer::cell::Cell;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

const COLS: usize = 10;
const ROWS: usize = 4;

fn make_state() -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(COLS, ROWS, 8, 16);
    state
}

fn row_text(state: &TerminalState, row: usize) -> String {
    let buffer = state.handler.buffer();
    (0..COLS)
        .map(|col| {
            buffer.rows()[row]
                .cells()
                .get(col)
                .map_or_else(|| " ".to_string(), Cell::into_utf8)
        })
        .collect()
}

/// Enter the alternate screen and fill row `r` with the digit `r`.
const FILL: &[u8] = b"\x1b[?1049h\x1b[1;1H0000000000\x1b[2;1H1111111111\
\x1b[3;1H2222222222\x1b[4;1H3333333333";

#[test]
fn wrap_scroll_is_confined_to_declrmm_margins() {
    let mut state = make_state();
    state.handle_incoming_data(FILL);
    // DECLRMM on, margins columns 2..5, cursor at the left margin of the
    // bottom row, then text that wraps at the right margin.
    state.handle_incoming_data(b"\x1b[?69h\x1b[2;5s\x1b[4;2Habcdef");

    assert!(state.handler.buffer().is_alternate_screen());
    assert_eq!(row_text(&state, 0), "0111100000");
    assert_eq!(row_text(&state, 1), "1222211111");
    assert_eq!(row_text(&state, 2), "2abcd22222");
    assert_eq!(row_text(&state, 3), "3ef  33333");
}

#[test]
fn wrap_scroll_shifts_whole_rows_with_declrmm_off() {
    let mut state = make_state();
    state.handle_incoming_data(FILL);
    state.handle_incoming_data(b"\x1b[4;1Habcdefghijk");

    assert_eq!(row_text(&state, 0), "1111111111");
    assert_eq!(row_text(&state, 1), "2222222222");
    assert_eq!(row_text(&state, 2), "abcdefghij");
    assert_eq!(row_text(&state, 3), "k         ");
}
