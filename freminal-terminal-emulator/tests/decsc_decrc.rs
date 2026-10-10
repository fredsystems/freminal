// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Handler-level tests for what DECSC / DECRC save and restore, and what DECRC
//! does when nothing was saved (Tasks 131.C1 and 131.C2).
//!
//! The reference is xterm's `CursorSave2` / `CursorRestoreFlags` (`cursor.c`):
//! DECSC saves the position, the SGR rendition, DECOM and the character set,
//! one slot per screen, and DECRC with an empty slot behaves as if a default
//! cursor at home had been saved.  Everything is driven through raw bytes,
//! exactly as the PTY thread does.

use freminal_buffer::cell::Cell;
use freminal_common::buffer_states::{
    fonts::FontWeight, format_tag::FormatTag, modes::decom::Decom,
};
use freminal_common::colors::TerminalColor;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use freminal_terminal_emulator::terminal_handler::TerminalHandler;

/// Feed raw bytes through the parser into the handler.
fn feed(handler: &mut TerminalHandler, bytes: &str) {
    let mut parser = FreminalAnsiParser::default();
    let outputs = parser.push(bytes.as_bytes());
    handler.process_outputs(&outputs);
}

/// The cursor's 0-based `(x, y)` screen position.
fn pos(handler: &TerminalHandler) -> (usize, usize) {
    let p = handler.buffer().cursor_screen_pos();
    (p.x, p.y)
}

/// The cell at 0-based screen `(x, y)` of the active screen.
fn cell_at(handler: &TerminalHandler, x: usize, y: usize) -> Cell {
    let rows = handler.buffer().visible_rows(0);
    let row = rows.get(y).expect("row exists");
    row.cells().get(x).expect("cell exists").clone()
}

