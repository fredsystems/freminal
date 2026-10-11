// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end alternate-screen lifecycle suite (Task 131.9).
//!
//! Every test drives a headless [`TerminalEmulator`] exactly as the PTY thread
//! does -- raw bytes into `handle_incoming_data` -- and asserts only on what
//! the GUI can observe: the published [`TerminalSnapshot`] (visible text,
//! cursor, `is_alternate_screen`, image placements, `kitty_keyboard_flags`) and
//! the replies written back to the PTY.  Handler-level assertions live in
//! `alt_screen_modes.rs`, `decsc_decrc.rs`, `kitty_keyboard_stack.rs` and
//! `reset_table.rs`; this file exists to catch the bugs those cannot see, such
//! as a stale per-buffer snapshot cache masking a screen switch in either
//! direction.
//!
//! The reference behaviour is the "131 Decisions" section of
//! `Documents/PLAN_VERSION_130.md` (the consensus of xterm, Ghostty, WezTerm
//! and kitty).

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::tchar::TChar;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::interface::TerminalEmulator;
use freminal_terminal_emulator::snapshot::TerminalSnapshot;

/// The three alternate-screen modes under test.
#[derive(Clone, Copy, Debug)]
enum AltMode {
    /// `?47`: switch only.
    M47,
    /// `?1047`: switch; clear on leave.
    M1047,
    /// `?1049`: DECSC + switch + clear on enter; switch + DECRC on leave.
    M1049,
}

impl AltMode {
    const ALL: [Self; 3] = [Self::M47, Self::M1047, Self::M1049];

    const fn number(self) -> u16 {
        match self {
            Self::M47 => 47,
            Self::M1047 => 1047,
            Self::M1049 => 1049,
        }
    }

    fn enter(self) -> String {
        format!("\x1b[?{}h", self.number())
    }

    fn leave(self) -> String {
        format!("\x1b[?{}l", self.number())
    }

    fn query(self) -> String {
        format!("\x1b[?{}$p", self.number())
    }

    fn report(self, status: u8) -> String {
        format!("\x1b[?{};{status}$y", self.number())
    }
}

/// A headless emulator plus the PTY channel it replies on.
struct Rig {
    emu: TerminalEmulator,
    rx: Receiver<PtyWrite>,
}

impl Rig {
    fn new() -> Self {
        let (emu, rx) = TerminalEmulator::new_headless(None);
        let rig = Self { emu, rx };
        rig.drain();
        rig
    }

    fn feed(&mut self, bytes: &str) {
        self.emu.handle_incoming_data(bytes.as_bytes());
    }

    fn snap(&mut self) -> TerminalSnapshot {
        self.emu.build_snapshot()
    }

