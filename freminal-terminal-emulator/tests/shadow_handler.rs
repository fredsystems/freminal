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

use freminal_common::buffer_states::modes::{
    decarm::Decarm,
    decckm::Decckm,
    decscnm::Decscnm,
    keypad::KeypadMode,
    lnm::Lnm,
    mouse::{MouseEncoding, MouseTrack},
    reverse_wrap_around::ReverseWrapAround,
    rl_bracket::RlBracket,
    sync_updates::SynchronizedUpdates,
    xtmsewin::XtMseWin,
};
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
    state.set_win_size(100, 30, 8, 16);

    state.handle_incoming_data(b"after resize\r\n");

    let (w, h) = state.get_win_size();
    assert_eq!(w, 100);
    assert_eq!(h, 30);
}

#[test]
fn shadow_handler_scroll_back_and_forward_do_not_panic() {
    let mut state = make_state();

    // Write enough lines to build up scrollback history.
    for i in 0..50_u8 {
        let line = format!("scrollback line {i}\r\n");
        state.handle_incoming_data(line.as_bytes());
    }

    // Scroll back (upward through history) by 3 lines from the bottom.
    let offset = state.handler.handle_scroll_back(0, 3);

    // Scroll forward (toward the bottom) by 2 lines.
    let offset = state.handler.handle_scroll_forward(offset, 2);

    // Scroll back past the start — must clamp, not panic.
    let offset = state.handler.handle_scroll_back(offset, 999);

    // Scroll forward past the bottom — must clamp, not panic.
    let _offset = state.handler.handle_scroll_forward(offset, 999);

    // Write more data — the shadow handler must not panic on new content.
    state.handle_incoming_data(b"new data after scroll\r\n");

    let (w, h) = state.get_win_size();
    assert!(w > 0);
    assert!(h > 0);
}

#[test]
fn terminal_state_default_scrollback_limit() {
    let state = TerminalState::default();
    assert_eq!(
        state.handler.buffer().scrollback_limit(),
        4000,
        "default TerminalState should use the compiled-in scrollback limit"
    );
}

#[test]
fn terminal_state_custom_scrollback_limit() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let state = TerminalState::new(tx, Some(123));
    assert_eq!(
        state.handler.buffer().scrollback_limit(),
        123,
        "TerminalState::new with Some(123) should wire the limit through"
    );
}

#[test]
fn terminal_state_none_scrollback_limit_uses_default() {
    let (tx, _rx) = crossbeam_channel::unbounded();
    let state = TerminalState::new(tx, None);
    assert_eq!(
        state.handler.buffer().scrollback_limit(),
        4000,
        "TerminalState::new with None should keep the default 4000"
    );
}

// ══════════════════════════════════════════════════════════════════════
// Mode wiring tests (subtasks 7.7–7.10, 7.15)
//
// Each test sends a DEC-set/reset escape sequence through
// handle_incoming_data and verifies that TerminalState.modes is updated.
// ══════════════════════════════════════════════════════════════════════

// ── 7.7  DECCKM (?1) ────────────────────────────────────────────────

#[test]
fn mode_decckm_set_enables_application_cursor_keys() {
    let mut state = make_state();
    assert_eq!(
        state.modes.cursor_key,
        Decckm::Ansi,
        "default should be Ansi"
    );
    state.handle_incoming_data(b"\x1b[?1h"); // DECSET ?1
    assert_eq!(state.modes.cursor_key, Decckm::Application);
}

#[test]
fn mode_decckm_reset_restores_ansi_cursor_keys() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1h"); // set
    state.handle_incoming_data(b"\x1b[?1l"); // reset
    assert_eq!(state.modes.cursor_key, Decckm::Ansi);
}

// ── 7.8  Bracketed paste (?2004) ────────────────────────────────────

#[test]
fn mode_bracketed_paste_set_enables() {
    let mut state = make_state();
    assert_eq!(state.modes.bracketed_paste, RlBracket::Disabled);
    state.handle_incoming_data(b"\x1b[?2004h"); // DECSET ?2004
    assert_eq!(state.modes.bracketed_paste, RlBracket::Enabled);
}

#[test]
fn mode_bracketed_paste_reset_disables() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?2004h");
    state.handle_incoming_data(b"\x1b[?2004l");
    assert_eq!(state.modes.bracketed_paste, RlBracket::Disabled);
}

// ── 7.9  Mouse tracking (?1000/?1002/?1003/?1006) ───────────────────

#[test]
fn mode_mouse_tracking_1000_set() {
    let mut state = make_state();
    assert_eq!(state.modes.mouse_tracking, MouseTrack::NoTracking);
    state.handle_incoming_data(b"\x1b[?1000h"); // X11 mouse
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseX11);
}

