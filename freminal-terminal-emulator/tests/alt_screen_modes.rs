// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Handler-level tests for the per-mode alternate-screen semantics
//! (`?47`, `?1047`, `?1049`) and the per-screen DECSC character-set slot.
//!
//! Everything is driven through raw bytes, exactly as the PTY thread does.
//! The reference behaviour is the consensus of xterm, Ghostty, WezTerm and
//! kitty; see `Documents/PLAN_VERSION_130.md` ("131 Decisions").

use freminal_buffer::cell::Cell;
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

/// All visible rows of the active screen, trailing blanks trimmed, joined by
/// newlines.
fn screen_text(handler: &TerminalHandler) -> String {
    handler
        .buffer()
        .visible_rows(0)
        .iter()
        .map(|row| {
            row.cells()
                .iter()
                .map(Cell::into_utf8)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// ?47
// ---------------------------------------------------------------------------

#[test]
fn mode_47_enter_keeps_cursor_position() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;10H");
    assert_eq!(pos(&h), (9, 4));
    feed(&mut h, "\x1b[?47h");
    assert!(h.is_alternate_screen());
    assert_eq!(pos(&h), (9, 4));
}

#[test]
fn mode_47_enter_does_not_save_the_cursor() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;10H\x1b[?47h");
    assert!(!h.has_saved_cursor());
}

#[test]
fn mode_47_does_not_clear_alternate_contents_on_reentry() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "PRIMARY");
    feed(&mut h, "\x1b[?47h");
    assert!(
        !screen_text(&h).contains("PRIMARY"),
        "the alternate screen starts blank"
    );
    feed(&mut h, "\x1b[3;1HALT");
    feed(&mut h, "\x1b[?47l");
    assert!(screen_text(&h).contains("PRIMARY"));
    feed(&mut h, "\x1b[?47h");
    assert!(screen_text(&h).contains("ALT"));
}

