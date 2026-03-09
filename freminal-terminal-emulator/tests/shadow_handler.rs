// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Step 5.1 smoke tests: drive a realistic sequence of terminal events through
//! `TerminalState::handle_incoming_data` and verify that:
//!   - The old buffer state is unchanged / correct (the primary invariant).
//!   - The shadow handler does not panic (tested implicitly — if it panics the
//!     test fails).
//!
//! `debug_assertions` are always active under `cargo test`, so the shadow
//! handler code path is always exercised here.

use freminal_terminal_emulator::state::internal::TerminalState;

fn make_state() -> TerminalState {
    TerminalState::default()
}

#[test]
fn shadow_handler_does_not_panic_on_basic_session() {
    let mut state = make_state();

    // Plain text
    state.handle_incoming_data(b"hello world");

    // CR + LF
    state.handle_incoming_data(b"\r\n");

    // SGR bold + text + reset
    state.handle_incoming_data(b"\x1b[1mbold\x1b[0m");

    // SGR foreground color (red) + text
    state.handle_incoming_data(b"\x1b[31mred text\x1b[0m");

    // DECAWM off + on
    state.handle_incoming_data(b"\x1b[?7l");
    state.handle_incoming_data(b"\x1b[?7h");

    // DECTCEM hide + show
    state.handle_incoming_data(b"\x1b[?25l");
    state.handle_incoming_data(b"\x1b[?25h");

    // Cursor movement
    state.handle_incoming_data(b"\x1b[3;5H"); // row 3, col 5

    // Erase to end of line
    state.handle_incoming_data(b"\x1b[K");

    // More text after erase
    state.handle_incoming_data(b"after erase");

    // Alternate screen enter + some text + leave
    state.handle_incoming_data(b"\x1b[?1049h");
    state.handle_incoming_data(b"alternate screen");
    state.handle_incoming_data(b"\x1b[?1049l");

    // OSC title
    state.handle_incoming_data(b"\x1b]0;My Terminal\x07");

    // Verify old buffer is still in a sane state.
    let (w, h) = state.get_win_size();
    assert!(w > 0, "terminal width must be > 0 after session");
    assert!(h > 0, "terminal height must be > 0 after session");
}

#[test]
fn shadow_handler_handles_rapid_writes() {
    let mut state = make_state();

    // Simulate a burst of many small writes (common in real PTY usage).
    for i in 0..100_u8 {
        let line = format!("line {i}\r\n");
        state.handle_incoming_data(line.as_bytes());
    }

    let (w, h) = state.get_win_size();
    assert!(w > 0);
    assert!(h > 0);
}

#[test]
fn shadow_handler_handles_sgr_sequence() {
    let mut state = make_state();

    // A rich SGR sequence: bold + italic + underline + fg + bg + reset.
    state.handle_incoming_data(b"\x1b[1;3;4;31;42mstyle\x1b[0m normal");

    let (w, h) = state.get_win_size();
    assert!(w > 0);
    assert!(h > 0);
}

#[test]
fn shadow_handler_handles_resize() {
    let mut state = make_state();

    state.handle_incoming_data(b"before resize\r\n");

    // Resize the terminal — both old buffer and shadow handler must resize.
    state.set_win_size(100, 30);

    state.handle_incoming_data(b"after resize\r\n");

    let (w, h) = state.get_win_size();
    assert_eq!(w, 100);
    assert_eq!(h, 30);
}