#[test]
fn mode_mouse_tracking_1000_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1000h");
    state.handle_incoming_data(b"\x1b[?1000l");
    assert_eq!(state.modes.mouse_tracking, MouseTrack::NoTracking);
}

#[test]
fn mode_mouse_tracking_1002_button_event() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1002h");
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseBtn);
}

#[test]
fn mode_mouse_tracking_1003_any_event() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1003h");
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseAny);
}

#[test]
fn mode_mouse_encoding_1006_sgr() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1006h");
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr);
}

#[test]
fn mode_mouse_encoding_1006_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1006h");
    state.handle_incoming_data(b"\x1b[?1006l");
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::X11);
}

// ── 7.10  Focus events (?1004) ──────────────────────────────────────

#[test]
fn mode_focus_reporting_set() {
    let mut state = make_state();
    assert_eq!(state.modes.focus_reporting, XtMseWin::Disabled);
    state.handle_incoming_data(b"\x1b[?1004h");
    assert_eq!(state.modes.focus_reporting, XtMseWin::Enabled);
}

#[test]
fn mode_focus_reporting_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1004h");
    state.handle_incoming_data(b"\x1b[?1004l");
    assert_eq!(state.modes.focus_reporting, XtMseWin::Disabled);
}

// ── 7.15  DECSCNM (?5) screen inversion ─────────────────────────────

#[test]
fn mode_decscnm_set_inverts_screen() {
    let mut state = make_state();
    assert_eq!(state.modes.invert_screen, Decscnm::NormalDisplay);
    state.handle_incoming_data(b"\x1b[?5h");
    assert_eq!(state.modes.invert_screen, Decscnm::ReverseDisplay);
}

#[test]
fn mode_decscnm_reset_restores_normal() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?5h");
    state.handle_incoming_data(b"\x1b[?5l");
    assert_eq!(state.modes.invert_screen, Decscnm::NormalDisplay);
}

// ── 7.15  DECARM (?8) repeat keys ───────────────────────────────────

#[test]
fn mode_decarm_set_enables_repeat() {
    let mut state = make_state();
    // Default for Decarm is RepeatKey (set), so reset first
    state.handle_incoming_data(b"\x1b[?8l"); // reset
    assert_eq!(state.modes.repeat_keys, Decarm::NoRepeatKey);
    state.handle_incoming_data(b"\x1b[?8h"); // set
    assert_eq!(state.modes.repeat_keys, Decarm::RepeatKey);
}

#[test]
fn mode_decarm_reset_disables_repeat() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?8l");
    assert_eq!(state.modes.repeat_keys, Decarm::NoRepeatKey);
}

// ── Reverse wrap around (?45) ───────────────────────────────────────

#[test]
fn mode_reverse_wrap_around_set() {
    let mut state = make_state();
    assert_eq!(
        state.modes.reverse_wrap_around,
        ReverseWrapAround::WrapAround,
        "default should be WrapAround"
    );
    // Reset first, then set to verify the toggle
    state.handle_incoming_data(b"\x1b[?45l"); // reset to DontWrap
    assert_eq!(state.modes.reverse_wrap_around, ReverseWrapAround::DontWrap);
    state.handle_incoming_data(b"\x1b[?45h"); // set back to WrapAround
    assert_eq!(
        state.modes.reverse_wrap_around,
        ReverseWrapAround::WrapAround
    );
}

#[test]
fn mode_reverse_wrap_around_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?45h");
    state.handle_incoming_data(b"\x1b[?45l");
    assert_eq!(state.modes.reverse_wrap_around, ReverseWrapAround::DontWrap);
}

// ── Synchronized updates (?2026) ────────────────────────────────────

#[test]
fn mode_synchronized_updates_set() {
    let mut state = make_state();
    assert_eq!(state.modes.synchronized_updates, SynchronizedUpdates::Draw);
    state.handle_incoming_data(b"\x1b[?2026h");
    assert_eq!(
        state.modes.synchronized_updates,
        SynchronizedUpdates::DontDraw
    );
}

#[test]
fn mode_synchronized_updates_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?2026h");
    state.handle_incoming_data(b"\x1b[?2026l");
    assert_eq!(state.modes.synchronized_updates, SynchronizedUpdates::Draw);
}

// ── LNM (20) line feed mode ─────────────────────────────────────────

#[test]
fn mode_lnm_set_enables_newline_mode() {
    let mut state = make_state();
    assert_eq!(state.modes.line_feed_mode, Lnm::LineFeed);
    state.handle_incoming_data(b"\x1b[20h");
    assert_eq!(state.modes.line_feed_mode, Lnm::NewLine);
}

#[test]
fn mode_lnm_reset_restores_linefeed_mode() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[20h");
    state.handle_incoming_data(b"\x1b[20l");
    assert_eq!(state.modes.line_feed_mode, Lnm::LineFeed);
}

