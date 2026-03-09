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

use freminal_terminal_emulator::interface::{TerminalInput, TerminalInputPayload};

/// Convenience: call `to_payload` with both mode flags `false` (normal cursor mode,
/// normal keypad mode) and unwrap the result as a `Vec<u8>`.
fn payload_bytes(input: &TerminalInput) -> Vec<u8> {
    match input.to_payload(false, false) {
        TerminalInputPayload::Single(b) => vec![b],
        TerminalInputPayload::Many(bs) => bs.to_vec(),
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
        payload_bytes(&TerminalInput::FunctionKey(1)),
        b"\x1bOP".to_vec()
    );
}

#[test]
fn f2_sends_ss3_q() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(2)),
        b"\x1bOQ".to_vec()
    );
}

#[test]
fn f3_sends_ss3_r() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(3)),
        b"\x1bOR".to_vec()
    );
}

#[test]
fn f4_sends_ss3_s() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(4)),
        b"\x1bOS".to_vec()
    );
}

#[test]
fn f5_sends_csi_15_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(5)),
        b"\x1b[15~".to_vec()
    );
}

#[test]
fn f6_sends_csi_17_tilde() {
    // Note: F6 maps to 17~ (not 16~); the VT sequence table skips 16
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(6)),
        b"\x1b[17~".to_vec()
    );
}

#[test]
fn f7_sends_csi_18_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(7)),
        b"\x1b[18~".to_vec()
    );
}

#[test]
fn f8_sends_csi_19_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(8)),
        b"\x1b[19~".to_vec()
    );
}

#[test]
fn f9_sends_csi_20_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(9)),
        b"\x1b[20~".to_vec()
    );
}

#[test]
fn f10_sends_csi_21_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(10)),
        b"\x1b[21~".to_vec()
    );
}

#[test]
fn f11_sends_csi_23_tilde() {
    // Note: F11 maps to 23~ (not 22~); the VT sequence table skips 22
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(11)),
        b"\x1b[23~".to_vec()
    );
}

#[test]
fn f12_sends_csi_24_tilde() {
    assert_eq!(
        payload_bytes(&TerminalInput::FunctionKey(12)),
        b"\x1b[24~".to_vec()
    );
}

/// An out-of-range function key should return an empty byte slice, not panic.
#[test]
fn function_key_out_of_range_does_not_panic() {
    let got = payload_bytes(&TerminalInput::FunctionKey(99));
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