#[test]
fn mode_47_leave_does_not_restore_the_cursor() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;10H\x1b[?47h");
    feed(&mut h, "\x1b[10;20H");
    assert_eq!(pos(&h), (19, 9));
    feed(&mut h, "\x1b[?47l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (19, 9));
}

// ---------------------------------------------------------------------------
// ?1047
// ---------------------------------------------------------------------------

#[test]
fn mode_1047_enter_keeps_cursor_and_does_not_clear() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[?47h\x1b[2;1HPERSIST\x1b[?47l");
    feed(&mut h, "\x1b[5;10H\x1b[?1047h");
    assert!(h.is_alternate_screen());
    assert_eq!(pos(&h), (9, 4));
    assert!(screen_text(&h).contains("PERSIST"));
}

#[test]
fn mode_1047_leave_clears_the_alternate_screen() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[?1047h\x1b[2;1HZZ");
    assert!(screen_text(&h).contains("ZZ"));
    feed(&mut h, "\x1b[?1047l");
    assert!(!h.is_alternate_screen());
    feed(&mut h, "\x1b[?47h");
    assert!(!screen_text(&h).contains("ZZ"));
}

#[test]
fn mode_1047_leave_does_not_restore_the_cursor() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;10H\x1b[?1047h\x1b[10;20H\x1b[?1047l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (19, 9));
}

#[test]
fn mode_1047_leave_on_primary_is_a_no_op() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[?47h\x1b[2;1HKEEP\x1b[?47l");
    feed(&mut h, "PRIMARY\x1b[7;3H");
    feed(&mut h, "\x1b[?1047l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (2, 6));
    assert!(screen_text(&h).contains("PRIMARY"));
    // The parked alternate screen was not cleared either.
    feed(&mut h, "\x1b[?47h");
    assert!(screen_text(&h).contains("KEEP"));
}

// ---------------------------------------------------------------------------
// ?1049
// ---------------------------------------------------------------------------

#[test]
fn mode_1049_enter_clears_and_keeps_cursor_position() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[?47h\x1b[2;1HOLD\x1b[?47l");
    feed(&mut h, "\x1b[?47h");
    assert!(
        screen_text(&h).contains("OLD"),
        "precondition: the alternate contents persisted"
    );
    feed(&mut h, "\x1b[?47l\x1b[5;10H\x1b[?1049h");
    assert!(h.is_alternate_screen());
    assert_eq!(pos(&h), (9, 4));
    assert!(!screen_text(&h).contains("OLD"));
}

#[test]
fn mode_1049_leave_restores_the_cursor_saved_at_entry() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[5;10H\x1b[?1049h\x1b[1;1H\x1b[?1049l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (9, 4));
}

#[test]
fn mode_1049_second_enter_saves_into_the_alternate_slot_and_clears() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[2;3H\x1b[?1049h");
    feed(&mut h, "\x1b[4;5HZZ\x1b[4;5H");
    assert!(screen_text(&h).contains("ZZ"));
    // Second entry while already on the alternate screen: DECSC into the
    // alternate slot (position (4, 3)), then a fresh clear.
    feed(&mut h, "\x1b[?1049h");
    assert!(h.is_alternate_screen());
    assert!(!screen_text(&h).contains("ZZ"));
    feed(&mut h, "\x1b[8;8H");
    // Leaving restores the PRIMARY slot, i.e. the one saved by the first entry.
    feed(&mut h, "\x1b[?1049l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (2, 1));
    // The alternate slot holds the second save.
    feed(&mut h, "\x1b[?47h\x1b[20;20H\x1b8");
    assert_eq!(pos(&h), (4, 3));
}

#[test]
fn mode_1049_leave_on_primary_performs_decrc() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[3;4H\x1b7\x1b[10;10H");
    feed(&mut h, "\x1b[?1049l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (3, 2));
}

#[test]
fn mixed_47_enter_then_1049_leave_switches_and_restores() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b[3;4H\x1b7\x1b[10;10H\x1b[?47h\x1b[15;15H");
    assert!(h.is_alternate_screen());
    feed(&mut h, "\x1b[?1049l");
    assert!(!h.is_alternate_screen());
    assert_eq!(pos(&h), (3, 2));
}

// ---------------------------------------------------------------------------
// ?1046 gating
// ---------------------------------------------------------------------------

#[test]
fn disallowed_alternate_screen_ignores_every_mode() {
    for set in ["\x1b[?47h", "\x1b[?1047h", "\x1b[?1049h"] {
        let mut h = TerminalHandler::new(80, 24);
        feed(&mut h, "\x1b[?1046l");
        feed(&mut h, "KEEP\x1b[5;10H");
        feed(&mut h, set);
        assert!(!h.is_alternate_screen(), "{set:?} must be ignored");
        assert_eq!(pos(&h), (9, 4), "{set:?} must not move the cursor");
        assert!(!h.has_saved_cursor(), "{set:?} must not save the cursor");
        assert!(screen_text(&h).contains("KEEP"), "{set:?} must not clear");
    }
}

// ---------------------------------------------------------------------------
// Per-screen DECSC character-set slot
// ---------------------------------------------------------------------------

#[test]
fn decsc_charset_slot_is_per_screen_primary_survives_alternate_save() {
    let mut h = TerminalHandler::new(80, 24);
    // Primary: DEC graphics on, DECSC at (3, 2).
    feed(&mut h, "\x1b[3;4H\x1b(0\x1b7");
    // Alternate: ASCII, DECSC elsewhere.
    feed(&mut h, "\x1b[?47h\x1b(B\x1b[9;9H\x1b7");
    // Back on the primary, DECRC must bring back the PRIMARY slot.
    feed(&mut h, "\x1b[?47l\x1b8");
    assert_eq!(pos(&h), (3, 2));
    feed(&mut h, "q");
    assert!(
        screen_text(&h).contains('─'),
        "primary DECRC must restore DEC graphics, got {:?}",
        screen_text(&h)
    );
}

#[test]
fn decsc_charset_slot_is_per_screen_alternate_survives_primary_save() {
    let mut h = TerminalHandler::new(80, 24);
    // Alternate: DEC graphics on, DECSC at (8, 8), then back to ASCII.
    feed(&mut h, "\x1b[?47h\x1b(0\x1b[9;9H\x1b7\x1b(B\x1b[?47l");
    // Primary: ASCII, DECSC afterwards (must not overwrite the alternate slot).
    feed(&mut h, "\x1b(B\x1b[3;4H\x1b7");
    feed(&mut h, "\x1b[?47h\x1b8");
    assert_eq!(pos(&h), (8, 8));
    feed(&mut h, "q");
    assert!(
        screen_text(&h).contains('─'),
        "alternate DECRC must restore DEC graphics, got {:?}",
        screen_text(&h)
    );
}

#[test]
fn ris_clears_both_charset_slots() {
    let mut h = TerminalHandler::new(80, 24);
    feed(&mut h, "\x1b(0\x1b7\x1b[?47h\x1b(0\x1b7");
    feed(&mut h, "\x1bc");
    assert!(!h.is_alternate_screen());
    // A DECRC on either screen must not resurrect DEC graphics.
    feed(&mut h, "\x1b8q");
    assert!(screen_text(&h).contains('q'));
    feed(&mut h, "\x1b[?47h\x1b8q");
    assert!(screen_text(&h).contains('q'));
    assert!(!screen_text(&h).contains('─'));
}

// ---------------------------------------------------------------------------
// Task 131.C5: image placement at the alternate screen's bottom
// ---------------------------------------------------------------------------

#[test]
fn kitty_image_on_alternate_last_row_keeps_the_store_at_height() {
    let mut h = TerminalHandler::new(80, 24);
    let height = h.buffer().terminal_height();
    feed(&mut h, "\x1b[?1049h");
    feed(&mut h, &format!("\x1b[{height};1H"));
    feed(&mut h, "\x1b_Ga=T,f=24,s=1,v=1,c=1,r=1,i=7,q=2;AAAA\x1b\\");
    assert!(h.is_alternate_screen());
    assert_eq!(
        h.buffer().rows().len(),
        height,
        "the alternate store must stay exactly one screen tall"
    );
}