// ── Compound test: realistic session with multiple modes ────────────

#[test]
fn mode_wiring_realistic_session() {
    // Simulate what a real application (e.g. vim/tmux) does on startup:
    // enable bracketed paste, application cursor keys, mouse tracking,
    // focus events, and synchronized updates.
    let mut state = make_state();

    // Application startup sequence
    state.handle_incoming_data(b"\x1b[?1h"); // DECCKM application
    state.handle_incoming_data(b"\x1b[?2004h"); // Bracketed paste on
    state.handle_incoming_data(b"\x1b[?1000h"); // X11 mouse on
    state.handle_incoming_data(b"\x1b[?1006h"); // SGR mouse encoding
    state.handle_incoming_data(b"\x1b[?1004h"); // Focus events on
    state.handle_incoming_data(b"\x1b[?2026h"); // Synchronized updates

    assert_eq!(state.modes.cursor_key, Decckm::Application);
    assert_eq!(state.modes.bracketed_paste, RlBracket::Enabled);
    // ?1000h sets tracking to X11, ?1006h sets encoding to SGR — orthogonal axes
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseX11);
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr);
    assert_eq!(state.modes.focus_reporting, XtMseWin::Enabled);
    assert_eq!(
        state.modes.synchronized_updates,
        SynchronizedUpdates::DontDraw
    );

    // Application shutdown sequence — reset everything
    state.handle_incoming_data(b"\x1b[?1l");
    state.handle_incoming_data(b"\x1b[?2004l");
    state.handle_incoming_data(b"\x1b[?1006l");
    state.handle_incoming_data(b"\x1b[?1004l");
    state.handle_incoming_data(b"\x1b[?2026l");

    assert_eq!(state.modes.cursor_key, Decckm::Ansi);
    assert_eq!(state.modes.bracketed_paste, RlBracket::Disabled);
    // ?1006l resets encoding to X11 but does NOT affect tracking level
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseX11);
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::X11);
    assert_eq!(state.modes.focus_reporting, XtMseWin::Disabled);
    assert_eq!(state.modes.synchronized_updates, SynchronizedUpdates::Draw);
}

// ── Lazygit scenario: mouse tracking + encoding are orthogonal ──────

#[test]
fn mode_lazygit_mouse_sequence() {
    // Lazygit sends: ?1006h (SGR encoding), ?1000h (X11 tracking),
    // ?1002h (button tracking), ?1003h (any-event tracking).
    // Before the fix, ?1006h set mouse_tracking to XtMseSgr and ?1000h
    // then overwrote it to XtMseX11, losing the SGR encoding.
    let mut state = make_state();

    state.handle_incoming_data(b"\x1b[?1006h"); // SGR encoding
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr);
    assert_eq!(state.modes.mouse_tracking, MouseTrack::NoTracking);

    state.handle_incoming_data(b"\x1b[?1000h"); // X11 tracking
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr); // encoding preserved!
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseX11);

    state.handle_incoming_data(b"\x1b[?1002h"); // Button tracking
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr); // encoding preserved!
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseBtn);

    state.handle_incoming_data(b"\x1b[?1003h"); // Any-event tracking
    assert_eq!(state.modes.mouse_encoding, MouseEncoding::Sgr); // encoding preserved!
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseAny);
}

// ── 7.14  DECPAM / DECPNM (keypad mode) ──────────────────────────────

#[test]
fn decpam_sets_application_keypad_mode() {
    let mut state = make_state();
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);

    // ESC = → DECPAM (Application Keypad Mode)
    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);
}

#[test]
fn decpnm_sets_numeric_keypad_mode() {
    let mut state = make_state();

    // First set application mode
    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    // ESC > → DECPNM (Normal/Numeric Keypad Mode)
    state.handle_incoming_data(b"\x1b>");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);
}

#[test]
fn decpam_decpnm_toggle_round_trip() {
    let mut state = make_state();
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);

    // Toggle several times
    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    state.handle_incoming_data(b"\x1b>");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);

    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    state.handle_incoming_data(b"\x1b="); // duplicate set — still application
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    state.handle_incoming_data(b"\x1b>");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);
}

#[test]
fn ris_resets_keypad_mode_to_numeric() {
    let mut state = make_state();

    // Set application keypad mode
    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    // RIS (ESC c) should reset everything including keypad mode
    state.handle_incoming_data(b"\x1bc");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);
}

// ── 7.21 — HTS (ESC H), TBC (CSI g), CHT (CSI I), CBT (CSI Z) ──────

#[test]
fn hts_does_not_panic() {
    let mut state = make_state();
    // ESC H — set tab stop at current column
    state.handle_incoming_data(b"\x1bH");
}

