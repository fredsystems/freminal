// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! CHT / CBT with an enormous `Ps` must finish immediately (Task 148.C2).
//!
//! `CSI Ps I` used to advance one tab stop at a time, `Ps` times, even after
//! the cursor was pinned at the last column, so `CSI 4294967295 I` (or the
//! `usize::MAX`-sized values the parser accepts) hung the PTY thread. This
//! was the `terminal_state` fuzz timeout committed as
//! `fuzz/regressions/terminal_state/cht-huge-count-hang`.

use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

fn make_state(cols: usize, rows: usize) -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(cols, rows, 8, 16);
    state
}

const fn cursor_x(state: &TerminalState) -> usize {
    state.handler.buffer().cursor().pos.x
}

#[test]
fn cht_with_u32_max_count_lands_on_the_last_column() {
    let mut state = make_state(80, 24);
    state.handle_incoming_data(b"\x1b[4294967295I");
    assert_eq!(cursor_x(&state), 79);
}

#[test]
fn cht_with_usize_max_count_lands_on_the_last_column() {
    let mut state = make_state(80, 24);
    let stream = format!("\x1b[{}I", usize::MAX);
    state.handle_incoming_data(stream.as_bytes());
    assert_eq!(cursor_x(&state), 79);
}

#[test]
fn cht_with_huge_count_equals_cht_with_large_count() {
    let mut huge = make_state(80, 24);
    huge.handle_incoming_data(b"\x1b[4294967295I");
    let mut large = make_state(80, 24);
    large.handle_incoming_data(b"\x1b[100I");
    assert_eq!(cursor_x(&huge), cursor_x(&large));
}

#[test]
fn cht_small_counts_walk_the_default_stops() {
    let mut state = make_state(80, 24);
    state.handle_incoming_data(b"\x1b[I");
    assert_eq!(cursor_x(&state), 8);
    state.handle_incoming_data(b"\x1b[2I");
    assert_eq!(cursor_x(&state), 24);
}

#[test]
fn cbt_with_huge_count_goes_home() {
    let mut state = make_state(80, 24);
    state.handle_incoming_data(b"\x1b[79G\x1b[4294967295Z");
    assert_eq!(cursor_x(&state), 0);
}

#[test]
fn cht_with_huge_count_on_a_one_column_terminal() {
    let mut state = make_state(1, 1);
    state.handle_incoming_data(b"\x1b[4294967295I");
    assert_eq!(cursor_x(&state), 0);
}
