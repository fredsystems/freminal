// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end DECRPM (DEC Private Mode Report) integration tests.
//!
//! These tests exercise the full pipeline: raw escape bytes → parser →
//! `TerminalState::handle_incoming_data` → DECRPM response on the PTY
//! write channel.  They cover modes owned by `TerminalState` (synced in
//! the mode-sync loop in `internal.rs`), complementing the handler-owned
//! mode query tests in `freminal-buffer/tests/terminal_handler_integration.rs`.

use crossbeam_channel::Receiver;
use freminal_common::config::ThemeMode;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

/// Create a `TerminalState` and return it along with the PTY write receiver
/// so we can inspect DECRPM responses.
fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let state = TerminalState::new(tx, None);
    (state, rx)
}

/// Drain all pending `PtyWrite::Write` messages from the channel, concatenate
/// the bytes, and return as a `String`.  Non-`Write` variants are ignored.
fn drain_pty_writes(rx: &Receiver<PtyWrite>) -> String {
    let mut buf = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    String::from_utf8(buf).expect("PTY responses must be valid UTF-8")
}

/// Feed raw bytes and return the concatenated PTY response.
fn feed_and_collect(state: &mut TerminalState, rx: &Receiver<PtyWrite>, input: &[u8]) -> String {
    // Drain any prior writes (e.g. from mode-set producing DA responses)
    let _ = drain_pty_writes(rx);
    state.handle_incoming_data(input);
    drain_pty_writes(rx)
}

// ═══════════════════════════════════════════════════════════════════════════
// DECCKM (?1)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decckm_default_is_reset() {
    let (mut state, rx) = make_state();
    // DECRQM for ?1: ESC[?1$p
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp, "\x1b[?1;2$y", "DECCKM default (Ansi) → Ps=2 (reset)");
}

#[test]
fn decrpm_decckm_after_enable() {
    let (mut state, rx) = make_state();
    // Enable DECCKM: ESC[?1h
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1h");
    // Query
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(
        resp, "\x1b[?1;1$y",
        "DECCKM after enable (Application) → Ps=1 (set)"
    );
}

#[test]
fn decrpm_decckm_enable_then_disable() {
    let (mut state, rx) = make_state();
    // Enable then disable
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1h");
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?1l");
    // Query
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp, "\x1b[?1;2$y", "DECCKM after disable → Ps=2 (reset)");
}

// ═══════════════════════════════════════════════════════════════════════════
// Bracketed Paste (?2004)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_bracketed_paste_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp, "\x1b[?2004;2$y",
        "Bracketed paste default → Ps=2 (reset)"
    );
}

#[test]
fn decrpm_bracketed_paste_after_enable() {
    let (mut state, rx) = make_state();
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2004h");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp, "\x1b[?2004;1$y",
        "Bracketed paste after enable → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DECSCNM (?5)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decscnm_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?5$p");
    assert_eq!(
        resp, "\x1b[?5;2$y",
        "DECSCNM default (normal display) → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// DECARM (?8)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decarm_default_is_set() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?8$p");
    assert_eq!(
        resp, "\x1b[?8;1$y",
        "DECARM default (repeat keys) → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// XtMseWin / Focus Events (?1004)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_xtmsewin_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?1004$p");
    assert_eq!(
        resp, "\x1b[?1004;2$y",
        "XtMseWin default (disabled) → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Reverse Wrap Around (?45)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_reverse_wrap_around_default_is_set() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?45$p");
    assert_eq!(
        resp, "\x1b[?45;1$y",
        "Reverse wrap around default → Ps=1 (set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Synchronized Updates (?2026)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_synchronized_updates_default_is_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2026$p");
    assert_eq!(
        resp, "\x1b[?2026;2$y",
        "Synchronized updates default → Ps=2 (reset)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: set mode, query, verify response
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_integration_set_then_query_multiple_modes() {
    let (mut state, rx) = make_state();

    // Enable bracketed paste and DECCKM
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2004h\x1b[?1h");

    // Query both — send as separate sequences
    let resp1 = feed_and_collect(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(
        resp1, "\x1b[?2004;1$y",
        "Bracketed paste must report set after enable"
    );

    let resp2 = feed_and_collect(&mut state, &rx, b"\x1b[?1$p");
    assert_eq!(resp2, "\x1b[?1;1$y", "DECCKM must report set after enable");
}

// ═══════════════════════════════════════════════════════════════════════════
// ?2031 Theming — DECRPM queries
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_theming_default_dark_locked() {
    // Default theme_mode is Dark → Ps=2 (permanently reset / dark locked)
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;2$y",
        "Default Dark theme_mode → Ps=2 (permanently reset)"
    );
}

#[test]
fn decrpm_theming_light_locked() {
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Light;
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;1$y",
        "Light theme_mode → Ps=1 (permanently set)"
    );
}

#[test]
fn decrpm_theming_auto_dark_active() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Auto;
    state.modes.theming = Theming::Dark;
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;4$y",
        "Auto mode with dark active → Ps=4 (temporarily reset)"
    );
}