    /// Everything written to the PTY since the last drain, as text.
    fn drain(&self) -> String {
        let bytes: Vec<u8> = self
            .rx
            .try_iter()
            .filter_map(|msg| match msg {
                PtyWrite::Write(bytes) => Some(bytes),
                PtyWrite::Resize(_) => None,
            })
            .flatten()
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Send `bytes` and return the PTY's reply.
    fn ask(&mut self, bytes: &str) -> String {
        self.drain();
        self.feed(bytes);
        self.drain()
    }

    /// The DECRQM reply for `mode`.
    fn decrqm(&mut self, mode: AltMode) -> String {
        self.ask(&mode.query())
    }
}

/// The visible rows of a snapshot, one string per row, trailing blanks trimmed.
fn rows(snap: &TerminalSnapshot) -> Vec<String> {
    let mut out = vec![String::new()];
    for c in snap.visible_chars.iter() {
        match c {
            TChar::NewLine => out.push(String::new()),
            TChar::Space => {
                if let Some(row) = out.last_mut() {
                    row.push(' ');
                }
            }
            TChar::Ascii(b) => {
                if let Some(row) = out.last_mut() {
                    row.push(char::from(*b));
                }
            }
            TChar::Utf8(buf, len) => {
                if let Some(row) = out.last_mut() {
                    row.push_str(&String::from_utf8_lossy(&buf[..usize::from(*len)]));
                }
            }
        }
    }
    out.iter().map(|r| r.trim_end().to_owned()).collect()
}

/// Every non-blank glyph of the snapshot, in order, with layout dropped.
fn text(snap: &TerminalSnapshot) -> String {
    rows(snap).concat().replace(' ', "")
}

/// The text of one 0-based visible row.
fn row(snap: &TerminalSnapshot, y: usize) -> String {
    rows(snap).get(y).cloned().unwrap_or_default()
}

/// The cursor's 0-based `(x, y)` position.
fn cursor(snap: &TerminalSnapshot) -> (usize, usize) {
    (snap.cursor_pos.x, snap.cursor_pos.y)
}

/// Whether any visible cell carries an image.
fn has_image(snap: &TerminalSnapshot) -> bool {
    snap.visible_image_placements.iter().any(Option::is_some)
}

/// A kitty graphics transmit-and-display of a 1x1 RGB image into one cell.
const KITTY_IMAGE: &str = "\x1b_Ga=T,f=24,s=1,v=1,c=1,r=1,i=7,q=2;AAAA\x1b\\";

// ---------------------------------------------------------------------------
// Per-mode lifecycle: enter / leave / double enter / double leave / query
// ---------------------------------------------------------------------------

#[test]
fn mode_47_enter_shows_blank_alternate_and_keeps_the_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    let before = rig.snap();
    assert!(!before.is_alternate_screen);
    assert_eq!(cursor(&before), (7, 0));

    rig.feed(&AltMode::M47.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert_eq!(text(&alt), "", "a first alternate entry is blank");
    assert_eq!(cursor(&alt), (7, 0), "?47 keeps the cursor position");
}

#[test]
fn mode_47_leave_restores_primary_text_and_keeps_the_cursor_position() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[4;3HALT");
    assert_eq!(cursor(&rig.snap()), (5, 3));

    rig.feed(&AltMode::M47.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert_eq!(text(&primary), "PRIMARY", "primary text intact");
    assert_eq!(
        cursor(&primary),
        (5, 3),
        "?47 leave neither restores nor saves: the cursor keeps its position"
    );
}

#[test]
fn mode_1047_enter_shows_blank_alternate_and_keeps_the_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1047.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert_eq!(text(&alt), "");
    assert_eq!(cursor(&alt), (7, 0));
}

#[test]
fn mode_1047_leave_clears_the_alternate_and_keeps_the_cursor_position() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1047.enter());
    rig.feed("\x1b[4;3HALT");

    rig.feed(&AltMode::M1047.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert_eq!(text(&primary), "PRIMARY");
    assert_eq!(cursor(&primary), (5, 3), "?1047 leave does not restore");

    rig.feed(&AltMode::M1047.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert_eq!(text(&alt), "", "?1047 leave cleared the alternate screen");
}

#[test]
fn mode_1049_enter_shows_blank_alternate_and_keeps_the_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1049.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert_eq!(text(&alt), "");
    assert_eq!(cursor(&alt), (7, 0), "?1049 keeps the cursor position");
}

#[test]
fn mode_1049_leave_restores_primary_text_and_the_saved_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1049.enter());
    rig.feed("\x1b[4;3HALT");
    assert_eq!(cursor(&rig.snap()), (5, 3));

    rig.feed(&AltMode::M1049.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert_eq!(text(&primary), "PRIMARY");
    assert_eq!(
        cursor(&primary),
        (7, 0),
        "?1049 leave is a DECRC of the cursor saved on enter"
    );
}

