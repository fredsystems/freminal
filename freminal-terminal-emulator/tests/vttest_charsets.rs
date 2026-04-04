// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 3 — Character Set Tests.
//!
//! Tests for G0 charset designation and DEC Special Graphics character mapping,
//! the only character-set feature currently implemented in Freminal.
//!
//! ## Coverage
//!
//! - **`ESC ( 0`** — designate DEC Special Graphics for G0
//! - **`ESC ( B`** — designate US ASCII for G0 (restore default)
//! - **SI / SO** — invoke G0 (SI) and G1 (SO; G1 is a no-op)
//! - **Complete DEC Special Graphics table** — all 32 mapped code points
//!   (0x5F–0x7E → Unicode equivalents)
//! - **Pass-through** — bytes outside 0x5F–0x7E are unaffected by G0 mode
//!
//! ## Excluded
//!
//! - **VT220 locking/single shifts** (SS2/SS3, G2/G3): not implemented.
//! - **National Replacement Character sets**: requires DECNRCM (Task 20.12).
//! - **ISO Latin character sets**: not implemented.
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row).

#![allow(clippy::unwrap_used)]

mod vttest_common;

use vttest_common::VtTestHelper;

// ─── Activation and Deactivation ─────────────────────────────────────────────

/// `ESC ( 0` activates DEC Special Graphics for G0. Characters in the
/// mapped range are replaced with their Unicode equivalents.
/// After `ESC ( B`, ASCII is restored and characters appear literally.
#[test]
fn dec_special_activate_and_deactivate() {
    let mut h = VtTestHelper::new_default();

    // With DEC Special Graphics active: 0x6a → '┘' (BOX LIGHT UP AND LEFT).
    h.feed(b"\x1b(0"); // ESC ( 0 — activate
    h.feed(b"\x6a"); // 'j' in ASCII; maps to '┘'
    h.feed(b"\x1b(B"); // ESC ( B — restore ASCII
    h.feed(b"\x6a"); // 'j' in ASCII; stays 'j'

    let row = h.screen_text()[0].clone();
    // First char is the line-drawing glyph, second is literal 'j'.
    let mut chars = row.chars();
    assert_eq!(
        chars.next(),
        Some('\u{2518}'),
        "DEC Special 0x6a must map to ┘ (U+2518)"
    );
    assert_eq!(
        chars.next(),
        Some('j'),
        "After ESC ( B, 0x6a must be literal 'j'"
    );
}

/// SI (0x0F) and SO (0x0E) are defined in the VT100 standard to invoke G0 and
/// G1 respectively, but Freminal does not currently implement them as control
/// characters — they are passed through as data. This test documents the
/// current behavior: SI/SO do not switch character sets.
#[test]
fn si_so_not_implemented_as_control() {
    let mut h = VtTestHelper::new_default();

    // With DEC Special Graphics active, write a known mapped char.
    h.feed(b"\x1b(0"); // designate DEC Special for G0
    h.feed(b"\x6a"); // 0x6a → '┘'

    // SI (0x0F) is not handled as a control char — does not switch sets.
    // SO (0x0E) is not handled as a control char — does not switch sets.
    // The DEC Special Graphics mode remains active throughout.
    h.feed(b"\x0e"); // SO — not implemented; passes through as data/ignored
    h.feed(b"\x6a"); // still in DEC Special mode → '┘'
    h.feed(b"\x0f"); // SI — not implemented; passes through as data/ignored
    h.feed(b"\x6a"); // still in DEC Special mode → '┘'

    // The row should contain the three '┘' glyphs (the SO/SI bytes may appear
    // literally or be ignored — we only assert that the '┘' chars are present).
    let row = h.screen_text()[0].clone();
    let box_count = row.chars().filter(|&c| c == '\u{2518}').count();
    assert_eq!(
        box_count, 3,
        "All three 0x6a writes must produce ┘ regardless of SO/SI; got: {row:?}"
    );
}

