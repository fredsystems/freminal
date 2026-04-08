// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Unit tests for `TerminalInput::to_payload` — control characters and function keys.
//!
//! These tests verify that:
//!  1. `Ctrl(letter)` maps to the correct C0 control byte (letter & 0x1F).
//!  2. `Ctrl(b'K')` maps to 0x0B (VT), *not* 0x0A (LF) — regression guard for the
//!     `Key::J | Key::K => LineFeed` special-case bug that was fixed by keeping only
//!     `Key::J` in that arm.
//!  3. Special Ctrl combos used by nano (`Ctrl+-`, `Ctrl+/`, `Ctrl+Space`) produce
//!     the expected bytes.
//!  4. `FunctionKey(n)` for F1–F12 produces the xterm/VT escape sequences that
//!     most terminal applications (including nano) expect.

use freminal_common::buffer_states::modes::{
    application_escape_key::ApplicationEscapeKey, decbkm::Decbkm, decckm::Decckm,
    keypad::KeypadMode, lnm::Lnm,
};
use freminal_terminal_emulator::input::{
    KeyEventMeta, KeyEventType, KeyModifiers, TerminalInput, TerminalInputPayload,
};

/// Convenience: call `to_payload` with both mode flags in normal/default state
/// and unwrap the result as a `Vec<u8>`.
fn payload_bytes(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

// ---------------------------------------------------------------------------
// Ctrl + letter (A–Z)
// ---------------------------------------------------------------------------

#[test]
fn ctrl_a_is_0x01() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'A')), vec![0x01]);
}

#[test]
fn ctrl_c_is_0x03() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'C')), vec![0x03]);
}

#[test]
fn ctrl_g_is_0x07() {
    // nano: ^G = Help
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'G')), vec![0x07]);
}

/// Regression: Ctrl+J must be 0x0A (LF) — this is what `LineFeed` also sends.
#[test]
fn ctrl_j_is_0x0a() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'J')), vec![0x0A]);
}

/// Regression: Ctrl+K must be 0x0B (VT), **not** 0x0A.
/// This was broken before the `Key::J | Key::K => LineFeed` arm was narrowed
/// to `Key::J` only.
#[test]
fn ctrl_k_is_0x0b_not_0x0a() {
    let got = payload_bytes(&TerminalInput::Ctrl(b'K'));
    assert_eq!(got, vec![0x0B], "Ctrl+K must send 0x0B (VT), not 0x0A (LF)");
    assert_ne!(
        got,
        vec![0x0A],
        "Ctrl+K must not be confused with Ctrl+J (LF)"
    );
}

#[test]
fn ctrl_x_is_0x18() {
    // nano: ^X = Exit
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'X')), vec![0x18]);
}

#[test]
fn ctrl_z_is_0x1a() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'Z')), vec![0x1A]);
}

// ---------------------------------------------------------------------------
// Ctrl + special/punctuation keys (via TerminalInput::Ascii for the ones that
// map outside the letter range, or via TerminalInput::Ctrl for the ones that
// share the 0x40–0x5F letter block).
// ---------------------------------------------------------------------------

/// Ctrl+[ => 0x1B (ESC) — same code as Key::Escape
#[test]
fn ctrl_open_bracket_is_0x1b() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'[')), vec![0x1B]);
}

/// Ctrl+] => 0x1D (GS)
#[test]
fn ctrl_close_bracket_is_0x1d() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b']')), vec![0x1D]);
}

/// Ctrl+\ => 0x1C (FS)
#[test]
fn ctrl_backslash_is_0x1c() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'\\')), vec![0x1C]);
}

/// Ctrl+Space => 0x00 (NUL) — Ctrl(b' ')
#[test]
fn ctrl_space_is_0x00() {
    // b' ' = 0x20; 0x20 & 0x1F = 0x00
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b' ')), vec![0x00]);
}

/// Ctrl+- => 0x1F (US), sent as TerminalInput::Ascii(0x1F) by the GUI layer.
/// Nano binds this to "Undo".
#[test]
fn ctrl_minus_ascii_is_0x1f() {
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1F)), vec![0x1F]);
}

/// Ctrl+/ => 0x1F (US), same byte.
/// Nano binds this to "Go to Line".
#[test]
fn ctrl_slash_ascii_is_0x1f() {
    // The GUI maps both Key::Slash and Key::Minus with Ctrl to Ascii(0x1F).
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1F)), vec![0x1F]);
}

// ---------------------------------------------------------------------------
// Ctrl + digit row (produces C0 bytes that the letter range cannot reach)
// ---------------------------------------------------------------------------

#[test]
fn ctrl_digit_2_is_0x00() {
    // Ctrl+2 => NUL (same as Ctrl+Space / Ctrl+@)
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x00)), vec![0x00]);
}

#[test]
fn ctrl_digit_3_is_0x1b() {
    // Ctrl+3 => ESC (same as Ctrl+[)
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1B)), vec![0x1B]);
}

#[test]
fn ctrl_digit_4_is_0x1c() {
    // Ctrl+4 => FS (same as Ctrl+\)
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1C)), vec![0x1C]);
}

#[test]
fn ctrl_digit_5_is_0x1d() {
    // Ctrl+5 => GS (same as Ctrl+])
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1D)), vec![0x1D]);
}

#[test]
fn ctrl_digit_6_is_0x1e() {
    // Ctrl+6 => RS (same as Ctrl+^)
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1E)), vec![0x1E]);
}

#[test]
fn ctrl_digit_7_is_0x1f() {
    // Ctrl+7 => US (same as Ctrl+_ / Ctrl+- / Ctrl+/)
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x1F)), vec![0x1F]);
}

#[test]
fn ctrl_digit_8_is_0x7f() {
    // Ctrl+8 => DEL
    assert_eq!(payload_bytes(&TerminalInput::Ascii(0x7F)), vec![0x7F]);
}