#[test]
fn mode_1049_reentry_is_blank_even_though_the_alternate_was_written() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M1049.enter());
    rig.feed("ALT");
    assert_eq!(text(&rig.snap()), "ALT");
    rig.feed(&AltMode::M1049.leave());
    let _ = rig.snap();

    rig.feed(&AltMode::M1049.enter());
    assert_eq!(
        text(&rig.snap()),
        "",
        "?1049 enter clears; no stale cached alternate frame may show"
    );
}

#[test]
fn double_enter_stays_on_the_alternate_screen_for_every_mode() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed("PRIMARY");
        rig.feed(&mode.enter());
        rig.feed("\x1b[3;1HALT");
        rig.feed(&mode.enter());
        let snap = rig.snap();
        assert!(snap.is_alternate_screen, "{mode:?}: still on the alternate");

        rig.feed(&mode.leave());
        let primary = rig.snap();
        assert!(
            !primary.is_alternate_screen,
            "{mode:?}: one leave returns to the primary after a double enter"
        );
        assert_eq!(text(&primary), "PRIMARY", "{mode:?}: primary text intact");
    }
}

#[test]
fn double_enter_keeps_alternate_text_for_47_and_1047_and_clears_it_for_1049() {
    for (mode, kept) in [
        (AltMode::M47, true),
        (AltMode::M1047, true),
        (AltMode::M1049, false),
    ] {
        let mut rig = Rig::new();
        rig.feed(&mode.enter());
        rig.feed("ALT");
        rig.feed(&mode.enter());
        let snap = rig.snap();
        let expected = if kept { "ALT" } else { "" };
        assert_eq!(text(&snap), expected, "{mode:?}: second enter");
    }
}

#[test]
fn double_leave_stays_on_the_primary_screen_with_text_intact_for_every_mode() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed("PRIMARY");
        rig.feed(&mode.enter());
        rig.feed(&mode.leave());
        rig.feed(&mode.leave());
        let snap = rig.snap();
        assert!(!snap.is_alternate_screen, "{mode:?}");
        assert_eq!(text(&snap), "PRIMARY", "{mode:?}");
    }
}

#[test]
fn leaving_the_primary_screen_with_1049_still_performs_decrc() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed("\x1b[5;5H\x1b7");
    rig.feed("\x1b[10;10H");
    assert_eq!(cursor(&rig.snap()), (9, 9));

    rig.feed(&AltMode::M1049.leave());
    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(
        text(&snap),
        "PRIMARY",
        "a leave on the primary clears nothing"
    );
    assert_eq!(
        cursor(&snap),
        (4, 4),
        "?1049 leave is an unconditional DECRC, even from the primary"
    );
}

#[test]
fn leaving_the_primary_screen_with_1047_does_not_clear_the_primary() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1047.leave());
    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(text(&snap), "PRIMARY");
    assert_eq!(cursor(&snap), (7, 0));
}

#[test]
fn decrqm_reports_set_on_the_alternate_and_reset_on_the_primary_for_every_mode() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        assert_eq!(
            rig.decrqm(mode),
            mode.report(2),
            "{mode:?}: initially reset"
        );

        rig.feed(&mode.enter());
        assert!(rig.snap().is_alternate_screen);
        assert_eq!(rig.decrqm(mode), mode.report(1), "{mode:?}: on alternate");

        rig.feed(&mode.leave());
        assert!(!rig.snap().is_alternate_screen);
        assert_eq!(
            rig.decrqm(mode),
            mode.report(2),
            "{mode:?}: back on primary"
        );
    }
}

// ---------------------------------------------------------------------------
// ?1046 gating
// ---------------------------------------------------------------------------

#[test]
fn disallowed_alternate_screen_keeps_the_primary_snapshot_for_every_mode() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed("\x1b[?1046l");
        rig.feed("PRIMARY");
        rig.feed(&mode.enter());
        let snap = rig.snap();
        assert!(
            !snap.is_alternate_screen,
            "{mode:?}: ?1046 l forbids the switch"
        );
        assert_eq!(text(&snap), "PRIMARY", "{mode:?}: primary still shown");

        rig.feed("MORE");
        assert_eq!(
            text(&rig.snap()),
            "PRIMARYMORE",
            "{mode:?}: output lands on primary"
        );
    }
}

