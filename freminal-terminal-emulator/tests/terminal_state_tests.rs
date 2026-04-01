// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for [`TerminalState`] methods.
//!
//! Covers:
//! - `send_focus_event` (enabled/disabled, focused/unfocused)
//! - `write` (ASCII, Enter, closed channel)
//! - Alternate-screen scroll routing (ArrowUp/Down via `write`)
//! - Mode accessors: `is_normal_display`, `should_repeat_keys`, `skip_draw_always`

#![allow(clippy::unwrap_used)]

use crossbeam_channel::unbounded;
use freminal_common::{
    buffer_states::modes::{
        alternate_scroll::AlternateScroll, decarm::Decarm, decbkm::Decbkm, decscnm::Decscnm,
        keypad::KeypadMode, sync_updates::SynchronizedUpdates, xtmsewin::XtMseWin,
    },
    pty_write::PtyWrite,
};
use freminal_terminal_emulator::{
    interface::{KeyModifiers, TerminalInput},
    state::internal::TerminalState,
};

// ─── helpers ────────────────────────────────────────────────────────────────

/// Create a fresh [`TerminalState`] together with the matching `PtyWrite`
/// receiver.  Always keep the receiver alive for the duration of the test so
/// the channel is never disconnected.
fn make_state() -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
    let (tx, rx) = unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Drain all pending messages from `rx` and discard them.
///
/// Used before assertions on scroll tests because `handle_incoming_data` may
/// produce device-attribute responses or other write-backs on the channel
/// before the code under test runs.
fn drain(rx: &crossbeam_channel::Receiver<PtyWrite>) {
    while rx.try_recv().is_ok() {}
}

/// Unwrap a `PtyWrite::Write` variant and return the inner bytes.
///
/// Panics with a descriptive message if the variant is `Resize`.
fn unwrap_write(msg: PtyWrite) -> Vec<u8> {
    match msg {
        PtyWrite::Write(bytes) => bytes,
        PtyWrite::Resize(_) => panic!("expected PtyWrite::Write, got PtyWrite::Resize"),
    }
}

// ─── send_focus_event ────────────────────────────────────────────────────────

/// Enabling focus reporting and calling `send_focus_event(true)` must write
/// the focus-gained sequence `ESC [ I` to the PTY channel.
#[test]
fn test_send_focus_event_enabled_focused() {
    let (mut state, rx) = make_state();
    state.modes.focus_reporting = XtMseWin::Enabled;
    drain(&rx);

    state.send_focus_event(true);

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    assert_eq!(
        bytes, b"\x1b[I",
        "send_focus_event(true) must send ESC[I (focus gained)"
    );
    // No extra messages.
    assert!(rx.try_recv().is_err(), "only one message should be sent");
}

/// Enabling focus reporting and calling `send_focus_event(false)` must write
/// the focus-lost sequence `ESC [ O` to the PTY channel.
#[test]
fn test_send_focus_event_enabled_unfocused() {
    let (mut state, rx) = make_state();
    state.modes.focus_reporting = XtMseWin::Enabled;
    drain(&rx);

    state.send_focus_event(false);

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    assert_eq!(
        bytes, b"\x1b[O",
        "send_focus_event(false) must send ESC[O (focus lost)"
    );
    assert!(rx.try_recv().is_err(), "only one message should be sent");
}

/// When focus reporting is disabled (the default), `send_focus_event` must
/// send nothing to the PTY channel.
#[test]
fn test_send_focus_event_disabled() {
    let (mut state, rx) = make_state();
    // Default is XtMseWin::Disabled — no explicit assignment needed, but be
    // explicit to make the invariant clear.
    state.modes.focus_reporting = XtMseWin::Disabled;
    drain(&rx);

    state.send_focus_event(true);

    assert!(
        rx.try_recv().is_err(),
        "send_focus_event must not send anything when focus reporting is disabled"
    );
}

// ─── write ───────────────────────────────────────────────────────────────────