// ---------------------------------------------------------------------------
// Function keys F1–F12
// ---------------------------------------------------------------------------

#[test]
fn f1_sends_ss3_p() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(1, KeyModifiers::NONE)),
        b"\x1bOP".to_vec()
    );
}

#[test]
fn f2_sends_ss3_q() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(2, KeyModifiers::NONE)),
        b"\x1bOQ".to_vec()
    );
}

#[test]
fn f3_sends_ss3_r() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(3, KeyModifiers::NONE)),
        b"\x1bOR".to_vec()
    );
}

#[test]
fn f4_sends_ss3_s() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(4, KeyModifiers::NONE)),
        b"\x1bOS".to_vec()
    );
}

#[test]
fn f5_sends_csi_15_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(5, KeyModifiers::NONE)),
        b"\x1b[15~".to_vec()
    );
}

#[test]
fn f6_sends_csi_17_tilde() {
    // Note: F6 maps to 17~ (not 16~); the VT sequence table skips 16
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(6, KeyModifiers::NONE)),
        b"\x1b[17~".to_vec()
    );
}

#[test]
fn f7_sends_csi_18_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(7, KeyModifiers::NONE)),
        b"\x1b[18~".to_vec()
    );
}

#[test]
fn f8_sends_csi_19_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(8, KeyModifiers::NONE)),
        b"\x1b[19~".to_vec()
    );
}

#[test]
fn f9_sends_csi_20_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(9, KeyModifiers::NONE)),
        b"\x1b[20~".to_vec()
    );
}

#[test]
fn f10_sends_csi_21_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(10, KeyModifiers::NONE)),
        b"\x1b[21~".to_vec()
    );
}

#[test]
fn f11_sends_csi_23_tilde() {
    // Note: F11 maps to 23~ (not 22~); the VT sequence table skips 22
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(11, KeyModifiers::NONE)),
        b"\x1b[23~".to_vec()
    );
}

#[test]
fn f12_sends_csi_24_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(12, KeyModifiers::NONE)),
        b"\x1b[24~".to_vec()
    );
}

/// An out-of-range function key should return an empty byte slice, not panic.
#[test]
fn function_key_out_of_range_does_not_panic() {
    let got = payload_bytes(&TerminalInput::FunctionKey(99, KeyModifiers::NONE));
    assert!(
        got.is_empty(),
        "out-of-range FunctionKey should return empty payload, got {got:?}"
    );
}

// ---------------------------------------------------------------------------
// Sanity-check: distinct Ctrl+J vs Ctrl+K payloads
// ---------------------------------------------------------------------------

#[test]
fn ctrl_j_and_ctrl_k_produce_different_bytes() {
    let j = payload_bytes(&TerminalInput::Ctrl(b'J'));
    let k = payload_bytes(&TerminalInput::Ctrl(b'K'));
    assert_ne!(
        j, k,
        "Ctrl+J (LF=0x0A) and Ctrl+K (VT=0x0B) must be different bytes"
    );
    assert_eq!(j, vec![0x0A], "Ctrl+J must be 0x0A");
    assert_eq!(k, vec![0x0B], "Ctrl+K must be 0x0B");
}

// ---------------------------------------------------------------------------
// Platform-intercepted Ctrl combos (egui-winit converts these to synthetic
// Event::Copy / Event::Cut before they ever become Event::Key events).
// The GUI layer maps those synthetic events back to Ctrl(b'c') / Ctrl(b'x'),
// so we verify the payload here to guard against regressions.
// ---------------------------------------------------------------------------

/// Event::Copy → Ctrl(b'c') → 0x03 (ETX / interrupt)
/// nano: ^C = Cancel current operation
#[test]
fn platform_copy_event_produces_ctrl_c() {
    // egui fires Event::Copy for Ctrl+C; the handler emits Ctrl(b'c').
    assert_eq!(
        payload_bytes(&TerminalInput::Ctrl(b'c')),
        vec![0x03],
        "Ctrl+C must send 0x03 (ETX)"
    );
}

/// Event::Cut → Ctrl(b'x') → 0x18 (CAN)
/// nano: ^X = Exit
#[test]
fn platform_cut_event_produces_ctrl_x() {
    // egui fires Event::Cut for Ctrl+X; the handler emits Ctrl(b'x').
    assert_eq!(
        payload_bytes(&TerminalInput::Ctrl(b'x')),
        vec![0x18],
        "Ctrl+X must send 0x18 (CAN)"
    );
}

/// Ctrl+C and Ctrl+X must produce distinct bytes.
#[test]
fn ctrl_c_and_ctrl_x_are_different() {
    let c = payload_bytes(&TerminalInput::Ctrl(b'c'));
    let x = payload_bytes(&TerminalInput::Ctrl(b'x'));
    assert_ne!(c, x, "Ctrl+C (0x03) and Ctrl+X (0x18) must differ");
}

// ---------------------------------------------------------------------------
// Modified key sequences (xterm-style CSI 1;Nm <final>)
// ---------------------------------------------------------------------------

/// Convenience: call `to_payload` with DECCKM mode on and unwrap.
fn payload_bytes_decckm(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(
        Decckm::Application,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// Shift+ArrowUp → ESC[1;2A
#[test]
fn shift_arrow_up_sends_csi_1_2_a() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::ArrowUp(mods));
    assert_eq!(got, b"\x1b[1;2A".to_vec());
}

/// Ctrl+ArrowLeft → ESC[1;5D
#[test]
fn ctrl_arrow_left_sends_csi_1_5_d() {
    let mods = KeyModifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::ArrowLeft(mods));
    assert_eq!(got, b"\x1b[1;5D".to_vec());
}

