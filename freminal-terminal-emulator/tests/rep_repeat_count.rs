// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! REP (CSI Ps b) repeat-count bounds and equivalence (Task 148.1,
//! GHSA-2746-x3cj-58c7).
//!
//! REP used to materialise `vec![ch; count]` with `count` taken straight from
//! the CSI parameter, so `a ESC [ 2000000000 b` requested tens of GB. The
//! count is now clamped to xterm's `MAX_I_PARAM` (65535) and written in
//! bounded chunks. These tests pin both halves: huge counts are cheap and
//! behave as exactly 65535 repeats, and for in-range counts the chunked write
//! is indistinguishable from writing the characters as ordinary text.

use freminal_buffer::buffer::Buffer;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;
use freminal_terminal_emulator::terminal_handler::TerminalHandler;

/// xterm's `MAX_I_PARAM`: the effective REP maximum.
const XTERM_MAX_REPEAT: usize = 65_535;

/// Everything observable about a buffer after a write: every row of the
/// visible window and of the oldest reachable scrollback window (text plus
/// wrap bookkeeping) and the cursor.
#[derive(Debug, PartialEq, Eq)]
struct BufferView {
    newest: Vec<(String, String)>,
    oldest: Vec<(String, String)>,
    cursor: (usize, usize),
}

fn window(buffer: &Buffer, scroll_offset: usize) -> Vec<(String, String)> {
    buffer
        .visible_rows(scroll_offset)
        .iter()
        .map(|row| {
            let text: String = row
                .characters()
                .iter()
                .map(freminal_buffer::cell::Cell::into_utf8)
                .collect();
            (
                text.trim_end().to_string(),
                format!("{:?}/{:?}", row.origin, row.join),
            )
        })
        .collect()
}

fn view(buffer: &Buffer) -> BufferView {
    let oldest_offset = buffer.scroll_back(0, usize::MAX);
    let cursor = buffer.cursor().pos;
    BufferView {
        newest: window(buffer, 0),
        oldest: window(buffer, oldest_offset),
        cursor: (cursor.x, cursor.y),
    }
}

/// A `TerminalState` of the given size, plus the PTY receiver (kept alive so
/// replies never hit a closed channel).
fn make_state(cols: usize, rows: usize) -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(cols, rows, 8, 16);
    (state, rx)
}

/// Feed `stream` through the full parser -> handler pipeline.
fn run_stream(cols: usize, rows: usize, stream: &[u8]) -> BufferView {
    let (mut state, _rx) = make_state(cols, rows);
    state.handle_incoming_data(stream);
    view(state.handler.buffer())
}

/// `prefix`, one `a`, then `count` repeats of it via REP.
fn rep_stream(prefix: &str, count: usize) -> Vec<u8> {
    format!("{prefix}a\x1b[{count}b").into_bytes()
}

/// `prefix`, then `count + 1` literal `a` characters as ordinary text.
fn text_stream(prefix: &str, count: usize) -> Vec<u8> {
    let mut stream = prefix.as_bytes().to_vec();
    stream.resize(stream.len() + count + 1, b'a');
    stream
}

const COLS: usize = 7;
const ROWS: usize = 3;

const fn equivalence_counts() -> [usize; 6] {
    [1, COLS - 1, COLS, COLS + 1, 3 * COLS * ROWS + 7, 5000]
}

fn assert_rep_equals_text(prefix: &str) {
    for count in equivalence_counts() {
        let rep = run_stream(COLS, ROWS, &rep_stream(prefix, count));
        let text = run_stream(COLS, ROWS, &text_stream(prefix, count));
        assert_eq!(
            rep, text,
            "REP {count} diverged from plain text (prefix {prefix:?})"
        );
    }
}

#[test]
fn rep_equals_plain_text_with_autowrap() {
    assert_rep_equals_text("");
}

#[test]
fn rep_equals_plain_text_with_autowrap_off() {
    // DECAWM reset: the overflow is discarded and the cursor sticks to the
    // last column.
    assert_rep_equals_text("\x1b[?7l");
}