/// `write(Ascii(b'A'))` must succeed and deliver a single byte `0x41` (`'A'`).
#[test]
fn test_write_ascii() {
    let (state, rx) = make_state();
    drain(&rx);

    let result = state.write(&TerminalInput::Ascii(b'A'));
    assert!(result.is_ok(), "write(Ascii) must return Ok");

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    assert_eq!(bytes, vec![b'A'], "Ascii(b'A') must send the byte 0x41");
}

/// `write(Enter)` must succeed and deliver a carriage-return control byte
/// (`0x0D` — `Ctrl+M`).
#[test]
fn test_write_enter() {
    let (state, rx) = make_state();
    drain(&rx);

    let result = state.write(&TerminalInput::Enter);
    assert!(result.is_ok(), "write(Enter) must return Ok");

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    // TerminalInput::Enter → to_payload → Single(char_to_ctrl_code(b'm'))
    // char_to_ctrl_code(b'm') = b'm' & 0x1F = 0x0D (CR)
    assert_eq!(
        bytes,
        vec![0x0D],
        "Enter must send 0x0D (Ctrl+M / CR); got {bytes:?}"
    );
}

/// Dropping the receiver disconnects the channel.  A subsequent `write` call
/// must return `Err` instead of panicking.
#[test]
fn test_write_closed_channel() {
    let (tx, rx) = unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    // Drop the receiver to sever the channel.
    drop(rx);

    let result = state.write(&TerminalInput::Ascii(b'X'));
    assert!(
        result.is_err(),
        "write must return Err when the channel is disconnected"
    );
}

// ─── alternate-screen scroll routing ─────────────────────────────────────────

/// In alternate-screen mode, writing ArrowUp must send an ArrowUp escape
/// sequence to the PTY (CSI A when DECCKM is off, the default).
#[test]
fn test_alternate_screen_write_arrow_up() {
    let (mut state, rx) = make_state();

    // Enter alternate screen via DECSET 1049.
    state.handle_incoming_data(b"\x1b[?1049h");

    // Drain any device-attribute responses or other write-backs produced by
    // handle_incoming_data before exercising write.
    drain(&rx);

    // ArrowUp(NONE) in alternate screen → CSI A (DECCKM off by default).
    state
        .write(&TerminalInput::ArrowUp(KeyModifiers::NONE))
        .unwrap();

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    assert_eq!(
        bytes, b"\x1b[A",
        "write(ArrowUp) in alternate screen must send ESC[A; got {bytes:?}"
    );
    assert!(
        rx.try_recv().is_err(),
        "exactly one message should be sent for arrow up"
    );
}

/// In alternate-screen mode, writing ArrowDown must send an ArrowDown escape
/// sequence to the PTY (CSI B when DECCKM is off, the default).
#[test]
fn test_alternate_screen_write_arrow_down() {
    let (mut state, rx) = make_state();

    // Enter alternate screen via DECSET 1049.
    state.handle_incoming_data(b"\x1b[?1049h");
    drain(&rx);

    // ArrowDown(NONE) in alternate screen → CSI B (DECCKM off by default).
    state
        .write(&TerminalInput::ArrowDown(KeyModifiers::NONE))
        .unwrap();

    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    assert_eq!(
        bytes, b"\x1b[B",
        "write(ArrowDown) in alternate screen must send ESC[B; got {bytes:?}"
    );
    assert!(
        rx.try_recv().is_err(),
        "exactly one message should be sent for arrow down"
    );
}

// ─── is_normal_display ───────────────────────────────────────────────────────

/// The default state must report a normal (non-inverted) display.
#[test]
fn test_is_normal_display_default() {
    let (state, _rx) = make_state();
    assert!(
        state.is_normal_display(),
        "is_normal_display must be true in the default state"
    );
}

/// After setting `modes.invert_screen` to `Decscnm::ReverseDisplay`,
/// `is_normal_display` must return `false`.
#[test]
fn test_is_normal_display_after_reverse() {
    let (mut state, _rx) = make_state();
    state.modes.invert_screen = Decscnm::ReverseDisplay;
    assert!(
        !state.is_normal_display(),
        "is_normal_display must be false when invert_screen is ReverseDisplay"
    );
}

