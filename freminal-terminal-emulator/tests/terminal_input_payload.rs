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

use freminal_terminal_emulator::input::{KeyModifiers, TerminalInput, TerminalInputPayload};

/// Convenience: call `to_payload` with both mode flags `false` (normal cursor mode,
/// normal keypad mode) and unwrap the result as a `Vec<u8>`.
fn payload_bytes(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(false, false, 0, false, true) {
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
    match input.to_payload(true, false, 0, false, true) {
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
// modifyOtherKeys level 2 encoding: Ctrl+letter → CSI 27 ; 5 ; CODE ~
// ---------------------------------------------------------------------------

/// Convenience: call `to_payload` with `modify_other_keys = 2`.
fn payload_bytes_mok2(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(false, false, 2, false, true) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
        TerminalInputPayload::Owned(bs) => bs,
    }
}

/// At modifyOtherKeys level 2, Ctrl+A should produce `ESC[27;5;65~`.
#[test]
fn ctrl_a_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'a'));
    assert_eq!(bytes, b"\x1b[27;5;65~");
}

/// At modifyOtherKeys level 2, Ctrl+C should produce `ESC[27;5;67~`.
#[test]
fn ctrl_c_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'c'));
    assert_eq!(bytes, b"\x1b[27;5;67~");
}

/// At modifyOtherKeys level 2, Ctrl+Z should produce `ESC[27;5;90~`.
#[test]
fn ctrl_z_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'z'));
    assert_eq!(bytes, b"\x1b[27;5;90~");
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
    match TerminalInput::Ctrl(b'A').to_payload(false, false, 1, false, true) {
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

/// Convenience: call `to_payload` with `application_escape_key = true`.
fn payload_bytes_aek(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(false, false, 0, true, true) {
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
    match TerminalInput::Ctrl(b'B').to_payload(false, false, 1, false, true) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x02),
        other => panic!("Expected Single(0x02), got {other:?}"),
    }
}

/// At modifyOtherKeys level 1, Ctrl+Z should still produce 0x1A.
#[test]
fn ctrl_z_modify_other_keys_level_1() {
    match TerminalInput::Ctrl(b'Z').to_payload(false, false, 1, false, true) {
        TerminalInputPayload::Single(b) => assert_eq!(b, 0x1A),
        other => panic!("Expected Single(0x1A), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// modifyOtherKeys level 2: additional Ctrl+letter boundary tests
// ---------------------------------------------------------------------------

/// At level 2, Ctrl+B → `ESC[27;5;66~`
#[test]
fn ctrl_b_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'b'));
    assert_eq!(bytes, b"\x1b[27;5;66~");
}

/// At level 2, Ctrl+Y → `ESC[27;5;89~`
#[test]
fn ctrl_y_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'y'));
    assert_eq!(bytes, b"\x1b[27;5;89~");
}

/// At level 2, Ctrl+M → `ESC[27;5;77~` (not bare CR)
#[test]
fn ctrl_m_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'm'));
    assert_eq!(bytes, b"\x1b[27;5;77~");
}

/// At level 2, Ctrl+[ → `ESC[27;5;91~` (CODE = 91, uppercase of `[`)
#[test]
fn ctrl_open_bracket_modify_other_keys_level_2() {
    let bytes = payload_bytes_mok2(&TerminalInput::Ctrl(b'['));
    assert_eq!(bytes, b"\x1b[27;5;91~");
}

/// At level 2, Ctrl+Space → `ESC[27;5;32~` (CODE = 32, uppercase of ` `)
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
    match TerminalInput::Escape.to_payload(false, false, 2, true, true) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[27;1;27~");
        }
        other => panic!("Expected Owned(CSI 27;1;27~), got {other:?}"),
    }
}

/// When both modifyOtherKeys >= 2 AND application_escape_key are active,
/// Ctrl+C should use the MOK2 encoding (CSI 27;5;67~), not the AEK one.
#[test]
fn ctrl_c_with_both_mok2_and_aek() {
    match TerminalInput::Ctrl(b'c').to_payload(false, false, 2, true, true) {
        TerminalInputPayload::Owned(bs) => {
            assert_eq!(bs, b"\x1b[27;5;67~");
        }
        other => panic!("Expected Owned(CSI 27;5;67~), got {other:?}"),
    }
}

/// Ctrl+A with application_escape_key=true and mok=0 — AEK has no effect on Ctrl.
#[test]
fn ctrl_a_with_aek_and_mok0() {
    match TerminalInput::Ctrl(b'A').to_payload(false, false, 0, true, true) {
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
    match TerminalInput::Backspace.to_payload(false, false, 0, false, true) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(b, 0x08, "DECBKM set: Backspace must send BS (0x08)")
        }
        other => panic!("Expected Single(0x08), got {other:?}"),
    }
}

/// Backspace with DECBKM reset: sends DEL (0x7F).
#[test]
fn backspace_decbkm_reset_sends_del() {
    match TerminalInput::Backspace.to_payload(false, false, 0, false, false) {
        TerminalInputPayload::Single(b) => {
            assert_eq!(b, 0x7F, "DECBKM reset: Backspace must send DEL (0x7F)")
        }
        other => panic!("Expected Single(0x7F), got {other:?}"),
    }
}