#[test]
fn decrpm_theming_auto_light_active() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Auto;
    state.modes.theming = Theming::Light;
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;3$y",
        "Auto mode with light active → Ps=3 (temporarily set)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// ?2031 Theming — DECSET/DECRST honoured only when Auto
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn theming_decset_ignored_when_dark_locked() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    // Default is Dark (locked).  DECSET ?2031 should be ignored.
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031h");
    assert_eq!(
        state.modes.theming,
        Theming::Light,
        "Theming state must not change when theme_mode is Dark (locked)"
    );
}

#[test]
fn theming_decrst_ignored_when_light_locked() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Light;
    state.modes.theming = Theming::Light;
    // DECRST ?2031 should be ignored when locked to Light.
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031l");
    assert_eq!(
        state.modes.theming,
        Theming::Light,
        "Theming state must not change when theme_mode is Light (locked)"
    );
}

#[test]
fn theming_decset_honoured_when_auto() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Auto;
    state.modes.theming = Theming::Dark;
    // DECSET ?2031 → switch to Light
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031h");
    assert_eq!(
        state.modes.theming,
        Theming::Light,
        "DECSET ?2031 should switch to Light when theme_mode is Auto"
    );
}

#[test]
fn theming_decrst_honoured_when_auto() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Auto;
    state.modes.theming = Theming::Light;
    // DECRST ?2031 → switch to Dark
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031l");
    assert_eq!(
        state.modes.theming,
        Theming::Dark,
        "DECRST ?2031 should switch to Dark when theme_mode is Auto"
    );
}

#[test]
fn theming_set_then_query_auto_mode() {
    use freminal_common::buffer_states::modes::theme::Theming;
    let (mut state, rx) = make_state();
    state.modes.theme_mode = ThemeMode::Auto;
    state.modes.theming = Theming::Dark;

    // DECSET ?2031 → switch to Light, then query
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031h");
    assert_eq!(state.modes.theming, Theming::Light);

    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;3$y",
        "After DECSET in Auto mode → Ps=3 (temporarily set / light)"
    );

    // DECRST ?2031 → switch back to Dark, then query
    let _ = feed_and_collect(&mut state, &rx, b"\x1b[?2031l");
    assert_eq!(state.modes.theming, Theming::Dark);

    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?2031$p");
    assert_eq!(
        resp, "\x1b[?2031;4$y",
        "After DECRST in Auto mode → Ps=4 (temporarily reset / dark)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// C1 framing of replies (S8C1T / S7C1T)
//
// Every DECRPM and the `CSI ? u` reply is framed by the handler, so the
// introducer follows the S8C1T state regardless of which layer owns the mode.
// ═══════════════════════════════════════════════════════════════════════════

/// Feed raw bytes and return the concatenated PTY response as raw bytes
/// (8-bit replies contain `0x9B`, which is not valid UTF-8 on its own).
fn feed_and_collect_bytes(
    state: &mut TerminalState,
    rx: &Receiver<PtyWrite>,
    input: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    buf.clear();
    state.handle_incoming_data(input);
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    buf
}

/// `true` if `haystack` contains the two-byte 7-bit CSI introducer `ESC [`.
fn contains_7bit_csi(haystack: &[u8]) -> bool {
    haystack.windows(2).any(|w| w == b"\x1b[")
}

/// DECRQM queries covering a handler-owned mode (`?7`), a `TerminalState`-owned
/// mode (`?2004`) and `?2031`, with the 7-bit replies they must produce.
const FRAMING_QUERIES: [(&[u8], &[u8]); 3] = [
    (b"\x1b[?7$p", b"\x1b[?7;1$y"),
    (b"\x1b[?2004$p", b"\x1b[?2004;2$y"),
    (b"\x1b[?2031$p", b"\x1b[?2031;2$y"),
];

#[test]
fn decrpm_in_8bit_mode_uses_0x9b_and_no_esc_bracket() {
    for (query, seven_bit) in FRAMING_QUERIES {
        let (mut state, rx) = make_state();
        state.handle_incoming_data(b"\x1b G"); // S8C1T
        let resp = feed_and_collect_bytes(&mut state, &rx, query);

        // The 8-bit reply is the 7-bit reply with `ESC [` collapsed to 0x9B.
        let mut expected = vec![0x9B];
        expected.extend_from_slice(&seven_bit[2..]);
        assert_eq!(
            resp,
            expected,
            "query {:?}: expected 8-bit framed reply, got {resp:?}",
            String::from_utf8_lossy(query)
        );
        assert_eq!(resp.first(), Some(&0x9B));
        assert!(
            !contains_7bit_csi(&resp),
            "query {:?}: reply must not contain ESC [, got {resp:?}",
            String::from_utf8_lossy(query)
        );
    }
}

#[test]
fn decrpm_in_7bit_mode_is_byte_identical_to_legacy_framing() {
    for (query, seven_bit) in FRAMING_QUERIES {
        let (mut state, rx) = make_state();
        let resp = feed_and_collect_bytes(&mut state, &rx, query);
        assert_eq!(
            resp,
            seven_bit,
            "query {:?}: 7-bit reply changed",
            String::from_utf8_lossy(query)
        );
    }
}

#[test]
fn decrpm_returns_to_7bit_framing_after_s7c1t() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G"); // S8C1T
    state.handle_incoming_data(b"\x1b F"); // S7C1T
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?2004$p");
    assert_eq!(resp, b"\x1b[?2004;2$y");
}