/// Alt+Home → ESC[1;3H
#[test]
fn alt_home_sends_csi_1_3_h() {
    let mods = KeyModifiers {
        shift: false,
        ctrl: false,
        alt: true,
    };
    let got = payload_bytes(&TerminalInput::Home(mods));
    assert_eq!(got, b"\x1b[1;3H".to_vec());
}

/// Ctrl+Shift+F5 → ESC[15;6~
#[test]
fn ctrl_shift_f5_sends_csi_15_6_tilde() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: true,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::FunctionKey(5, mods));
    assert_eq!(got, b"\x1b[15;6~".to_vec());
}

/// Shift+Delete → ESC[3;2~
#[test]
fn shift_delete_sends_csi_3_2_tilde() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::Delete(mods));
    assert_eq!(got, b"\x1b[3;2~".to_vec());
}

/// ArrowUp with no modifiers (normal mode) → ESC[A (unchanged from before)
#[test]
fn unmodified_arrow_up_normal_mode() {
    let got = payload_bytes(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    assert_eq!(got, b"\x1b[A".to_vec());
}

/// ArrowUp with no modifiers in DECCKM mode → ESC O A (SS3 form)
#[test]
fn unmodified_arrow_up_decckm_mode() {
    let got = payload_bytes_decckm(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    assert_eq!(got, b"\x1bOA".to_vec());
}

/// ArrowUp with modifiers in DECCKM mode → CSI form, NOT SS3.
/// Modified keys always use CSI even when DECCKM is active.
#[test]
fn modified_arrow_up_decckm_uses_csi_not_ss3() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    let got = payload_bytes_decckm(&TerminalInput::ArrowUp(mods));
    assert_eq!(
        got,
        b"\x1b[1;2A".to_vec(),
        "Modified keys must use CSI form even in DECCKM mode"
    );
    assert!(
        !got.starts_with(b"\x1bO"),
        "Modified keys must NOT use SS3 form"
    );
}

/// Ctrl+Alt+Shift+ArrowRight → ESC[1;8C (all three modifiers: 1+1+2+4=8)
#[test]
fn all_modifiers_arrow_right() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: true,
        alt: true,
    };
    let got = payload_bytes(&TerminalInput::ArrowRight(mods));
    assert_eq!(got, b"\x1b[1;8C".to_vec());
}

/// Shift+F1 → ESC[1;2P (F1–F4 with modifiers use CSI final form, not SS3)
#[test]
fn shift_f1_sends_csi_1_2_p() {
    let mods = KeyModifiers {
        shift: true,
        ctrl: false,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::FunctionKey(1, mods));
    assert_eq!(got, b"\x1b[1;2P".to_vec());
}

/// Ctrl+Insert → ESC[2;5~
#[test]
fn ctrl_insert_sends_csi_2_5_tilde() {
    let mods = KeyModifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::Insert(mods));
    assert_eq!(got, b"\x1b[2;5~".to_vec());
}

/// Alt+PageDown → ESC[6;3~
#[test]
fn alt_page_down_sends_csi_6_3_tilde() {
    let mods = KeyModifiers {
        shift: false,
        ctrl: false,
        alt: true,
    };
    let got = payload_bytes(&TerminalInput::PageDown(mods));
    assert_eq!(got, b"\x1b[6;3~".to_vec());
}

/// Ctrl+End → ESC[1;5F
#[test]
fn ctrl_end_sends_csi_1_5_f() {
    let mods = KeyModifiers {
        shift: false,
        ctrl: true,
        alt: false,
    };
    let got = payload_bytes(&TerminalInput::End(mods));
    assert_eq!(got, b"\x1b[1;5F".to_vec());
}

/// KeyModifiers::NONE has modifier_param() == None.
#[test]
fn key_modifiers_none_has_no_param() {
    assert!(KeyModifiers::NONE.is_empty());
    assert_eq!(KeyModifiers::NONE.modifier_param(), None);
}

/// Verify all eight modifier combinations produce correct param values.
#[test]
fn key_modifiers_all_combinations() {
    // Shift=2, Alt=3, Shift+Alt=4, Ctrl=5, Ctrl+Shift=6, Ctrl+Alt=7, Ctrl+Alt+Shift=8
    let cases: &[(bool, bool, bool, u8)] = &[
        (true, false, false, 2), // Shift
        (false, false, true, 3), // Alt
        (true, false, true, 4),  // Shift+Alt
        (false, true, false, 5), // Ctrl
        (true, true, false, 6),  // Ctrl+Shift
        (false, true, true, 7),  // Ctrl+Alt
        (true, true, true, 8),   // Ctrl+Alt+Shift
    ];
    for &(shift, ctrl, alt, expected) in cases {
        let mods = KeyModifiers { shift, ctrl, alt };
        assert_eq!(
            mods.modifier_param(),
            Some(expected),
            "shift={shift}, ctrl={ctrl}, alt={alt} should produce param {expected}"
        );
    }
}

// ---------------------------------------------------------------------------
// modifyOtherKeys level 2: Ctrl+letter sends CSI 27;5;code~ encoding
// ---------------------------------------------------------------------------
//
// Programs like tmux send `CSI > 4 ; 2 m` (modifyOtherKeys level 2) and
// expect the outer terminal to encode Ctrl+letter as `CSI 27;5;code~`
// where code is the decimal ASCII value of the lowercase letter.  This
// allows tmux and inner programs (like nvim) to distinguish all key
// combinations unambiguously.  WezTerm sends this format at level 2.

/// Convenience: call `to_payload` with `modify_other_keys = 2`.
fn payload_bytes_mok2(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        2,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// At modifyOtherKeys level 2, Ctrl+A sends CSI 27;5;97~ (a=97).
#[test]
fn ctrl_a_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'a'));
    assert_eq!(bytes, b"\x1b[27;5;97~");
}