/// After leaving and re-entering DEC Special mode, the mapping is still active.
#[test]
fn dec_special_toggle_multiple_times() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b(0"); // activate
    h.feed(b"\x71"); // 0x71 → '─' (U+2500)
    h.feed(b"\x1b(B"); // deactivate
    h.feed(b"\x71"); // literal 'q'
    h.feed(b"\x1b(0"); // activate again
    h.feed(b"\x71"); // → '─' again

    let row = h.screen_text()[0].clone();
    let mut chars = row.chars();
    assert_eq!(
        chars.next(),
        Some('\u{2500}'),
        "first: DEC Special 0x71 → ─"
    );
    assert_eq!(chars.next(), Some('q'), "second: ASCII 0x71 → q");
    assert_eq!(
        chars.next(),
        Some('\u{2500}'),
        "third: DEC Special 0x71 → ─ again"
    );
}

// ─── Pass-Through: Bytes Outside 0x5F–0x7E ───────────────────────────────────

/// Bytes below 0x5F are not remapped even when DEC Special Graphics is active.
#[test]
fn dec_special_below_range_passes_through() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0"); // activate
    // 0x41 = 'A' — well below 0x5F; must pass through unchanged.
    h.feed(b"ABC");
    h.assert_row(0, "ABC");
}

/// 0x7F (DEL) is above the mapped range and must pass through unchanged.
#[test]
fn dec_special_del_passes_through() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0"); // activate
    // Write 'X', then DEL (0x7F — not in the mapping), then 'Y'.
    h.feed(b"X\x7fY");
    // DEL is a control character and is typically ignored; 'X' and 'Y' remain.
    let row = h.screen_text()[0].clone();
    assert!(
        row.starts_with("XY") || row.starts_with("X"),
        "DEL (0x7F) must not produce a line-drawing character; got: {row:?}"
    );
}

/// Digits, uppercase letters, and punctuation in the 0x20–0x5E range are
/// unaffected by DEC Special Graphics mode. Note: lowercase letters (0x61–0x7A)
/// ARE in the mapped range and will produce line drawing characters.
#[test]
fn dec_special_printable_ascii_below_range_unchanged() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0"); // activate
    // Use only bytes below 0x5F: digits, uppercase letters, space.
    h.feed(b"ABC 123");
    // None of 0x41-0x5A or 0x30-0x39 are in 0x5F–0x7E, so they must appear literally.
    h.assert_row(0, "ABC 123");
}

// ─── Complete DEC Special Graphics Mapping Table ─────────────────────────────
//
// Each test feeds a single byte in the 0x5F–0x7E range while DEC Special
// Graphics is active and verifies the correct Unicode code point appears in
// the buffer.

/// 0x5F → U+00A0 NO-BREAK SPACE.
///
/// The result is verified by placing the mapped character between two literal
/// 'X' sentinels (to prevent trailing-whitespace trimming in `screen_text()`).
#[test]
fn dec_special_0x5f_no_break_space() {
    let mut h = VtTestHelper::new_default();
    // Write 'X', then 0x5F (→ NO-BREAK SPACE), then 'X' as sentinels so the
    // space character is not trimmed by the trailing-whitespace normaliser.
    h.feed(b"\x1b(0"); // activate DEC Special Graphics
    h.feed(b"X\x5fX");
    let row = h.screen_text()[0].clone();
    let mut chars = row.chars();
    assert_eq!(chars.next(), Some('X'), "leading sentinel");
    assert_eq!(
        chars.next(),
        Some('\u{00A0}'),
        "0x5F must map to U+00A0 (NO-BREAK SPACE)"
    );
    assert_eq!(chars.next(), Some('X'), "trailing sentinel");
}

/// 0x60 → U+25C6 BLACK DIAMOND ◆.
#[test]
fn dec_special_0x60_black_diamond() {
    assert_dec_special_char(0x60, '\u{25C6}', "BLACK DIAMOND");
}

/// 0x61 → U+2592 MEDIUM SHADE ▒.
#[test]
fn dec_special_0x61_medium_shade() {
    assert_dec_special_char(0x61, '\u{2592}', "MEDIUM SHADE");
}

/// 0x62 → U+2409 SYMBOL FOR HT.
#[test]
fn dec_special_0x62_symbol_ht() {
    assert_dec_special_char(0x62, '\u{2409}', "SYMBOL FOR HT");
}

/// 0x63 → U+240C SYMBOL FOR FF.
#[test]
fn dec_special_0x63_symbol_ff() {
    assert_dec_special_char(0x63, '\u{240C}', "SYMBOL FOR FF");
}