#[test]
fn decrc_restores_the_saved_sgr_rendition() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[1;31m\x1b7\x1b[0m\x1b8X");

    let cell = cell_at(&h, 0, 0);
    assert_eq!(cell.into_utf8(), "X");
    assert_eq!(cell.tag().font_weight, FontWeight::Bold);
    assert_eq!(cell.tag().colors.color, TerminalColor::Red);
}

#[test]
fn decrc_writes_over_earlier_text_with_the_restored_rendition() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[1;31m\x1b7\x1b[0mA\x1b8B");

    let plain = cell_at(&h, 0, 0);
    assert_eq!(plain.into_utf8(), "B");
    assert_eq!(
        plain.tag().font_weight,
        FontWeight::Bold,
        "B overwrote A at the restored position with the restored rendition"
    );
}

#[test]
fn decom_survives_decsc_decrc_without_homing() {
    let mut h = TerminalHandler::new(80, 24);
    // Margins rows 3..10, DECOM on, move inside the region, DECSC.
    feed(&mut h, "\x1b[3;10r\x1b[?6h\x1b[2;5H");
    assert_eq!(h.buffer().is_decom_enabled(), Decom::OriginMode);
    let saved = pos(&h);
    assert_eq!(saved, (4, 3), "row 2 of the region is screen row 3");
    feed(&mut h, "\x1b7");

    // DECOM off homes the cursor; DECRC must bring both back.
    feed(&mut h, "\x1b[?6l");
    assert_eq!(h.buffer().is_decom_enabled(), Decom::NormalCursor);
    assert_eq!(pos(&h), (0, 0));
    feed(&mut h, "\x1b8");

    assert_eq!(h.buffer().is_decom_enabled(), Decom::OriginMode);
    assert_eq!(pos(&h), saved, "DECRC must not home the cursor");

    // And DECOM is observably on: CUP 1;1 lands on the top margin.
    feed(&mut h, "\x1b[1;1H");
    assert_eq!(pos(&h), (0, 2));
}

#[test]
fn decrc_turns_decom_off_when_it_was_off_at_the_save() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[3;10r\x1b[5;5H\x1b7\x1b[?6h");
    assert_eq!(h.buffer().is_decom_enabled(), Decom::OriginMode);
    feed(&mut h, "\x1b8");
    assert_eq!(h.buffer().is_decom_enabled(), Decom::NormalCursor);
    assert_eq!(pos(&h), (4, 4));
}

#[test]
fn decrc_with_nothing_saved_homes_and_resets_sgr_and_charset() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[11;6H\x1b[1;31m\x1b(0");
    assert_eq!(pos(&h), (5, 10));
    assert!(!h.has_saved_cursor(), "precondition: nothing saved");

    feed(&mut h, "\x1b8");
    assert_eq!(pos(&h), (0, 0), "DECRC with no save homes the cursor");
    assert_eq!(*h.current_format(), FormatTag::default());

    feed(&mut h, "q");
    let cell = cell_at(&h, 0, 0);
    assert_eq!(cell.into_utf8(), "q", "the charset is reset, not DEC line");
    assert_eq!(cell.tag().font_weight, FontWeight::Normal);
    assert_eq!(cell.tag().colors.color, TerminalColor::Default);
}

#[test]
fn decrc_with_nothing_saved_turns_decom_off() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[3;10r\x1b[?6h\x1b[2;5H\x1b8");
    assert_eq!(h.buffer().is_decom_enabled(), Decom::NormalCursor);
    assert_eq!(pos(&h), (0, 0));
}

#[test]
fn decrc_on_a_fresh_alternate_screen_homes_even_though_the_primary_saved() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[3;4H\x1b[1;31m\x1b7");
    feed(&mut h, "\x1b[?47h\x1b[9;9H\x1b[0m");
    assert!(h.is_alternate_screen());
    assert!(!h.has_saved_cursor(), "the alternate slot is empty");

    feed(&mut h, "\x1b8");
    assert_eq!(pos(&h), (0, 0), "no save on this screen: home");
    assert_eq!(
        *h.current_format(),
        FormatTag::default(),
        "the primary's bold red is not the alternate's"
    );

    // The primary slot is untouched.
    feed(&mut h, "\x1b[?47l\x1b8");
    assert_eq!(pos(&h), (3, 2));
    assert_eq!(h.current_format().font_weight, FontWeight::Bold);
}

#[test]
fn mode_1049_round_trip_restores_the_sgr_saved_at_entry() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[1m\x1b[?1049h\x1b[0m\x1b[?1049lY");

    assert!(!h.is_alternate_screen());
    let cell = cell_at(&h, 0, 0);
    assert_eq!(cell.into_utf8(), "Y");
    assert_eq!(cell.tag().font_weight, FontWeight::Bold);
}

#[test]
fn decstr_then_decrc_restores_default_rendition_at_home() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;5H\x1b[1;31m\x1b7\x1b[10;10H\x1b[1m\x1b[!p");
    feed(&mut h, "\x1b8");
    assert_eq!(pos(&h), (0, 0));
    assert_eq!(*h.current_format(), FormatTag::default());
}

/// An open OSC 8 hyperlink is not part of the DECSC state (xterm saves none,
/// kitty keeps it on the screen, Ghostty's saved cursor has none): DECRC keeps
/// the hyperlink that is live when it runs, in both directions.
#[test]
fn decrc_keeps_the_live_hyperlink() {
    // Saved without a link, restored while one is open: the link stays open.
    let mut handler = TerminalHandler::new(80, 24);
    feed(&mut handler, "\x1b7\x1b]8;;https://example.com\x1b\\\x1b8");
    assert!(
        handler.current_format().url.is_some(),
        "DECRC must not close a hyperlink opened after the save"
    );

    // Saved inside a link, restored after it closed: no link is resurrected.
    let mut handler = TerminalHandler::new(80, 24);
    feed(
        &mut handler,
        "\x1b]8;;https://example.com\x1b\\\x1b7\x1b]8;;\x1b\\\x1b8",
    );
    assert!(
        handler.current_format().url.is_none(),
        "DECRC must not reopen a hyperlink that was closed after the save"
    );
}