/// At modifyOtherKeys level 2, Ctrl+C sends CSI 27;5;99~ (c=99).
#[test]
fn ctrl_c_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'c'));
    assert_eq!(bytes, b"\x1b[27;5;99~");
}

/// At modifyOtherKeys level 2, Ctrl+Z sends CSI 27;5;122~ (z=122).
#[test]
fn ctrl_z_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'z'));
    assert_eq!(bytes, b"\x1b[27;5;122~");
}

/// At modifyOtherKeys level 0, Ctrl+A should still produce the control code 0x01.
#[test]
fn ctrl_a_modify_other_keys_level_0() {
    let bytes = payload_bytes(&TerminalInput::Ctrl(b'A'));
    assert_eq!(bytes, vec![0x01]);
}

/// At modifyOtherKeys level 1, Ctrl+A should still produce the control code 0x01
/// (level 1 only affects ambiguous keys, and Freminal sends control codes at level 1).
#[test]
fn ctrl_a_modify_other_keys_level_1() {
    match TerminalInput::Ctrl(b'A').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        1,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x01),
        other => panic!("Expected Single(0x01), got {other:?}"),
    }
}

/// Arrow keys are NOT affected by modifyOtherKeys — they still use CSI sequences.
#[test]
fn arrow_up_unaffected_by_modify_other_keys() {
    let bytes_mok0 = payload_bytes(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    let bytes_mok2 = payload_bytes_mok2(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    assert_eq!(bytes_mok0, bytes_mok2);
    assert_eq!(bytes_mok2, b"\x1b[A");
}

// ---------------------------------------------------------------------------
// Application Escape Key (?7727) tests
// ---------------------------------------------------------------------------

/// Convenience: call `to_payload` with `application_escape_key = Set`.
fn payload_bytes_aek(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Set,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// When Application Escape Key is active, Escape sends `CSI 27 ; 1 ; 27 ~`.
#[test]
fn escape_with_application_escape_key() {
    let bytes = payload_bytes_aek(&TerminalInput::Escape);
    assert_eq!(bytes, b"\x1b[27;1;27~");
}

/// When Application Escape Key is NOT active, Escape sends bare `0x1b`.
#[test]
fn escape_without_application_escape_key() {
    let bytes = payload_bytes(&TerminalInput::Escape);
    assert_eq!(bytes, vec![0x1b]);
}

/// Arrow keys are NOT affected by Application Escape Key.
#[test]
fn arrow_up_unaffected_by_application_escape_key() {
    let normal = payload_bytes(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    let aek = payload_bytes_aek(&TerminalInput::ArrowUp(KeyModifiers::NONE));
    assert_eq!(normal, aek);
    assert_eq!(aek, b"\x1b[A");
}

/// Ctrl keys are NOT affected by Application Escape Key.
#[test]
fn ctrl_c_unaffected_by_application_escape_key() {
    let normal = payload_bytes(&TerminalInput::Ctrl(b'C'));
    let aek = payload_bytes_aek(&TerminalInput::Ctrl(b'C'));
    assert_eq!(normal, aek);
    assert_eq!(aek, vec![0x03]);
}

/// Enter is NOT affected by Application Escape Key.
#[test]
fn enter_unaffected_by_application_escape_key() {
    let normal = payload_bytes(&TerminalInput::Enter);
    let aek = payload_bytes_aek(&TerminalInput::Enter);
    assert_eq!(normal, aek);
}

// ---------------------------------------------------------------------------
// modifyOtherKeys level 1 semantics
// ---------------------------------------------------------------------------

/// At modifyOtherKeys level 1, Ctrl+B should produce the C0 control code 0x02
/// (level 1 only modifies ambiguous keys; Freminal sends C0 codes at level 1).
#[test]
fn ctrl_b_modify_other_keys_level_1() {
    match TerminalInput::Ctrl(b'B').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        1,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x02),
        other => panic!("Expected Single(0x02), got {other:?}"),
    }
}

/// At modifyOtherKeys level 1, Ctrl+Z should still produce 0x1A.
#[test]
fn ctrl_z_modify_other_keys_level_1() {
    match TerminalInput::Ctrl(b'Z').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        1,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x1A),
        other => panic!("Expected Single(0x1A), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// modifyOtherKeys level 2: Ctrl combinations send CSI 27;5;code~ encoding
// ---------------------------------------------------------------------------

/// At level 2, Ctrl+B sends CSI 27;5;98~ (b=98).
#[test]
fn ctrl_b_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'b'));
    assert_eq!(bytes, b"\x1b[27;5;98~");
}

/// At level 2, Ctrl+Y sends CSI 27;5;121~ (y=121).
#[test]
fn ctrl_y_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'y'));
    assert_eq!(bytes, b"\x1b[27;5;121~");
}

/// At level 2, Ctrl+M sends CSI 27;5;109~ (m=109).
#[test]
fn ctrl_m_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'm'));
    assert_eq!(bytes, b"\x1b[27;5;109~");
}

/// At level 2, Ctrl+[ sends CSI 27;5;91~ ([=91).
#[test]
fn ctrl_open_bracket_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'['));
    assert_eq!(bytes, b"\x1b[27;5;91~");
}

/// At level 2, Ctrl+Space sends CSI 27;5;32~ (space=32).
#[test]
fn ctrl_space_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b' '));
    assert_eq!(bytes, b"\x1b[27;5;32~");
}

// ---------------------------------------------------------------------------
// Interaction: modifyOtherKeys + Application Escape Key simultaneously
// ---------------------------------------------------------------------------

/// When both modifyOtherKeys >= 2 AND application_escape_key are active,
/// Escape should use the application_escape_key encoding (CSI 27;1;27~).
/// The Escape key is handled by its own match arm, independent of MOK.
#[test]
fn escape_with_both_mok2_and_aek() {
    match TerminalInput::Escape.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        2,
        ApplicationEscapeKey::Set,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[27;1;27~");
        }
        other => panic!("Expected Owned(CSI 27;1;27~), got {other:?}"),
    }
}