// ─── should_repeat_keys ──────────────────────────────────────────────────────

/// The default mode is `Decarm::RepeatKey`, so `should_repeat_keys` must be
/// `true` out of the box.
#[test]
fn test_should_repeat_keys_default() {
    let (state, _rx) = make_state();
    assert!(
        state.should_repeat_keys(),
        "should_repeat_keys must be true in the default state (Decarm::RepeatKey)"
    );
}

/// After setting `modes.repeat_keys` to `Decarm::NoRepeatKey`,
/// `should_repeat_keys` must return `false`.
#[test]
fn test_should_repeat_keys_disabled() {
    let (mut state, _rx) = make_state();
    state.modes.repeat_keys = Decarm::NoRepeatKey;
    assert!(
        !state.should_repeat_keys(),
        "should_repeat_keys must be false when repeat_keys is Decarm::NoRepeatKey"
    );
}

// ─── skip_draw_always ────────────────────────────────────────────────────────

/// The default mode is `SynchronizedUpdates::Draw`, so `skip_draw_always` must
/// be `false` out of the box.
#[test]
fn test_skip_draw_always_default() {
    let (state, _rx) = make_state();
    assert!(
        !state.skip_draw_always(),
        "skip_draw_always must be false in the default state (SynchronizedUpdates::Draw)"
    );
}

/// After setting `modes.synchronized_updates` to `SynchronizedUpdates::DontDraw`,
/// `skip_draw_always` must return `true`.
#[test]
fn test_skip_draw_always_enabled() {
    let (mut state, _rx) = make_state();
    state.modes.synchronized_updates = SynchronizedUpdates::DontDraw;
    assert!(
        state.skip_draw_always(),
        "skip_draw_always must be true when synchronized_updates is DontDraw"
    );
}

// ─── KeyModifiers constant used in scroll tests (compile-time guard) ─────────

/// Verify that the `KeyModifiers::NONE` constant is accessible from the
/// interface module — this guards the import path used by `scroll()`.
#[test]
fn test_key_modifiers_none_is_empty() {
    assert!(
        KeyModifiers::NONE.is_empty(),
        "KeyModifiers::NONE must report is_empty() == true"
    );
}

// ─── DECNKM (?66) — Numeric Keypad Mode ─────────────────────────────────────

/// Default keypad mode is Numeric.
#[test]
fn test_decnkm_default_is_numeric() {
    let (state, _rx) = make_state();
    assert_eq!(
        state.modes.keypad_mode,
        KeypadMode::Numeric,
        "Default keypad mode must be Numeric"
    );
}

/// `CSI ? 66 h` (DECSET) sets keypad to Application mode.
#[test]
fn test_decnkm_set_switches_to_application() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Send DECSET ?66
    state.handle_incoming_data(b"\x1b[?66h");
    assert_eq!(
        state.modes.keypad_mode,
        KeypadMode::Application,
        "DECSET ?66 must switch keypad to Application"
    );
}

/// `CSI ? 66 l` (DECRST) sets keypad back to Numeric mode.
#[test]
fn test_decnkm_reset_switches_to_numeric() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // First set to Application
    state.handle_incoming_data(b"\x1b[?66h");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    // Then reset to Numeric
    state.handle_incoming_data(b"\x1b[?66l");
    assert_eq!(
        state.modes.keypad_mode,
        KeypadMode::Numeric,
        "DECRST ?66 must switch keypad to Numeric"
    );
}

/// `CSI ? 66 h` (DECSET ?66) produces the same effect as `ESC =` (DECKPAM).
#[test]
fn test_decnkm_set_matches_deckpam() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Use ESC = (DECKPAM) first
    state.handle_incoming_data(b"\x1b=");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Application);

    // Reset via ESC > (DECKPNM)
    state.handle_incoming_data(b"\x1b>");
    assert_eq!(state.modes.keypad_mode, KeypadMode::Numeric);

    // Now use DECSET ?66 — same effect
    state.handle_incoming_data(b"\x1b[?66h");
    assert_eq!(
        state.modes.keypad_mode,
        KeypadMode::Application,
        "DECSET ?66 must produce the same effect as ESC ="
    );
}