#[test]
fn decrpm_mouse_query_in_8bit_mode_uses_0x9b() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G"); // S8C1T
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?1006$p");
    assert_eq!(resp, b"\x9b?1006;2$y");
}

#[test]
fn decrpm_unknown_mode_in_8bit_mode_uses_0x9b() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G"); // S8C1T
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?9999$p");
    assert_eq!(resp, b"\x9b?9999;0$y");
    assert!(!contains_7bit_csi(&resp));
}

#[test]
fn decrpm_unknown_mode_in_7bit_mode_is_unchanged() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?9999$p");
    assert_eq!(resp, b"\x1b[?9999;0$y");
}

#[test]
fn kitty_keyboard_query_in_8bit_mode_is_0x9b_question_flags_u() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G"); // S8C1T
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?u");
    assert_eq!(resp, b"\x9b?0u");
}

#[test]
fn kitty_keyboard_query_in_7bit_mode_is_unchanged() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?u");
    assert_eq!(resp, b"\x1b[?0u");
}

#[test]
fn decrpm_on_disconnected_channel_does_not_panic() {
    let (mut state, rx) = make_state();
    drop(rx);
    // Handler-owned, state-owned and ?2031 replies must all be dropped quietly.
    state.handle_incoming_data(b"\x1b[?7$p\x1b[?2004$p\x1b[?2031$p");
}

// ═══════════════════════════════════════════════════════════════════════════
// LNM (ANSI mode 20) — answered in the ANSI form `20;Ps$y`, without `?`
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_lnm_default_is_reset_in_ansi_form() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[20$p");
    assert_eq!(resp, "\x1b[20;2$y", "LNM default → Ps=2, no `?` prefix");
}

#[test]
fn decrpm_lnm_after_set_is_set() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b[20h");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[20$p");
    assert_eq!(resp, "\x1b[20;1$y");
}

#[test]
fn decrpm_lnm_after_reset_is_reset() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b[20h");
    state.handle_incoming_data(b"\x1b[20l");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[20$p");
    assert_eq!(resp, "\x1b[20;2$y");
}

// ═══════════════════════════════════════════════════════════════════════════
// DECSCLM (?4) — recognised but never settable: always "permanently reset"
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn decrpm_decsclm_default_is_permanently_reset() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?4$p");
    assert_eq!(resp, "\x1b[?4;4$y", "exactly one Ps=4 reply, nothing else");
}

#[test]
fn decrpm_decsclm_after_set_is_permanently_reset() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b[?4h");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?4$p");
    assert_eq!(resp, "\x1b[?4;4$y");
}

#[test]
fn decrpm_decsclm_after_reset_is_permanently_reset() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b[?4h");
    state.handle_incoming_data(b"\x1b[?4l");
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?4$p");
    assert_eq!(resp, "\x1b[?4;4$y");
}

#[test]
fn decsclm_set_and_reset_produce_no_reply() {
    let (mut state, rx) = make_state();
    let resp = feed_and_collect(&mut state, &rx, b"\x1b[?4h\x1b[?4l");
    assert_eq!(
        resp, "",
        "DECSET/DECRST of ?4 are not acted on and not answered"
    );
}

#[test]
fn decrpm_decsclm_in_8bit_mode_uses_0x9b() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G"); // S8C1T
    let resp = feed_and_collect_bytes(&mut state, &rx, b"\x1b[?4$p");
    assert_eq!(resp, b"\x9b?4;4$y");
}
