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
    decarm::Decarm, decckm::Decckm, decscnm::Decscnm, keypad::KeypadMode, lnm::Lnm,
    mouse::MouseTrack, reverse_wrap_around::ReverseWrapAround, rl_bracket::RlBracket,
    sync_updates::SynchronizedUpdates, xtmsewin::XtMseWin,
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
    state.set_win_size(100, 30);

    state.handle_incoming_data(b"after resize\r\n");

    let (w, h) = state.get_win_size();
    assert_eq!(w, 100);
    assert_eq!(h, 30);
}

#[test]
fn shadow_handler_scroll_does_not_panic() {
    let mut state = make_state();

    // Write enough lines to build up scrollback history.
    for i in 0..50_u8 {
        let line = format!("scrollback line {i}\r\n");
        state.handle_incoming_data(line.as_bytes());
    }

    // Scroll up (backward through history) — positive value.
    state.scroll(3.0);

    // Scroll down (forward toward the bottom) — negative value.
    state.scroll(-2.0);

    // Scroll up again past the start (should clamp, not panic).
    state.scroll(999.0);

    // Scroll down past the bottom (should clamp, not panic).
    state.scroll(-999.0);

    // Write more data — the shadow handler must reset to the bottom so new
    // content is visible again.
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
fn mode_mouse_tracking_1006_sgr() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1006h");
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseSgr);
}

#[test]
fn mode_mouse_tracking_1006_reset() {
    let mut state = make_state();
    state.handle_incoming_data(b"\x1b[?1006h");
    state.handle_incoming_data(b"\x1b[?1006l");
    assert_eq!(state.modes.mouse_tracking, MouseTrack::NoTracking);
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
    // Note: ?1006 overwrites ?1000 since both are MouseMode variants
    assert_eq!(state.modes.mouse_tracking, MouseTrack::XtMseSgr);
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
    assert_eq!(state.modes.mouse_tracking, MouseTrack::NoTracking);
    assert_eq!(state.modes.focus_reporting, XtMseWin::Disabled);
    assert_eq!(state.modes.synchronized_updates, SynchronizedUpdates::Draw);
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
