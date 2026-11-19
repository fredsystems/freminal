// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Task 140.2, end to end: erased cells take only the current background.
//!
//! The bytes go through `TerminalState::handle_incoming_data`, as the PTY
//! thread feeds them, so the SGR handler, the OSC 8 handler and the buffer's
//! erase operations are all exercised together.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::{cursor::StateColors, format_tag::FormatTag};
use freminal_common::colors::TerminalColor;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

fn make_state() -> TerminalState {
    // The receiver is dropped: nothing in these streams needs a reply.
    let (tx, _rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(20, 5, 8, 16);
    state
}

const fn buffer(state: &TerminalState) -> &Buffer {
    state.handler.buffer()
}

fn blue_background_only() -> FormatTag {
    FormatTag {
        colors: StateColors::default().with_background_color(TerminalColor::Blue),
        ..FormatTag::default()
    }
}

#[test]
fn el_after_a_loud_sgr_leaves_only_the_blue_background() {
    let mut state = make_state();
    state.handle_incoming_data(b"HELLO WORLD");
    state.handle_incoming_data(b"\x1b[1;3H");
    state.handle_incoming_data(b"\x1b[1;4;31;44m");
    state.handle_incoming_data(b"\x1b[K");

    let row = &buffer(&state).rows()[0];
    for col in 2..20 {
        let cell = row.resolve_cell(col);
        assert_eq!(
            cell.tag(),
            &blue_background_only(),
            "col {col} carries only the blue background"
        );
        assert_eq!(cell.into_utf8(), " ", "col {col} is blank");
    }
    assert_eq!(row.resolve_cell(0).into_utf8(), "H");
    assert_eq!(row.resolve_cell(1).tag(), &FormatTag::default());
}

#[test]
fn el_with_an_open_hyperlink_does_not_put_the_url_in_the_erased_cells() {
    let mut state = make_state();
    state.handle_incoming_data(b"HELLO WORLD");
    state.handle_incoming_data(b"\x1b[1;3H");
    state.handle_incoming_data(b"\x1b[1;4;31;44m\x1b]8;;https://example.com/x\x1b\\");
    assert!(state.handler.current_format().url.is_some());
    state.handle_incoming_data(b"\x1b[K");

    let row = &buffer(&state).rows()[0];
    for col in 2..20 {
        let tag = row.resolve_cell(col).tag().clone();
        assert!(tag.url.is_none(), "col {col} carries no hyperlink");
        assert_eq!(tag, blue_background_only());
    }
}

#[test]
fn ed2_and_ech_after_a_loud_sgr_also_leave_only_the_background() {
    let mut state = make_state();
    state.handle_incoming_data(b"A\r\nB\r\nC\r\nD\r\nE");
    state.handle_incoming_data(b"\x1b[1;4;5;7;31;44m");
    state.handle_incoming_data(b"\x1b[2J");
    for row in 0..5 {
        let cell = buffer(&state).rows()[row].resolve_cell(7);
        assert_eq!(cell.tag(), &blue_background_only(), "ED 2 row {row}");
    }

    state.handle_incoming_data(b"\x1b[0m\x1b[H");
    state.handle_incoming_data(b"ABCDEFGH\x1b[1;1H");
    state.handle_incoming_data(b"\x1b[1;4;31;44m\x1b[3X");
    for col in 0..3 {
        let cell = buffer(&state).rows()[0].resolve_cell(col);
        assert_eq!(cell.tag(), &blue_background_only(), "ECH col {col}");
    }
    assert_eq!(buffer(&state).rows()[0].resolve_cell(3).into_utf8(), "D");
}

#[test]
fn reverse_video_with_a_default_background_erases_to_default_cells() {
    let mut state = make_state();
    state.handle_incoming_data(b"HELLO WORLD\x1b[1;3H");
    state.handle_incoming_data(b"\x1b[7;31m\x1b[K");

    let row = &buffer(&state).rows()[0];
    for col in 2..20 {
        assert_eq!(row.resolve_cell(col).tag(), &FormatTag::default());
    }
}

#[test]
fn dch_with_a_huge_count_does_not_panic_and_fills_the_edge() {
    let mut state = make_state();
    state.handle_incoming_data(b"ABCDEFGH\x1b[1;4H");
    state.handle_incoming_data(b"\x1b[1;4;31;44m");
    state.handle_incoming_data(b"\x1b[18446744073709551615P");

    let row = &buffer(&state).rows()[0];
    assert_eq!(row.resolve_cell(0).into_utf8(), "A");
    assert_eq!(row.resolve_cell(2).into_utf8(), "C");
    for col in 3..20 {
        let cell = row.resolve_cell(col);
        assert_eq!(cell.tag(), &blue_background_only(), "col {col}");
        assert_eq!(cell.into_utf8(), " ", "col {col} is blank");
    }
}

#[test]
fn dch_after_a_loud_sgr_fills_the_vacated_right_edge() {
    let mut state = make_state();
    state.handle_incoming_data(b"ABCDEFGH\x1b[1;2H");
    state.handle_incoming_data(b"\x1b[1;4;31;44m\x1b[2P");

    let row = &buffer(&state).rows()[0];
    let text: String = (0..6)
        .map(|col| row.resolve_cell(col).into_utf8())
        .collect();
    assert_eq!(text, "ADEFGH");
    for col in 6..18 {
        assert_eq!(
            row.resolve_cell(col).tag(),
            &FormatTag::default(),
            "col {col}"
        );
    }
    for col in 18..20 {
        assert_eq!(
            row.resolve_cell(col).tag(),
            &blue_background_only(),
            "col {col}"
        );
    }
}