#[test]
fn disallowed_alternate_screen_reports_reset_in_decrqm() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed("\x1b[?1046l");
        rig.feed(&mode.enter());
        assert_eq!(rig.decrqm(mode), mode.report(2), "{mode:?}");
    }
}

// ---------------------------------------------------------------------------
// Mixed modes
// ---------------------------------------------------------------------------

#[test]
fn enter_47_then_leave_1049_performs_decrc_on_the_primary() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed("\x1b[5;5H\x1b7");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[10;10HALT");
    assert!(rig.snap().is_alternate_screen);

    rig.feed(&AltMode::M1049.leave());
    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(text(&snap), "PRIMARY");
    assert_eq!(
        cursor(&snap),
        (4, 4),
        "?1049 leave restores the primary's DECSC even after a ?47 entry"
    );
}

#[test]
fn enter_1049_then_leave_47_does_not_restore_the_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M1049.enter());
    rig.feed("\x1b[10;10HALT");

    rig.feed(&AltMode::M47.leave());
    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(text(&snap), "PRIMARY");
    assert_eq!(
        cursor(&snap),
        (12, 9),
        "?47 leave never restores, so the alternate cursor position is kept"
    );
}

#[test]
fn enter_1049_leave_47_then_47_reentry_does_not_clear_the_alternate() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M1049.enter());
    rig.feed("ALT");
    rig.feed(&AltMode::M47.leave());
    assert_eq!(text(&rig.snap()), "");

    rig.feed(&AltMode::M47.enter());
    let snap = rig.snap();
    assert!(snap.is_alternate_screen);
    assert_eq!(
        text(&snap),
        "ALT",
        "?47 enter must not clear a parked alternate"
    );
}

#[test]
fn enter_47_then_leave_1047_clears_the_alternate() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M47.enter());
    rig.feed("ALT");
    rig.feed(&AltMode::M1047.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert_eq!(text(&primary), "PRIMARY");

    rig.feed(&AltMode::M47.enter());
    assert_eq!(
        text(&rig.snap()),
        "",
        "?1047 leave cleared what ?47 entered"
    );
}

// ---------------------------------------------------------------------------
// Persistence through snapshots (no stale-cache masking in either direction)
// ---------------------------------------------------------------------------

#[test]
fn alternate_text_persists_across_47_cycles_in_the_snapshot() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    let _ = rig.snap();

    rig.feed(&AltMode::M47.enter());
    rig.feed("ALT");
    let first_alt = rig.snap();
    assert!(first_alt.is_alternate_screen);
    assert_eq!(text(&first_alt), "ALT");

    rig.feed(&AltMode::M47.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert_eq!(
        text(&primary),
        "PRIMARY",
        "the alternate frame must not leak"
    );

    rig.feed(&AltMode::M47.enter());
    let second_alt = rig.snap();
    assert!(second_alt.is_alternate_screen);
    assert_eq!(
        text(&second_alt),
        "ALT",
        "the parked alternate screen returns, not a stale primary frame"
    );

    rig.feed(&AltMode::M47.leave());
    assert_eq!(text(&rig.snap()), "PRIMARY");
}

#[test]
fn primary_edits_made_while_alternate_is_parked_show_on_return() {
    let mut rig = Rig::new();
    rig.feed("ONE");
    let _ = rig.snap();
    rig.feed(&AltMode::M47.enter());
    rig.feed("ALT");
    let _ = rig.snap();
    rig.feed(&AltMode::M47.leave());
    rig.feed("TWO");
    assert_eq!(text(&rig.snap()), "ONETWO");

    rig.feed(&AltMode::M47.enter());
    assert_eq!(text(&rig.snap()), "ALT");
    rig.feed(&AltMode::M47.leave());
    assert_eq!(text(&rig.snap()), "ONETWO");
}