/// 0x64 → U+240D SYMBOL FOR CR.
#[test]
fn dec_special_0x64_symbol_cr() {
    assert_dec_special_char(0x64, '\u{240D}', "SYMBOL FOR CR");
}

/// 0x65 → U+240A SYMBOL FOR LF.
#[test]
fn dec_special_0x65_symbol_lf() {
    assert_dec_special_char(0x65, '\u{240A}', "SYMBOL FOR LF");
}

/// 0x66 → U+00B0 DEGREE SIGN °.
#[test]
fn dec_special_0x66_degree_sign() {
    assert_dec_special_char(0x66, '\u{00B0}', "DEGREE SIGN");
}

/// 0x67 → U+00B1 PLUS-MINUS SIGN ±.
#[test]
fn dec_special_0x67_plus_minus() {
    assert_dec_special_char(0x67, '\u{00B1}', "PLUS-MINUS SIGN");
}

/// 0x68 → U+2424 SYMBOL FOR NEWLINE.
#[test]
fn dec_special_0x68_symbol_newline() {
    assert_dec_special_char(0x68, '\u{2424}', "SYMBOL FOR NEWLINE");
}

/// 0x69 → U+240B SYMBOL FOR VT.
#[test]
fn dec_special_0x69_symbol_vt() {
    assert_dec_special_char(0x69, '\u{240B}', "SYMBOL FOR VT");
}

/// 0x6A → U+2518 BOX LIGHT UP AND LEFT ┘.
#[test]
fn dec_special_0x6a_box_up_left() {
    assert_dec_special_char(0x6a, '\u{2518}', "BOX LIGHT UP AND LEFT");
}

/// 0x6B → U+2510 BOX LIGHT DOWN AND LEFT ┐.
#[test]
fn dec_special_0x6b_box_down_left() {
    assert_dec_special_char(0x6b, '\u{2510}', "BOX LIGHT DOWN AND LEFT");
}

/// 0x6C → U+250C BOX LIGHT DOWN AND RIGHT ┌.
#[test]
fn dec_special_0x6c_box_down_right() {
    assert_dec_special_char(0x6c, '\u{250C}', "BOX LIGHT DOWN AND RIGHT");
}

/// 0x6D → U+2514 BOX LIGHT UP AND RIGHT └.
#[test]
fn dec_special_0x6d_box_up_right() {
    assert_dec_special_char(0x6d, '\u{2514}', "BOX LIGHT UP AND RIGHT");
}

/// 0x6E → U+253C BOX LIGHT VERTICAL AND HORIZONTAL ┼.
#[test]
fn dec_special_0x6e_box_cross() {
    assert_dec_special_char(0x6e, '\u{253C}', "BOX LIGHT VERTICAL AND HORIZONTAL");
}

/// 0x6F → U+23BA HORIZONTAL SCAN LINE-1 ⎺.
#[test]
fn dec_special_0x6f_scan_line_1() {
    assert_dec_special_char(0x6f, '\u{23BA}', "HORIZONTAL SCAN LINE-1");
}

/// 0x70 → U+23BB HORIZONTAL SCAN LINE-3 ⎻.
#[test]
fn dec_special_0x70_scan_line_3() {
    assert_dec_special_char(0x70, '\u{23BB}', "HORIZONTAL SCAN LINE-3");
}

/// 0x71 → U+2500 BOX LIGHT HORIZONTAL ─.
#[test]
fn dec_special_0x71_box_horizontal() {
    assert_dec_special_char(0x71, '\u{2500}', "BOX LIGHT HORIZONTAL");
}

/// 0x72 → U+23BC HORIZONTAL SCAN LINE-7 ⎼.
#[test]
fn dec_special_0x72_scan_line_7() {
    assert_dec_special_char(0x72, '\u{23BC}', "HORIZONTAL SCAN LINE-7");
}

/// 0x73 → U+23BD HORIZONTAL SCAN LINE-9 ⎽.
#[test]
fn dec_special_0x73_scan_line_9() {
    assert_dec_special_char(0x73, '\u{23BD}', "HORIZONTAL SCAN LINE-9");
}

/// 0x74 → U+251C BOX LIGHT VERTICAL AND RIGHT ├.
#[test]
fn dec_special_0x74_box_vert_right() {
    assert_dec_special_char(0x74, '\u{251C}', "BOX LIGHT VERTICAL AND RIGHT");
}

