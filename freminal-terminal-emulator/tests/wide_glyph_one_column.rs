// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A double-width glyph in a 1-column terminal must not hang the emulator
//! (Task 148.5, issue #558).
//!
//! The `terminal_state` fuzz target found a six-byte input -- one column, four
//! rows, one CJK ideograph -- that made `Buffer::insert_text` wrap forever and
//! push a row per iteration until a 2 GiB `Vec<Row>` growth killed the
//! process. This feeds the same bytes through the same headless pipeline.
//! Before the fix every test here hung or ran out of memory.
//!
//! The decided behaviour is the kitty / xterm one: a glyph wider than the
//! line is discarded, and the text around it is unaffected.

use freminal_terminal_emulator::interface::TerminalEmulator;

/// U+4E2D, display width 2.
const WIDE: &str = "\u{4e2d}";

fn headless(cols: usize, rows: usize) -> TerminalEmulator {
    let (mut emulator, _write_rx) = TerminalEmulator::new_headless(None);
    assert!(
        emulator.set_win_size(cols, rows, 8, 16).is_ok(),
        "resize a headless emulator"
    );
    emulator
}

/// The visible screen, one string per row, trailing blanks trimmed and
/// trailing blank rows dropped (rows are allocated lazily).
fn screen(emulator: &TerminalEmulator) -> Vec<String> {
    let buffer = emulator.internal.handler.buffer();
    let mut rows: Vec<String> = buffer
        .visible_rows(0)
        .iter()
        .map(|row| {
            (0..buffer.terminal_width())
                .map(|col| row.resolve_cell(col))
                .filter(|cell| !cell.is_continuation())
                .map(|cell| cell.tchar().to_string())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

#[test]
fn fuzz_reproducer_terminates() {
    // The minimised `terminal_state` input: cols seed 0 (1 column), rows seed
    // 3 (4 rows), chunk seed 0, then the three bytes of U+4E2D.
    let input = [0x00, 0x03, 0x00, 0xe4, 0xb8, 0xad];
    let cols = 1 + usize::from(input[0]) % 300;
    let rows = 1 + usize::from(input[1]) % 120;
    assert_eq!((cols, rows), (1, 4));

    let mut emulator = headless(cols, rows);
    emulator.handle_incoming_data(&input[3..]);
    drop(emulator.build_snapshot());

    assert!(screen(&emulator).is_empty(), "nothing may be written");
}

#[test]
fn wide_glyph_is_dropped_and_surrounding_text_kept_in_one_column() {
    let mut emulator = headless(1, 4);
    emulator.handle_incoming_data(format!("a{WIDE}b{WIDE}{WIDE}c").as_bytes());

    assert_eq!(screen(&emulator), ["a", "b", "c"]);
}

#[test]
fn wide_glyph_terminates_on_the_alternate_screen() {
    // The alternate screen has no scrollback, so the old loop spun on a
    // scroll per iteration instead of growing: a hang, not an OOM.
    let mut emulator = headless(1, 4);
    emulator.handle_incoming_data(format!("\x1b[?1049h{WIDE}a").as_bytes());

    assert_eq!(screen(&emulator), ["a"]);
}

#[test]
fn wide_glyph_terminates_on_a_double_width_row() {
    // `ESC # 6` makes the row double-width: a 3-column terminal offers a
    // span of one cell there, narrower than the glyph.
    let mut emulator = headless(3, 4);
    emulator.handle_incoming_data(format!("\x1b#6{WIDE}a").as_bytes());

    assert_eq!(screen(&emulator)[0], "a");
}