#[test]
fn alternate_edits_made_between_snapshots_show_on_the_next_entry() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("ONE");
    let _ = rig.snap();
    rig.feed(&AltMode::M47.leave());
    let _ = rig.snap();
    rig.feed(&AltMode::M47.enter());
    rig.feed("TWO");
    assert_eq!(text(&rig.snap()), "ONETWO");
}

// ---------------------------------------------------------------------------
// Kitty keyboard flags per screen
// ---------------------------------------------------------------------------

#[test]
fn kitty_keyboard_flags_are_independent_per_screen_in_the_snapshot() {
    let mut rig = Rig::new();
    rig.feed("\x1b[>1u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 1);

    rig.feed(&AltMode::M1049.enter());
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        0,
        "the alternate screen starts with its own empty stack"
    );
    rig.feed("\x1b[>4u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 4);

    rig.feed(&AltMode::M1049.leave());
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        1,
        "the primary's flags are untouched by the alternate session"
    );
}

#[test]
fn kitty_keyboard_alternate_stack_persists_between_alternate_sessions() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[>4u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 4);
    rig.feed(&AltMode::M47.leave());
    assert_eq!(rig.snap().kitty_keyboard_flags, 0);

    rig.feed(&AltMode::M47.enter());
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        4,
        "the alternate stack persists, as kitty's alt_key_encoding_flags does"
    );
}

#[test]
fn kitty_keyboard_query_reply_follows_the_active_screen() {
    let mut rig = Rig::new();
    rig.feed("\x1b[>1u");
    assert_eq!(rig.ask("\x1b[?u"), "\x1b[?1u");

    rig.feed(&AltMode::M47.enter());
    assert_eq!(rig.ask("\x1b[?u"), "\x1b[?0u");
    rig.feed("\x1b[>8u");
    assert_eq!(rig.ask("\x1b[?u"), "\x1b[?8u");

    rig.feed(&AltMode::M47.leave());
    assert_eq!(rig.ask("\x1b[?u"), "\x1b[?1u");
}

#[test]
fn kitty_keyboard_pop_on_the_alternate_leaves_the_primary_stack_alone() {
    let mut rig = Rig::new();
    rig.feed("\x1b[>1u\x1b[>2u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 2);

    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[<5u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 0);

    rig.feed(&AltMode::M47.leave());
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        2,
        "popping on the alternate must not pop the primary's entries"
    );
}

// ---------------------------------------------------------------------------
// DECSC per screen
// ---------------------------------------------------------------------------