/// DECRQM query for ?66 returns the correct DECRPM response.
#[test]
fn test_decnkm_decrqm_default_is_numeric() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Query in default (Numeric) state
    state.handle_incoming_data(b"\x1b[?66$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?66;2$y",
        "DECRQM ?66 in Numeric state must return Ps=2 (reset)"
    );
}

/// DECRQM query for ?66 after DECSET returns Ps=1 (set).
#[test]
fn test_decnkm_decrqm_after_set_is_application() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Set to Application
    state.handle_incoming_data(b"\x1b[?66h");

    // Drain any writes from the set operation
    drain(&rx);

    // Query
    state.handle_incoming_data(b"\x1b[?66$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?66;1$y",
        "DECRQM ?66 in Application state must return Ps=1 (set)"
    );
}

// ─── DECBKM (?67) — Backarrow Key Mode ──────────────────────────────────────

/// Default backarrow key mode is `BackarrowSendsBs` (set state).
#[test]
fn test_decbkm_default_is_bs() {
    let (state, _rx) = make_state();
    assert_eq!(
        state.modes.backarrow_key_mode,
        Decbkm::BackarrowSendsBs,
        "Default backarrow key mode must be BackarrowSendsBs"
    );
}

/// `CSI ? 67 h` (DECSET) sets backarrow to send BS.
#[test]
fn test_decbkm_set_sends_bs() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // First reset to DEL, then set back to BS
    state.handle_incoming_data(b"\x1b[?67l");
    assert_eq!(state.modes.backarrow_key_mode, Decbkm::BackarrowSendsDel);

    state.handle_incoming_data(b"\x1b[?67h");
    assert_eq!(
        state.modes.backarrow_key_mode,
        Decbkm::BackarrowSendsBs,
        "DECSET ?67 must switch backarrow to send BS"
    );
}

/// `CSI ? 67 l` (DECRST) sets backarrow to send DEL.
#[test]
fn test_decbkm_reset_sends_del() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?67l");
    assert_eq!(
        state.modes.backarrow_key_mode,
        Decbkm::BackarrowSendsDel,
        "DECRST ?67 must switch backarrow to send DEL"
    );
}

/// DECRQM query for ?67 returns Ps=1 in default (set/BS) state.
#[test]
fn test_decbkm_decrqm_default_is_set() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?67$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?67;1$y",
        "DECRQM ?67 in default (BS) state must return Ps=1 (set)"
    );
}

/// DECRQM query for ?67 after DECRST returns Ps=2 (reset/DEL).
#[test]
fn test_decbkm_decrqm_after_reset_is_del() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?67l");
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?67$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?67;2$y",
        "DECRQM ?67 in DEL state must return Ps=2 (reset)"
    );
}

// ─── Grapheme Clustering (?2027) — permanently set ───────────────────────────

/// DECRQM query for ?2027 always returns Ps=3 (permanently set).
#[test]
fn test_grapheme_clustering_decrqm_permanently_set() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?2027$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?2027;3$y",
        "DECRQM ?2027 must return Ps=3 (permanently set)"
    );
}

/// DECRQM query for ?2027 still returns Ps=3 after DECRST.
#[test]
fn test_grapheme_clustering_decrqm_after_reset_still_permanently_set() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // Send DECRST for ?2027 — silently accepted
    state.handle_incoming_data(b"\x1b[?2027l");
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?2027$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?2027;3$y",
        "DECRQM ?2027 must return Ps=3 even after DECRST"
    );
}

/// DECSET ?2027 is silently accepted (no error, no state change).
#[test]
fn test_grapheme_clustering_set_silently_accepted() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // DECSET ?2027 — should not produce any PTY output
    state.handle_incoming_data(b"\x1b[?2027h");
    // No DECRPM response expected for DECSET
    assert!(
        rx.try_recv().is_err(),
        "DECSET ?2027 must not produce a PTY response"
    );
}