/// When both modifyOtherKeys >= 2 AND application_escape_key are active,
/// Ctrl+C sends the MOK2 encoding CSI 27;5;99~ — AEK does not affect
/// Ctrl+letter encoding, but MOK2 does.
#[test]
fn ctrl_c_with_both_mok2_and_aek() {
    match TerminalInput::Ctrl(b'c').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        2,
        ApplicationEscapeKey::Set,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[27;5;99~");
        }
        other => panic!("Expected Owned(CSI 27;5;99~), got {other:?}"),
    }
}

/// Ctrl+A with application_escape_key=Set and mok=0 — AEK has no effect on Ctrl.
#[test]
fn ctrl_a_with_aek_and_mok0() {
    match TerminalInput::Ctrl(b'A').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Set,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x01),
        other => panic!("Expected Single(0x01), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// DECBKM (?67) — Backarrow Key Mode
// ---------------------------------------------------------------------------

/// Backspace with DECBKM set (default): sends BS (0x08).
#[test]
fn backspace_decbkm_set_sends_bs() {
    match TerminalInput::Backspace.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(b, 0x08, "DECBKM set: Backspace must send BS (0x08)")
        }
        other => panic!("Expected Single(0x08), got {other:?}"),
    }
}

/// Backspace with DECBKM reset: sends DEL (0x7F).
#[test]
fn backspace_decbkm_reset_sends_del() {
    match TerminalInput::Backspace.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsDel,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(b, 0x7F, "DECBKM reset: Backspace must send DEL (0x7F)")
        }
        other => panic!("Expected Single(0x7F), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// LNM (Line Feed / New Line Mode) — Enter key encoding
// ---------------------------------------------------------------------------

/// Enter with LNM reset (default): sends bare CR (0x0D).
///
/// This is the standard VT100 behavior.  The POSIX tty layer's ICRNL flag
/// translates CR→NL on input for shells that need it.
#[test]
fn enter_lnm_reset_sends_cr() {
    match TerminalInput::Enter.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(b, 0x0D, "LNM reset: Enter must send bare CR (0x0D)");
        }
        other => panic!("Expected Single(0x0D), got {other:?}"),
    }
}

/// Enter with LNM set: sends CR+LF (0x0D 0x0A).
///
/// When the host sets LNM (CSI 20 h), the terminal must send CR+LF for the
/// Enter key instead of bare CR.  vttest menu 6.2 (`tst_NLM`) tests this by
/// enabling LNM, having the user press Enter, and verifying the host receives
/// both CR and LF.
#[test]
fn enter_lnm_set_sends_cr_lf() {
    match TerminalInput::Enter.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::NewLine,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Many(bs) => {
            assert_eq!(
                bs, b"\x0d\x0a",
                "LNM set: Enter must send CR+LF (0x0D 0x0A)"
            );
        }
        other => panic!("Expected Many(CR+LF), got {other:?}"),
    }
}

/// LineFeed is unaffected by LNM — it always sends bare LF (0x0A).
///
/// LNM only affects the Enter key, not `TerminalInput::LineFeed` (which
/// represents Ctrl+J mapped to the LF byte).
#[test]
fn linefeed_unaffected_by_lnm_set() {
    match TerminalInput::LineFeed.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::NewLine,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(
                b, 0x0A,
                "LineFeed must always send LF (0x0A) regardless of LNM"
            );
        }
        other => panic!("Expected Single(0x0A), got {other:?}"),
    }
}

// ===========================================================================
// Kitty Keyboard Protocol (KKP) encoding tests
// ===========================================================================

/// Convenience: call `to_payload` with KKP flags active and default modes.
fn payload_bytes_kkp(input: &TerminalInput, flags: u32) -> Vec<u8> {
    match input.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsDel,
        Lnm::LineFeed,
        flags,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// KKP flags=0 must not change any behaviour: Ctrl+C still sends 0x03.
#[test]
fn kkp_flag0_no_change_to_ctrl_c() {
    assert_eq!(payload_bytes(&TerminalInput::Ctrl(b'C')), vec![0x03]);
}

/// KKP flag 1 (DISAMBIGUATE): Ctrl+C → CSI 99;5u (lowercase c = 99).
#[test]
fn kkp_disambiguate_ctrl_c() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'C'), 1),
        b"\x1b[99;5u"
    );
}

/// KKP flag 1 (DISAMBIGUATE): Ctrl+A → CSI 97;5u (lowercase a = 97).
#[test]
fn kkp_disambiguate_ctrl_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'A'), 1),
        b"\x1b[97;5u"
    );
}

/// KKP flag 1 (DISAMBIGUATE): Escape → CSI 27u.
#[test]
fn kkp_disambiguate_escape() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Escape, 1), b"\x1b[27u");
}

/// KKP flag 1 exception: Enter still sends legacy CR (0x0D), NOT CSI u.
#[test]
fn kkp_disambiguate_enter_still_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Enter, 1), vec![0x0D]);
}

/// KKP flag 1 exception: Tab still sends legacy HT (0x09), NOT CSI u.
#[test]
fn kkp_disambiguate_tab_still_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Tab, 1), vec![0x09]);
}

/// KKP flag 1 exception: Backspace still sends legacy DEL (0x7F), NOT CSI u.
/// Uses `BackarrowSendsDel` (the default for `payload_bytes_kkp`).
#[test]
fn kkp_disambiguate_backspace_still_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Backspace, 1), vec![0x7F]);
}

/// KKP flag 8 (REPORT_ALL_KEYS): Enter → CSI 13u.
#[test]
fn kkp_all_keys_enter() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Enter, 8), b"\x1b[13u");
}