#[test]
fn decsc_slot_is_per_screen_as_seen_through_the_cursor() {
    let mut rig = Rig::new();
    rig.feed("\x1b[3;3H\x1b7");

    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[10;10H\x1b7\x1b[1;1H");
    assert_eq!(cursor(&rig.snap()), (0, 0));
    rig.feed("\x1b8");
    assert_eq!(
        cursor(&rig.snap()),
        (9, 9),
        "the alternate restores its own save"
    );

    rig.feed(&AltMode::M47.leave());
    rig.feed("\x1b8");
    assert_eq!(
        cursor(&rig.snap()),
        (2, 2),
        "the primary restores its own save, not the alternate's"
    );

    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[1;1H\x1b8");
    assert_eq!(
        cursor(&rig.snap()),
        (9, 9),
        "the alternate's slot survives a visit to the primary"
    );
}

#[test]
fn decsc_on_the_alternate_does_not_disturb_the_primary_slot_across_1049() {
    let mut rig = Rig::new();
    rig.feed("\x1b[3;3H");
    rig.feed(&AltMode::M1049.enter());
    rig.feed("\x1b[10;10H\x1b7");
    rig.feed(&AltMode::M1049.leave());
    assert_eq!(
        cursor(&rig.snap()),
        (2, 2),
        "?1049 leave restores the primary save made by ?1049 enter"
    );
}

#[test]
fn second_1049_enter_resaves_in_the_alternate_slot_and_leaves_the_primary_save() {
    let mut rig = Rig::new();
    rig.feed("\x1b[3;3H");
    rig.feed(&AltMode::M1049.enter());
    rig.feed("\x1b[10;10H");
    rig.feed(&AltMode::M1049.enter());
    rig.feed("\x1b[1;1H\x1b8");
    assert_eq!(
        cursor(&rig.snap()),
        (9, 9),
        "the second enter saved the live alternate cursor into the alternate slot"
    );

    rig.feed(&AltMode::M1049.leave());
    assert_eq!(
        cursor(&rig.snap()),
        (2, 2),
        "the primary's save from the first enter is untouched"
    );
}

// ---------------------------------------------------------------------------
// Shared scroll margins
// ---------------------------------------------------------------------------

/// Write `RNN` markers on rows 4..=11 (1-based), set `5;10` margins first.
fn mark_rows_around_margins(rig: &mut Rig) {
    for r in 4..=11 {
        rig.feed(&format!("\x1b[{r};1HR{r}"));
    }
}

#[test]
fn margins_set_on_the_primary_confine_scrolling_on_the_alternate_with_47() {
    let mut rig = Rig::new();
    rig.feed("\x1b[5;10r");
    rig.feed(&AltMode::M47.enter());
    mark_rows_around_margins(&mut rig);
    rig.feed("\x1b[10;1H\n");

    let snap = rig.snap();
    assert!(snap.is_alternate_screen);
    assert_eq!(row(&snap, 3), "R4", "above the region: untouched");
    assert_eq!(row(&snap, 4), "R6", "region scrolled up by one");
    assert_eq!(row(&snap, 8), "R10", "region scrolled up by one");
    assert_eq!(row(&snap, 9), "", "the vacated bottom margin row is blank");
    assert_eq!(row(&snap, 10), "R11", "below the region: untouched");
}

#[test]
fn margins_survive_a_1049_round_trip_unchanged() {
    let mut rig = Rig::new();
    rig.feed("\x1b[5;10r");
    rig.feed(&AltMode::M1049.enter());
    rig.feed(&AltMode::M1049.leave());
    for r in 4..=11 {
        rig.feed(&format!("\x1b[{r};1HR{r}"));
    }
    rig.feed("\x1b[10;1H\n");

    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(row(&snap, 3), "R4");
    assert_eq!(row(&snap, 4), "R6", "the primary margins were not reset");
    assert_eq!(row(&snap, 9), "");
    assert_eq!(row(&snap, 10), "R11");
}

#[test]
fn margins_set_on_the_alternate_confine_scrolling_on_the_primary() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[5;10r");
    rig.feed(&AltMode::M47.leave());
    mark_rows_around_margins(&mut rig);
    rig.feed("\x1b[10;1H\n");

    let snap = rig.snap();
    assert!(!snap.is_alternate_screen);
    assert_eq!(row(&snap, 4), "R6", "margins are shared between screens");
    assert_eq!(row(&snap, 10), "R11");
}

// ---------------------------------------------------------------------------
// Image placements per screen
// ---------------------------------------------------------------------------

#[test]
fn kitty_image_on_the_alternate_shows_only_in_the_alternate_snapshot() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    assert!(!has_image(&rig.snap()));

    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[3;3H");
    rig.feed(KITTY_IMAGE);
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert!(has_image(&alt), "the alternate snapshot shows the image");

    rig.feed(&AltMode::M47.leave());
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert!(
        !has_image(&primary),
        "an alternate-screen image must not appear on the primary"
    );
    assert_eq!(text(&primary), "PRIMARY");

    rig.feed(&AltMode::M47.enter());
    assert!(
        has_image(&rig.snap()),
        "the parked alternate placement comes back with ?47h"
    );
}

#[test]
fn kitty_image_on_the_primary_does_not_show_on_the_alternate() {
    let mut rig = Rig::new();
    rig.feed(KITTY_IMAGE);
    assert!(has_image(&rig.snap()));

    rig.feed(&AltMode::M47.enter());
    assert!(!has_image(&rig.snap()));

    rig.feed(&AltMode::M47.leave());
    assert!(
        has_image(&rig.snap()),
        "the primary image survives the visit"
    );
}

#[test]
fn mode_1049_enter_clears_a_parked_alternate_image() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[3;3H");
    rig.feed(KITTY_IMAGE);
    assert!(has_image(&rig.snap()));
    rig.feed(&AltMode::M47.leave());
    let _ = rig.snap();

    rig.feed(&AltMode::M1049.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert!(
        !has_image(&alt),
        "?1049 enter clears the alternate's images"
    );
    assert_eq!(text(&alt), "");
}

#[test]
fn mode_1047_leave_clears_the_alternate_image() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M1047.enter());
    rig.feed(KITTY_IMAGE);
    assert!(has_image(&rig.snap()));
    rig.feed(&AltMode::M1047.leave());
    let _ = rig.snap();

    rig.feed(&AltMode::M1047.enter());
    assert!(
        !has_image(&rig.snap()),
        "?1047 leave cleared the alternate, images included"
    );
}

// ---------------------------------------------------------------------------
// RIS and DECSTR from the alternate screen
// ---------------------------------------------------------------------------

#[test]
fn ris_from_the_alternate_returns_to_a_blank_primary() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[5;5HALT");
    let _ = rig.snap();

    rig.feed("\x1bc");
    let snap = rig.snap();
    assert!(!snap.is_alternate_screen, "RIS leaves the alternate screen");
    assert_eq!(text(&snap), "", "RIS clears the primary");
    assert_eq!(cursor(&snap), (0, 0), "RIS homes the cursor");
}

#[test]
fn ris_drops_the_parked_alternate_so_a_later_47_entry_is_blank() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("ALT");
    let _ = rig.snap();
    rig.feed(&AltMode::M47.leave());
    let _ = rig.snap();

    rig.feed("\x1bc");
    rig.feed(&AltMode::M47.enter());
    let alt = rig.snap();
    assert!(alt.is_alternate_screen);
    assert_eq!(text(&alt), "", "RIS dropped the parked alternate contents");
}

#[test]
fn ris_from_the_alternate_drops_the_alternate_image_and_both_kitty_stacks() {
    let mut rig = Rig::new();
    rig.feed("\x1b[>1u");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[>4u");
    rig.feed(KITTY_IMAGE);
    assert!(has_image(&rig.snap()));

    rig.feed("\x1bc");
    let primary = rig.snap();
    assert!(!primary.is_alternate_screen);
    assert!(!has_image(&primary));
    assert_eq!(
        primary.kitty_keyboard_flags, 0,
        "RIS clears the primary stack"
    );

    rig.feed(&AltMode::M47.enter());
    let alt = rig.snap();
    assert!(!has_image(&alt), "the parked alternate image is gone");
    assert_eq!(
        alt.kitty_keyboard_flags, 0,
        "RIS clears the alternate stack"
    );
}

#[test]
fn decrqm_reports_reset_after_ris_from_the_alternate() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed(&mode.enter());
        rig.feed("\x1bc");
        assert_eq!(rig.decrqm(mode), mode.report(2), "{mode:?}");
    }
}