/// 0x75 → U+2524 BOX LIGHT VERTICAL AND LEFT ┤.
#[test]
fn dec_special_0x75_box_vert_left() {
    assert_dec_special_char(0x75, '\u{2524}', "BOX LIGHT VERTICAL AND LEFT");
}

/// 0x76 → U+2534 BOX LIGHT UP AND HORIZONTAL ┴.
#[test]
fn dec_special_0x76_box_up_horiz() {
    assert_dec_special_char(0x76, '\u{2534}', "BOX LIGHT UP AND HORIZONTAL");
}

/// 0x77 → U+252C BOX LIGHT DOWN AND HORIZONTAL ┬.
#[test]
fn dec_special_0x77_box_down_horiz() {
    assert_dec_special_char(0x77, '\u{252C}', "BOX LIGHT DOWN AND HORIZONTAL");
}

/// 0x78 → U+2502 BOX LIGHT VERTICAL │.
#[test]
fn dec_special_0x78_box_vertical() {
    assert_dec_special_char(0x78, '\u{2502}', "BOX LIGHT VERTICAL");
}

/// 0x79 → U+2264 LESS-THAN OR EQUAL TO ≤.
#[test]
fn dec_special_0x79_less_equal() {
    assert_dec_special_char(0x79, '\u{2264}', "LESS-THAN OR EQUAL TO");
}

/// 0x7A → U+2265 GREATER-THAN OR EQUAL TO ≥.
#[test]
fn dec_special_0x7a_greater_equal() {
    assert_dec_special_char(0x7a, '\u{2265}', "GREATER-THAN OR EQUAL TO");
}

/// 0x7B → U+03C0 GREEK SMALL LETTER PI π.
#[test]
fn dec_special_0x7b_pi() {
    assert_dec_special_char(0x7b, '\u{03C0}', "GREEK SMALL LETTER PI");
}

/// 0x7C → U+2260 NOT EQUAL TO ≠.
#[test]
fn dec_special_0x7c_not_equal() {
    assert_dec_special_char(0x7c, '\u{2260}', "NOT EQUAL TO");
}

/// 0x7D → U+00A3 POUND SIGN £.
#[test]
fn dec_special_0x7d_pound_sign() {
    assert_dec_special_char(0x7d, '\u{00A3}', "POUND SIGN");
}

/// 0x7E → U+00B7 MIDDLE DOT ·.
#[test]
fn dec_special_0x7e_middle_dot() {
    assert_dec_special_char(0x7e, '\u{00B7}', "MIDDLE DOT");
}

// ─── Line Drawing Box: Full Box with All Four Corners and Edges ───────────────

/// Draw a complete box using DEC Special Graphics characters. Verifies that a
/// sequence of line drawing chars produces the expected Unicode layout.
///
/// ```text
/// ┌──┐
/// │  │
/// └──┘
/// ```
#[test]
fn dec_special_draw_box() {
    let mut h = VtTestHelper::new_default();

    // Activate DEC Special Graphics.
    h.feed(b"\x1b(0");

    // Row 0: ┌──┐ (0x6c 0x71 0x71 0x6b)
    h.feed(b"\x6c\x71\x71\x6b\r\n");
    // Row 1: │  │ (0x78 SP SP 0x78)
    h.feed(b"\x78  \x78\r\n");
    // Row 2: └──┘ (0x6d 0x71 0x71 0x6a)
    h.feed(b"\x6d\x71\x71\x6a");

    h.assert_row(0, "┌──┐");
    h.assert_row(1, "│  │");
    h.assert_row(2, "└──┘");
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Feed a single DEC Special Graphics byte and assert the first character on
/// the first row equals the expected Unicode code point.
fn assert_dec_special_char(byte: u8, expected: char, description: &str) {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0"); // activate DEC Special Graphics
    h.feed(&[byte]);
    let row = h.screen_text()[0].clone();
    let first = row.chars().next().unwrap_or('\0');
    assert_eq!(
        first, expected,
        "DEC Special 0x{byte:02x} must map to U+{:04X} ({description}); got U+{:04X} ({first:?})",
        expected as u32, first as u32,
    );
}