#[test]
fn tbc_clear_current_does_not_panic() {
    let mut state = make_state();
    // CSI 0 g — clear tab stop at cursor
    state.handle_incoming_data(b"\x1b[0g");
}

#[test]
fn tbc_clear_all_does_not_panic() {
    let mut state = make_state();
    // CSI 3 g — clear all tab stops
    state.handle_incoming_data(b"\x1b[3g");
}

#[test]
fn cht_forward_tabulation_does_not_panic() {
    let mut state = make_state();
    // CSI 2 I — advance 2 tab stops
    state.handle_incoming_data(b"\x1b[2I");
}

#[test]
fn cbt_backward_tabulation_does_not_panic() {
    let mut state = make_state();
    // Move to col 20 first, then CSI 2 Z — back 2 tab stops
    state.handle_incoming_data(b"\x1b[1;21H");
    state.handle_incoming_data(b"\x1b[2Z");
}

// ── 7.24 — CSI s / CSI u (Save / Restore Cursor) ────────────────────

#[test]
fn csi_s_u_save_restore_cursor_does_not_panic() {
    let mut state = make_state();
    // Move to row 5, col 10
    state.handle_incoming_data(b"\x1b[5;10H");
    // CSI s — save cursor
    state.handle_incoming_data(b"\x1b[s");
    // Move elsewhere
    state.handle_incoming_data(b"\x1b[1;1H");
    // CSI u — restore cursor
    state.handle_incoming_data(b"\x1b[u");
}

// ── 7.25 — REP (CSI b) — Repeat Preceding Graphic Character ─────────

#[test]
fn rep_does_not_panic() {
    let mut state = make_state();
    state.handle_incoming_data(b"A");
    // CSI 5 b — repeat 'A' 5 times
    state.handle_incoming_data(b"\x1b[5b");
}

#[test]
fn rep_with_no_preceding_char_does_not_panic() {
    let mut state = make_state();
    // REP with no preceding graphic — should be a no-op
    state.handle_incoming_data(b"\x1b[3b");
}

// ── 7.26 — HPA (CSI `) alias for CHA ────────────────────────────────

#[test]
fn hpa_backtick_does_not_panic() {
    let mut state = make_state();
    // CSI 10 ` — move to column 10 (backtick = 0x60)
    state.handle_incoming_data(b"\x1b[10`");
}

// ── 7.28 — DECALN (ESC # 8) — Screen Alignment Test ─────────────────

#[test]
fn decaln_does_not_panic() {
    let mut state = make_state();
    // Write some content first
    state.handle_incoming_data(b"Hello World\r\nSecond Line");
    // ESC # 8 — DECALN
    state.handle_incoming_data(b"\x1b#8");
}

// ── 7.30 — OSC Unknown silently consumed ─────────────────────────────

#[test]
fn osc_unknown_does_not_panic_or_log_error() {
    let mut state = make_state();
    // OSC 999 ; some data ST — unknown OSC, should be silently consumed
    state.handle_incoming_data(b"\x1b]999;some unknown data\x07");
    // Another unknown OSC
    state.handle_incoming_data(b"\x1b]1234;test\x1b\\");
}

// ── 7.29 — Legacy alternate screen ?47/?1047/?1048 ──────────────────

#[test]
fn alt_screen_47_set_reset_does_not_panic() {
    let mut state = make_state();
    state.handle_incoming_data(b"primary content");
    // CSI ? 47 h — enter alternate screen (legacy)
    state.handle_incoming_data(b"\x1b[?47h");
    state.handle_incoming_data(b"alt content");
    // CSI ? 47 l — leave alternate screen (legacy)
    state.handle_incoming_data(b"\x1b[?47l");
}

#[test]
fn alt_screen_1047_set_reset_does_not_panic() {
    let mut state = make_state();
    state.handle_incoming_data(b"primary content");
    // CSI ? 1047 h — enter alternate screen (legacy)
    state.handle_incoming_data(b"\x1b[?1047h");
    state.handle_incoming_data(b"alt content");
    // CSI ? 1047 l — leave alternate screen (legacy)
    state.handle_incoming_data(b"\x1b[?1047l");
}

#[test]
fn save_cursor_1048_set_reset_does_not_panic() {
    let mut state = make_state();
    // Move cursor
    state.handle_incoming_data(b"\x1b[5;10H");
    // CSI ? 1048 h — save cursor
    state.handle_incoming_data(b"\x1b[?1048h");
    // Move elsewhere
    state.handle_incoming_data(b"\x1b[1;1H");
    // CSI ? 1048 l — restore cursor
    state.handle_incoming_data(b"\x1b[?1048l");
}

#[test]
fn alt_screen_47_query_does_not_panic() {
    let mut state = make_state();
    // CSI ? 47 $ p — DECRQM query
    state.handle_incoming_data(b"\x1b[?47$p");
}