/// KKP flag 8 (REPORT_ALL_KEYS): Tab → CSI 9u.
#[test]
fn kkp_all_keys_tab() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Tab, 8), b"\x1b[9u");
}

/// KKP flag 8 (REPORT_ALL_KEYS): Backspace → CSI 127u.
#[test]
fn kkp_all_keys_backspace() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Backspace, 8),
        b"\x1b[127u"
    );
}

/// KKP flag 8 (REPORT_ALL_KEYS): Ascii(b'a') → CSI 97u.
#[test]
fn kkp_all_keys_ascii_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'a'), 8),
        b"\x1b[97u"
    );
}

/// KKP flag 8 (REPORT_ALL_KEYS): Ascii(b'A') → CSI 97;2u (lowercase + Shift).
#[test]
fn kkp_all_keys_ascii_shift_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'A'), 8),
        b"\x1b[97;2u"
    );
}

/// KKP flag 1: ArrowLeft(NONE) → legacy `\x1b[D` (unchanged by KKP).
#[test]
fn kkp_arrow_left_no_mods() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::ArrowLeft(KeyModifiers::NONE), 1),
        b"\x1b[D"
    );
}

/// KKP flag 1: ArrowLeft(ctrl) → `\x1b[1;5D` (modifier encoding is unchanged).
#[test]
fn kkp_arrow_left_ctrl() {
    let mods = KeyModifiers {
        ctrl: true,
        ..KeyModifiers::NONE
    };
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::ArrowLeft(mods), 1),
        b"\x1b[1;5D"
    );
}

/// KKP flag 1: Home(NONE) → legacy `\x1b[H` (unchanged by KKP).
#[test]
fn kkp_home_no_mods() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Home(KeyModifiers::NONE), 1),
        b"\x1b[H"
    );
}

/// KKP flag 1: FunctionKey(1, NONE) → legacy `\x1bOP` (unchanged by KKP).
#[test]
fn kkp_f1_no_mods() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::FunctionKey(1, KeyModifiers::NONE), 1),
        b"\x1bOP"
    );
}

/// KKP flag 1: FunctionKey(5, shift) → `\x1b[15;2~` (legacy tilde encoding).
#[test]
fn kkp_f5_shift() {
    let mods = KeyModifiers {
        shift: true,
        ..KeyModifiers::NONE
    };
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::FunctionKey(5, mods), 1),
        b"\x1b[15;2~"
    );
}

// ===========================================================================
// KKP flags 2/4/16 alone: must NOT change encoding (legacy preserved)
// ===========================================================================
//
// Flags 2 (REPORT_EVENT_TYPES), 4 (REPORT_ALTERNATE_KEYS), and 16
// (REPORT_ASSOCIATED_TEXT) are additive metadata flags.  When set without
// DISAMBIGUATE (1) or REPORT_ALL (8), they must not change the base encoding.

/// KKP flag 2 alone: Ctrl+A sends legacy C0 byte 0x01, not CSI u.
#[test]
fn kkp_flag_2_only_ctrl_a_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Ctrl(b'A'), 2), vec![0x01]);
}

/// KKP flag 4 alone: Ctrl+C sends legacy C0 byte 0x03, not CSI u.
#[test]
fn kkp_flag_4_only_ctrl_c_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Ctrl(b'C'), 4), vec![0x03]);
}

/// KKP flag 16 alone: Escape sends legacy bare ESC (0x1B), not CSI 27u.
#[test]
fn kkp_flag_16_only_escape_legacy() {
    assert_eq!(payload_bytes_kkp(&TerminalInput::Escape, 16), vec![0x1B]);
}

/// KKP flags 2|4|16 combined (22): Ctrl+A still sends legacy C0 byte 0x01.
#[test]
fn kkp_flags_2_4_16_ctrl_a_legacy() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'A'), 2 | 4 | 16),
        vec![0x01]
    );
}

/// KKP flags 2|4|16 combined: Escape still sends legacy ESC (0x1B).
#[test]
fn kkp_flags_2_4_16_escape_legacy() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Escape, 2 | 4 | 16),
        vec![0x1B]
    );
}

/// KKP flags 2|4|16 combined: plain ASCII 'a' sends legacy byte.
#[test]
fn kkp_flags_2_4_16_ascii_legacy() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'a'), 2 | 4 | 16),
        vec![b'a']
    );
}

/// KKP flag 2 + DISAMBIGUATE (3): Ctrl+A gets CSI u encoding (flag 1 wins).
#[test]
fn kkp_flag_2_plus_disambiguate_ctrl_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'A'), 1 | 2),
        b"\x1b[97;5u"
    );
}

/// KKP flag 4 + REPORT_ALL (12): plain ASCII gets CSI u encoding with
/// alternate key fields: `CSI 97:65:97 u` (codepoint:shifted:base).
#[test]
fn kkp_flag_4_plus_report_all_ascii() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'a'), 4 | 8),
        b"\x1b[97:65:97u"
    );
}

// ===========================================================================
// KKP flag 2 (REPORT_EVENT_TYPES) — event type metadata
// ===========================================================================
//
// When flag 2 is active (along with flag 1 or 8 to enable CSI u), the
// modifier parameter gets a `:event-type` suffix:
//   - Press = 1 (elided since it's the default)
//   - Repeat = 2
//   - Release = 3
//
// Format: CSI codepoint ; modifiers[:event_type] u

/// Convenience: call `to_payload` with KKP flags and custom `KeyEventMeta`.
fn payload_bytes_kkp_meta(input: &TerminalInput, flags: u32, meta: &KeyEventMeta) -> Vec<u8> {
    match input.to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        0,
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsDel,
        Lnm::LineFeed,
        flags,
        meta,
    ) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// Flag 2 + flag 8: plain 'a' press → CSI 97u (event_type 1 is elided).