// ─── Alternate Scroll Mode (?1007) ──────────────────────────────────────────

/// Default alternate scroll mode is `Disabled` (reset state).
#[test]
fn test_alternate_scroll_default_is_disabled() {
    let (state, _rx) = make_state();
    assert_eq!(
        state.modes.alternate_scroll,
        AlternateScroll::Disabled,
        "Default alternate scroll mode must be Disabled"
    );
}

/// `CSI ? 1007 h` (DECSET) enables alternate scroll mode.
#[test]
fn test_alternate_scroll_set_enables() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1007h");
    assert_eq!(
        state.modes.alternate_scroll,
        AlternateScroll::Enabled,
        "DECSET ?1007 must enable alternate scroll mode"
    );
}

/// `CSI ? 1007 l` (DECRST) disables alternate scroll mode.
#[test]
fn test_alternate_scroll_reset_disables() {
    let (mut state, rx) = make_state();
    drain(&rx);

    // First enable, then reset
    state.handle_incoming_data(b"\x1b[?1007h");
    assert_eq!(state.modes.alternate_scroll, AlternateScroll::Enabled);

    state.handle_incoming_data(b"\x1b[?1007l");
    assert_eq!(
        state.modes.alternate_scroll,
        AlternateScroll::Disabled,
        "DECRST ?1007 must disable alternate scroll mode"
    );
}

/// DECRQM query for ?1007 returns Ps=2 in default (reset/disabled) state.
#[test]
fn test_alternate_scroll_decrqm_default_is_reset() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1007$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?1007;2$y",
        "DECRQM ?1007 in default (disabled) state must return Ps=2 (reset)"
    );
}

/// DECRQM query for ?1007 after DECSET returns Ps=1 (set/enabled).
#[test]
fn test_alternate_scroll_decrqm_after_set_is_enabled() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1007h");
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1007$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?1007;1$y",
        "DECRQM ?1007 after DECSET must return Ps=1 (set)"
    );
}

// ─── Hilite Mouse Tracking (?1001) ──────────────────────────────────────────

/// `CSI ? 1001 h` (DECSET) sets mouse tracking to `XtMseHilite`.
#[test]
fn test_hilite_mouse_tracking_set() {
    use freminal_common::buffer_states::modes::mouse::MouseTrack;
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1001h");
    assert_eq!(
        state.modes.mouse_tracking,
        MouseTrack::XtMseHilite,
        "DECSET ?1001 must set mouse tracking to XtMseHilite"
    );
}

/// `CSI ? 1001 l` (DECRST) resets mouse tracking to `NoTracking`.
#[test]
fn test_hilite_mouse_tracking_reset() {
    use freminal_common::buffer_states::modes::mouse::MouseTrack;
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1001h");
    state.handle_incoming_data(b"\x1b[?1001l");
    assert_eq!(
        state.modes.mouse_tracking,
        MouseTrack::NoTracking,
        "DECRST ?1001 must reset mouse tracking to NoTracking"
    );
}

/// DECRQM `?1001` returns Ps=2 in default (no-tracking) state.
#[test]
fn test_hilite_mouse_tracking_decrqm_default_is_reset() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1001$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?1001;2$y",
        "DECRQM ?1001 in default (no-tracking) state must return Ps=2 (reset)"
    );
}

/// DECRQM `?1001` returns Ps=1 after DECSET.
#[test]
fn test_hilite_mouse_tracking_decrqm_after_set() {
    let (mut state, rx) = make_state();
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1001h");
    drain(&rx);

    state.handle_incoming_data(b"\x1b[?1001$p");
    let msg = rx.try_recv().unwrap();
    let bytes = unwrap_write(msg);
    let resp = String::from_utf8(bytes).unwrap();
    assert_eq!(
        resp, "\x1b[?1001;1$y",
        "DECRQM ?1001 after DECSET must return Ps=1 (set)"
    );
}
