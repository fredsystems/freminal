//! Regression test for the "extra blank line on narrower resize" bug.
//!
//! Reproduced from a real fastfetch-over-SSH recording (`nik-freminal.frec`):
//! a program that leaves `ESC[1m` (bold) active, disables autowrap (`ESC[?7l`),
//! and lays out a side panel using cursor-forward (`CUF`, `ESC[<n>C`) after
//! bare line feeds. Before the fix, each line-feed-created row was BCE-filled
//! with the active (bold) SGR out to the full terminal width. Those full-width
//! bold-blank rows then survived `reflow_to_width`: on a narrower resize the
//! trailing blank padding overflowed into a spurious `SoftWrap` continuation
//! row, inserting an empty line between every content row. This matched the
//! user's report of extra spacing between lines that appears at one width and
//! disappears at another.
//!
//! A line feed only moves the active position; it is not an explicit erase, so
//! BCE must not apply. After the fix the line-feed-created rows are sparse and
//! reflow produces no extra blank rows.

use freminal_buffer::row::{Row, RowJoin};
use freminal_terminal_emulator::interface::TerminalEmulator;

fn row_text(row: &Row) -> String {
    row.characters()
        .iter()
        .map(freminal_buffer::cell::Cell::into_utf8)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Count rows that are a blank soft-wrap continuation — the bug's signature.
fn blank_continuation_rows(emu: &TerminalEmulator) -> usize {
    emu.internal
        .handler
        .buffer()
        .rows()
        .iter()
        .filter(|r| r.join == RowJoin::ContinueLogicalLine && row_text(r).is_empty())
        .count()
}

#[test]
fn bold_lf_side_panel_reflow_adds_no_blank_lines() {
    let (mut emu, _rx) = TerminalEmulator::new_headless(Some(4000));
    emu.set_win_size(80, 24, 8, 16).unwrap();

    // Turn on bold and never reset it (as the real fastfetch does), disable
    // autowrap, then draw several "art" lines each followed by a bare LF, and
    // overlay panel text to the right using CUF. All content is well under the
    // terminal width, so nothing should ever legitimately wrap.
    emu.handle_incoming_data(b"\x1b[1m\x1b[?7l");
    for i in 0..8u8 {
        // A short art fragment on the left...
        emu.handle_incoming_data(b"\x1b[34m###");
        // ...then jump right and write a panel label...
        emu.handle_incoming_data(b"\x1b[40C");
        emu.handle_incoming_data(format!("label {i}").as_bytes());
        // ...and a bare line feed to the next row (carriage return + LF).
        emu.handle_incoming_data(b"\r\n");
    }
    emu.handle_incoming_data(b"\x1b[?7h\x1b[0m");

    let rows_after_feed = emu.internal.handler.buffer().rows().len();
    assert_eq!(
        blank_continuation_rows(&emu),
        0,
        "no blank continuation rows should exist before any resize"
    );

    // Shrink the width. Nothing on screen exceeds the new width, so reflow must
    // not create any new rows and must not insert blank continuation lines.
    emu.set_win_size(72, 24, 8, 16).unwrap();
    assert_eq!(
        blank_continuation_rows(&emu),
        0,
        "narrowing must not insert blank continuation rows (the reported bug)"
    );
    assert_eq!(
        emu.internal.handler.buffer().rows().len(),
        rows_after_feed,
        "narrowing content that fits must not change the row count"
    );

    // Grow back — still no spurious blanks, still the same row count.
    emu.set_win_size(80, 24, 8, 16).unwrap();
    assert_eq!(blank_continuation_rows(&emu), 0);
    assert_eq!(emu.internal.handler.buffer().rows().len(), rows_after_feed);
}