#[test]
fn kkp_flag_2_press_ascii_elides_event_type() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 2, &meta),
        b"\x1b[97u"
    );
}

/// Flag 2 + flag 8: plain 'a' repeat → CSI 97;1:2u (modifier=1, event=2).
#[test]
fn kkp_flag_2_repeat_ascii() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Repeat,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 2, &meta),
        b"\x1b[97;1:2u"
    );
}

/// Flag 2 + flag 8: plain 'a' release → CSI 97;1:3u (modifier=1, event=3).
#[test]
fn kkp_flag_2_release_ascii() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Release,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 2, &meta),
        b"\x1b[97;1:3u"
    );
}

/// Flag 2 + flag 8: Ctrl+A repeat → CSI 97;5:2u (modifier=5, event=2).
#[test]
fn kkp_flag_2_repeat_ctrl_a() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Repeat,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ctrl(b'A'), 8 | 2, &meta),
        b"\x1b[97;5:2u"
    );
}

/// Flag 2 + flag 8: Ctrl+A release → CSI 97;5:3u (modifier=5, event=3).
#[test]
fn kkp_flag_2_release_ctrl_a() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Release,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ctrl(b'A'), 8 | 2, &meta),
        b"\x1b[97;5:3u"
    );
}

/// Flag 2 + flag 1: Escape release → CSI 27;1:3u.
#[test]
fn kkp_flag_2_release_escape() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Release,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Escape, 1 | 2, &meta),
        b"\x1b[27;1:3u"
    );
}

/// Flag 2 + flag 8: Enter repeat → CSI 13;1:2u.
#[test]
fn kkp_flag_2_repeat_enter() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Repeat,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Enter, 8 | 2, &meta),
        b"\x1b[13;1:2u"
    );
}

// ===========================================================================
// KKP flag 4 (REPORT_ALTERNATE_KEYS) — shifted/base codepoint fields
// ===========================================================================
//
// When flag 4 is active along with flag 1 or 8, the codepoint field becomes
// `codepoint:shifted:base`. Shifted is the US QWERTY shifted form if it
// differs from the base; otherwise the extra fields are omitted.

/// Flag 4 + flag 8: uppercase 'A' → CSI 97:65:97;2u
/// (lowercase 97, shifted 65='A', base 97, modifier 2=Shift).
#[test]
fn kkp_flag_4_uppercase_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'A'), 4 | 8),
        b"\x1b[97:65:97;2u"
    );
}

/// Flag 4 + flag 8: digit '1' → CSI 49:33:49u
/// (codepoint 49='1', shifted 33='!', base 49).
#[test]
fn kkp_flag_4_digit_1() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b'1'), 4 | 8),
        b"\x1b[49:33:49u"
    );
}

/// Flag 4 + flag 8: semicolon ';' → CSI 59:58:59u
/// (codepoint 59=';', shifted 58=':', base 59).
#[test]
fn kkp_flag_4_semicolon() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b';'), 4 | 8),
        b"\x1b[59:58:59u"
    );
}

/// Flag 4 + flag 8: Ctrl+A → CSI 97;5u (no shifted form for Ctrl).
/// Ctrl+letter codepoint is lowercase letter — same shifted mapping as ASCII.
#[test]
fn kkp_flag_4_ctrl_a() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'A'), 4 | 8),
        b"\x1b[97:65:97;5u"
    );
}

/// Flag 4 + flag 8: space (0x20) has no distinct shifted form → just `32`.
#[test]
fn kkp_flag_4_space_no_shifted() {
    // Space shifted is still space on US QWERTY — us_qwerty_shifted returns None
    // so no :shifted:base suffix.
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ascii(b' '), 4 | 8),
        b"\x1b[32u"
    );
}

/// Flag 4 + flag 1 (no report_all): Ctrl+C → CSI 99:67:99;5u.
#[test]
fn kkp_flag_4_disambiguate_ctrl_c() {
    assert_eq!(
        payload_bytes_kkp(&TerminalInput::Ctrl(b'C'), 1 | 4),
        b"\x1b[99:67:99;5u"
    );
}

// ===========================================================================
// KKP flag 16 (REPORT_ASSOCIATED_TEXT) — text as codepoints
// ===========================================================================
//
// When flag 16 is active along with flag 1 or 8, the associated text
// (if present) is appended as a third parameter: colon-separated codepoints.
//
// Format: CSI codepoint ; modifiers ; text_cp1:text_cp2:... u

/// Flag 16 + flag 8: ASCII 'a' with associated text "a" → CSI 97;1;97u.
#[test]
fn kkp_flag_16_ascii_a_with_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: Some("a".to_owned()),
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 16, &meta),
        b"\x1b[97;1;97u"
    );
}

/// Flag 16 + flag 8: Enter with no associated text → CSI 13u (no text field).
#[test]
fn kkp_flag_16_enter_no_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Enter, 8 | 16, &meta),
        b"\x1b[13u"
    );
}

/// Flag 16 + flag 8: Enter with empty string → CSI 13u (empty string = no text).
#[test]
fn kkp_flag_16_enter_empty_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: Some(String::new()),
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Enter, 8 | 16, &meta),
        b"\x1b[13u"
    );
}

/// Flag 16 + flag 8: Ctrl+A with no text → CSI 97;5u (no text field).
#[test]
fn kkp_flag_16_ctrl_a_no_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ctrl(b'A'), 8 | 16, &meta),
        b"\x1b[97;5u"
    );
}

/// Flag 16 + flag 8: multi-char text "ab" → text field has two codepoints.
#[test]
fn kkp_flag_16_multi_char_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: Some("ab".to_owned()),
    };
    // 'a'=97, 'b'=98
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 16, &meta),
        b"\x1b[97;1;97:98u"
    );
}