#[test]
fn rep_equals_plain_text_with_left_right_margins() {
    // DECLRMM on, margins at columns 2..=5: wrapping is confined to them.
    assert_rep_equals_text("\x1b[?69h\x1b[2;5s");
}

#[test]
fn rep_equals_plain_text_on_double_width_line() {
    // DECDWL halves the wrap width of the row.
    assert_rep_equals_text("\x1b#6");
}

#[test]
fn rep_equals_plain_text_from_mid_row() {
    // Starts mid-row and with a non-default cursor, so the first chunk begins
    // with text already on the line.
    assert_rep_equals_text("xy");
}

#[test]
fn rep_of_wide_character_equals_plain_text() {
    // A double-width glyph in a 7-column row leaves a 1-column hole at the
    // wrap edge that both paths must treat identically.
    for count in equivalence_counts() {
        let rep = run_stream(COLS, ROWS, &rep_stream_with("\u{4e2d}", count));
        let mut text = Vec::new();
        for _ in 0..=count {
            text.extend_from_slice("\u{4e2d}".as_bytes());
        }
        assert_eq!(rep, run_stream(COLS, ROWS, &text), "wide REP {count}");
    }
}

fn rep_stream_with(glyph: &str, count: usize) -> Vec<u8> {
    format!("{glyph}\x1b[{count}b").into_bytes()
}

#[test]
fn rep_default_and_zero_counts_repeat_once() {
    let default = run_stream(COLS, ROWS, b"a\x1b[b");
    let zero = run_stream(COLS, ROWS, b"a\x1b[0b");
    let one = run_stream(COLS, ROWS, b"aa");
    assert_eq!(default, one);
    assert_eq!(zero, one);
}

#[test]
fn rep_count_at_cap_equals_plain_text() {
    let rep = run_stream(COLS, ROWS, &rep_stream("", XTERM_MAX_REPEAT));
    let text = run_stream(COLS, ROWS, &text_stream("", XTERM_MAX_REPEAT));
    assert_eq!(rep, text);
}

#[test]
fn rep_huge_count_is_clamped_to_the_xterm_maximum() {
    // `usize::MAX` through the handler: must neither allocate nor loop
    // `usize::MAX` times, and must equal exactly 65535 repeats.
    let mut huge = TerminalHandler::new(COLS, ROWS);
    huge.handle_data(b"a");
    huge.process_outputs(&[TerminalOutput::RepeatCharacter(usize::MAX)]);

    let mut capped = TerminalHandler::new(COLS, ROWS);
    capped.handle_data(b"a");
    capped.process_outputs(&[TerminalOutput::RepeatCharacter(XTERM_MAX_REPEAT)]);

    assert_eq!(view(huge.buffer()), view(capped.buffer()));
    // And the capped result is the same as plain text of 65535 + 1 chars.
    assert_eq!(
        view(huge.buffer()),
        run_stream(COLS, ROWS, &text_stream("", XTERM_MAX_REPEAT))
    );
}

#[test]
fn rep_huge_count_through_the_full_parser() {
    // The fuzz reproducer: 2,000,000,000 would need tens of GB unclamped.
    let huge = run_stream(COLS, ROWS, b"a\x1b[2000000000b");
    let capped = run_stream(COLS, ROWS, &rep_stream("", XTERM_MAX_REPEAT));
    assert_eq!(huge, capped);

    // The screen really is filled with the repeated character: 65536 'a' in
    // total on a 7-column screen leaves 65536 % 7 = 2 on the last row.
    let texts: Vec<&str> = huge.newest.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(texts, ["aaaaaaa", "aaaaaaa", "aa"]);
    // The cursor's row is an index into scrollback + screen, so only the
    // column is asserted here; the row is covered by the equality above.
    assert_eq!(huge.cursor.0, 2);
}

#[test]
fn rep_huge_count_with_no_preceding_character_is_a_noop() {
    let view_after = run_stream(COLS, ROWS, b"\x1b[2000000000b");
    assert_eq!(view_after.cursor, (0, 0));
    assert!(view_after.newest.iter().all(|(text, _)| text.is_empty()));
}