#[test]
fn decstr_on_the_alternate_stays_on_the_alternate_with_content_and_cursor() {
    let mut rig = Rig::new();
    rig.feed("PRIMARY");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[5;5HALT");
    let before = rig.snap();
    assert_eq!(cursor(&before), (7, 4));

    rig.feed("\x1b[!p");
    let after = rig.snap();
    assert!(
        after.is_alternate_screen,
        "DECSTR does not leave the alternate"
    );
    assert_eq!(text(&after), "ALT", "DECSTR does not clear the screen");
    assert_eq!(cursor(&after), (7, 4), "DECSTR does not move the cursor");
    assert_eq!(rig.decrqm(AltMode::M47), AltMode::M47.report(1));

    rig.feed(&AltMode::M47.leave());
    assert_eq!(text(&rig.snap()), "PRIMARY", "the primary is untouched too");
}

#[test]
fn decstr_on_the_alternate_clears_both_kitty_keyboard_stacks() {
    let mut rig = Rig::new();
    rig.feed("\x1b[>1u");
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[>4u");
    assert_eq!(rig.snap().kitty_keyboard_flags, 4);

    rig.feed("\x1b[!p");
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        0,
        "DECSTR clears the alternate stack"
    );

    rig.feed(&AltMode::M47.leave());
    assert_eq!(
        rig.snap().kitty_keyboard_flags,
        0,
        "DECSTR clears the primary stack"
    );
}

#[test]
fn decstr_on_the_alternate_homes_the_active_screens_saved_cursor() {
    let mut rig = Rig::new();
    rig.feed(&AltMode::M47.enter());
    rig.feed("\x1b[10;10H\x1b7\x1b[3;3H");

    rig.feed("\x1b[!p");
    assert_eq!(cursor(&rig.snap()), (2, 2), "DECSTR leaves the live cursor");

    rig.feed("\x1b8");
    assert_eq!(
        cursor(&rig.snap()),
        (0, 0),
        "DECSTR resets the active screen's saved cursor to home"
    );
}

// ---------------------------------------------------------------------------
// URL data per screen
// ---------------------------------------------------------------------------

/// The URLs the snapshot's visible tags carry, in order.
fn snapshot_urls(snap: &TerminalSnapshot) -> Vec<String> {
    snap.url_tag_indices
        .iter()
        .filter_map(|&i| snap.visible_tags.get(i))
        .filter_map(|tag| tag.url.as_ref().map(|u| u.url.clone()))
        .collect()
}

#[test]
fn primary_urls_do_not_appear_on_the_alternate_and_survive_the_round_trip() {
    for mode in AltMode::ALL {
        let mut rig = Rig::new();
        rig.feed("\x1b]8;;https://example.com/osc8\x1b\\link\x1b]8;;\x1b\\ plain");
        let primary = rig.snap();
        assert!(primary.has_urls, "{mode:?}: sanity: the link is visible");
        assert_eq!(
            snapshot_urls(&primary),
            ["https://example.com/osc8"],
            "{mode:?}: sanity"
        );

        rig.feed(&mode.enter());
        let alt = rig.snap();
        assert!(alt.is_alternate_screen);
        assert!(
            !alt.has_urls,
            "{mode:?}: the alternate snapshot carries no URL data"
        );
        assert!(
            alt.url_tag_indices.is_empty(),
            "{mode:?}: the alternate snapshot indexes no URL tags"
        );

        rig.feed(&mode.leave());
        let back = rig.snap();
        assert!(!back.is_alternate_screen);
        assert!(back.has_urls, "{mode:?}: the primary link survives");
        assert_eq!(
            snapshot_urls(&back),
            ["https://example.com/osc8"],
            "{mode:?}: the primary URL data is intact"
        );
        assert_eq!(text(&back), "linkplain");
    }
}

#[test]
fn auto_detected_primary_url_survives_an_alternate_round_trip() {
    let mut rig = Rig::new();
    rig.feed("see https://example.com/auto here");
    let primary = rig.snap();
    assert!(primary.has_urls, "sanity: the URL is auto-detected");
    let before = snapshot_urls(&primary);

    rig.feed(&AltMode::M47.enter());
    assert!(!rig.snap().has_urls);
    rig.feed(&AltMode::M47.leave());
    assert_eq!(snapshot_urls(&rig.snap()), before);
}