// ===========================================================================
// KKP combined flags (2+4+16 with activating flags)
// ===========================================================================

/// All flags (1|2|4|16): Ctrl+A repeat with text "x" → full CSI u encoding.
/// CSI 97:65:97 ; 5:2 ; 120 u
///   codepoint=97, shifted=65, base=97, modifier=5 (Ctrl), event=2 (repeat),
///   text codepoint=120 ('x').
#[test]
fn kkp_all_flags_ctrl_a_repeat_with_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Repeat,
        associated_text: Some("x".to_owned()),
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ctrl(b'A'), 1 | 2 | 4 | 16, &meta),
        b"\x1b[97:65:97;5:2;120u"
    );
}

/// Flags 8|2|16: plain 'a' release with text "a".
/// CSI 97 ; 1:3 ; 97 u
#[test]
fn kkp_flags_8_2_16_release_with_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Release,
        associated_text: Some("a".to_owned()),
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 2 | 16, &meta),
        b"\x1b[97;1:3;97u"
    );
}

/// Flags 8|4|16: uppercase 'A' with text "A".
/// CSI 97:65:97 ; 2 ; 65 u
///   (lowercase=97, shifted=65, base=97, modifier=2=Shift, text=65='A')
#[test]
fn kkp_flags_8_4_16_uppercase_a_with_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Press,
        associated_text: Some("A".to_owned()),
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'A'), 8 | 4 | 16, &meta),
        b"\x1b[97:65:97;2;65u"
    );
}

/// Flags 8|2|4: 'a' repeat, no text → CSI 97:65:97;1:2u.
#[test]
fn kkp_flags_8_2_4_repeat_no_text() {
    let meta = KeyEventMeta {
        event_type: KeyEventType::Repeat,
        associated_text: None,
    };
    assert_eq!(
        payload_bytes_kkp_meta(&TerminalInput::Ascii(b'a'), 8 | 2 | 4, &meta),
        b"\x1b[97:65:97;1:2u"
    );
}

// ===========================================================================
// modifyOtherKeys level 2: additional edge-case tests
// ===========================================================================

/// MOK2 does NOT affect Enter — it still sends legacy CR (0x0D).
#[test]
fn enter_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Enter);
    assert_eq!(bytes, vec![0x0D]);
}

/// MOK2 does NOT affect Tab — it still sends legacy HT (0x09).
#[test]
fn tab_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Tab);
    assert_eq!(bytes, vec![0x09]);
}

/// MOK2 does NOT affect Backspace — it still sends legacy BS (0x08) with DECBKM set.
#[test]
fn backspace_unaffected_by_mok2() {
    // payload_bytes_mok2 uses Decbkm::BackarrowSendsBs → 0x08
    let bytes = payload_bytes_mok2(&TerminalInput::Backspace);
    assert_eq!(bytes, vec![0x08]);
}

/// MOK2 does NOT affect Escape — it still sends bare ESC (0x1B) without AEK.
#[test]
fn escape_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Escape);
    assert_eq!(bytes, vec![0x1B]);
}

/// MOK2 does NOT affect Home — it still sends CSI H.
#[test]
fn home_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Home(KeyModifiers::NONE));
    assert_eq!(bytes, b"\x1b[H");
}

/// MOK2 does NOT affect End — it still sends CSI F.
#[test]
fn end_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::End(KeyModifiers::NONE));
    assert_eq!(bytes, b"\x1b[F");
}

/// MOK2 does NOT affect Delete — it still sends CSI 3~.
#[test]
fn delete_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Delete(KeyModifiers::NONE));
    assert_eq!(bytes, b"\x1b[3~");
}

/// MOK2 does NOT affect F1 — it still sends SS3 P.
#[test]
fn f1_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::FunctionKey(1, KeyModifiers::NONE));
    assert_eq!(bytes, b"\x1bOP");
}

/// MOK2 does NOT affect modified arrows — Shift+Up still sends CSI 1;2A.
#[test]
fn shift_arrow_up_unaffected_by_mok2() {
    let mods = KeyModifiers {
        shift: true,
        ..KeyModifiers::NONE
    };
    let bytes = payload_bytes_mok2(&TerminalInput::ArrowUp(mods));
    assert_eq!(bytes, b"\x1b[1;2A");
}

/// MOK2 does NOT affect plain ASCII — 'a' still sends 0x61.
#[test]
fn ascii_a_unaffected_by_mok2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ascii(b'a'));
    assert_eq!(bytes, vec![b'a']);
}

/// When KKP is active, MOK2 is ignored — KKP takes precedence.
/// Ctrl+A with KKP flag 1 and MOK2=2 should use KKP encoding (CSI 97;5u),
/// NOT MOK2 encoding (CSI 27;5;97~).
#[test]
fn kkp_takes_precedence_over_mok2() {
    match TerminalInput::Ctrl(b'A').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        2, // MOK level 2
        ApplicationEscapeKey::Reset,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        1, // KKP flag 1
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[97;5u", "KKP must take precedence over MOK2");
        }
        other => panic!("Expected Owned(CSI 97;5u), got {other:?}"),
    }
}

/// Ctrl+A with MOK2 and AEK and mok=2 sends MOK2 encoding, not C0.
#[test]
fn ctrl_a_with_aek_and_mok2() {
    match TerminalInput::Ctrl(b'A').to_payload(
        Decckm::Ansi,
        KeypadMode::Numeric,
        2,
        ApplicationEscapeKey::Set,
        Decbkm::BackarrowSendsBs,
        Lnm::LineFeed,
        0,
        &KeyEventMeta::PRESS,
    ) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[27;5;97~");
        }
        other => panic!("Expected Owned(CSI 27;5;97~), got {other:?}"),
    }
}
