// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! vttest Menu 3 — Character Set Tests.
//!
//! Tests for G0–G3 designation, the locking shifts (SI, SO, LS2, LS3, and the
//! GR forms), the single shifts (SS2, SS3) and the DEC Special Graphics and UK
//! sets.
//!
//! ## Coverage
//!
//! - **`ESC ( 0` / `ESC ( B`** — designate DEC Special Graphics / US ASCII
//!   for G0, and the same for G1–G3 (`ESC ) * +`)
//! - **SI / SO / `ESC n` / `ESC o`** — invoke G0, G1, G2 and G3 into GL
//! - **GR** — `ESC ~ } |` are tracked but never applied (UTF-8)
//! - **SS2 / SS3** — `ESC N` / `ESC O`, and 0x8E / 0x8F under S8C1T, each
//!   affect exactly one character
//! - **DECSC / DECRC, RIS, DECSTR, VT52** — what saves, restores and resets
//!   the character-set state
//! - **Complete DEC Special Graphics table** — all 32 mapped code points
//!   (0x5F–0x7E → Unicode equivalents)
//! - **Pass-through** — bytes outside 0x5F–0x7E are unaffected by DEC Special
//!
//! ## Excluded
//!
//! - **National Replacement Character sets** other than UK, and the
//!   96-character designators (`ESC - . /`): not implemented. An unknown
//!   designator final is ignored and the slot keeps its set.
//! - **ISO Latin character sets**: not implemented.
//!
//! All cursor positions in the helper API are **0-indexed** (`x` = column,
//! `y` = row).

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

/// SO (0x0E) invokes G1 into GL and SI (0x0F) invokes G0. Neither appears as
/// visible data. With DEC Special in G0 only, SO switches GL to the (ASCII) G1,
/// so the next byte is literal, and SI switches back.
#[test]
fn si_so_switch_gl_between_g0_and_g1() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b(0"); // G0 = DEC Special; G1 stays ASCII
    h.feed(b"\x6a"); // GL = G0 → '┘'
    h.feed(b"\x0e"); // SO — GL = G1 (ASCII)
    h.feed(b"\x6a"); // literal 'j'
    h.feed(b"\x0f"); // SI — GL = G0 again
    h.feed(b"\x6a"); // '┘'

    let row = h.screen_text()[0].clone();
    assert_eq!(
        row, "\u{2518}j\u{2518}",
        "SO/SI must switch GL and leave no artifacts; got: {row:?}"
    );
}

/// The classic way to draw a box: designate DEC Special Graphics into G1 with
/// `ESC ) 0`, then SO to use it and SI to leave it.
#[test]
fn so_after_designating_g1_draws_a_box() {
    let mut h = VtTestHelper::new_default();

    h.feed(b"\x1b)0");
    h.feed(b"lqqk"); // GL = G0 (ASCII) still: literal
    h.feed(b"\x0elqqk\x0f"); // SO … SI
    h.feed(b"lqqk");

    h.assert_row(0, "lqqk\u{250c}\u{2500}\u{2500}\u{2510}lqqk");
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
        row.starts_with("XY") || row.starts_with('X'),
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

// ─── Designation and invocation, every slot ──────────────────────────────────

const BOX_HORIZONTAL: &str = "\u{2500}";
const POUND: &str = "\u{a3}";

/// Designate `set` (the final byte) into the slot `intermediate` names, check
/// that nothing changes while GL still invokes G0, then invoke the slot with
/// `invoke` and check the mapping, then invoke G0 again with SI.
fn assert_slot_maps_through_gl(intermediate: u8, invoke: &[u8], probe: u8, mapped: &str) {
    let mut h = VtTestHelper::new_default();
    h.feed(&[
        0x1b,
        intermediate,
        if mapped == POUND { b'A' } else { b'0' },
    ]);
    h.feed(&[probe]);
    h.feed(invoke);
    h.feed(&[probe]);
    h.feed(b"\x0f");
    h.feed(&[probe]);

    let literal = char::from(probe).to_string();
    let expected = if intermediate == b'(' {
        // G0 is GL from the start, so the first byte is already mapped, and
        // invoking G0 again (ESC ( has no invoke) leaves it mapped.
        format!("{mapped}{mapped}{mapped}")
    } else {
        format!("{literal}{mapped}{literal}")
    };
    h.assert_row(0, &expected);
}

#[test]
fn g0_designation_maps_through_gl_immediately() {
    assert_slot_maps_through_gl(b'(', b"", b'q', BOX_HORIZONTAL);
}

#[test]
fn g1_designation_maps_after_so() {
    assert_slot_maps_through_gl(b')', b"\x0e", b'q', BOX_HORIZONTAL);
}

#[test]
fn g2_designation_maps_after_ls2() {
    assert_slot_maps_through_gl(b'*', b"\x1bn", b'q', BOX_HORIZONTAL);
}

#[test]
fn g3_designation_maps_after_ls3() {
    assert_slot_maps_through_gl(b'+', b"\x1bo", b'q', BOX_HORIZONTAL);
}

#[test]
fn uk_set_maps_hash_to_pound_in_every_slot() {
    assert_slot_maps_through_gl(b'(', b"", b'#', POUND);
    assert_slot_maps_through_gl(b')', b"\x0e", b'#', POUND);
    assert_slot_maps_through_gl(b'*', b"\x1bn", b'#', POUND);
    assert_slot_maps_through_gl(b'+', b"\x1bo", b'#', POUND);
}

#[test]
fn uk_set_only_changes_the_hash() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(A");
    h.feed(b"a#~$");
    h.assert_row(0, "a\u{a3}~$");
}

/// `ESC + 0` designates G3 and nothing else: G0 stays ASCII.
#[test]
fn esc_plus_0_does_not_touch_g0() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b+0");
    h.feed(b"q");
    h.assert_row(0, "q");
}

/// Designating a set into a slot GL is not using does not change what is drawn.
#[test]
fn designating_an_unused_slot_does_not_affect_gl() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b)0\x1b*0\x1b+0");
    h.feed(b"q");
    h.assert_row(0, "q");
}

/// The locking shifts persist: every following character is mapped.
#[test]
fn locking_shift_persists_until_changed() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b*0\x1bn");
    h.feed(b"qqq");
    h.feed(b"\x0f");
    h.feed(b"qqq");
    h.assert_row(0, "\u{2500}\u{2500}\u{2500}qqq");
}

/// An unknown final leaves the slot as it was; it does not reset it to ASCII.
#[test]
fn unknown_designator_final_leaves_the_slot_unchanged() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b)0"); // G1 = DEC Special
    h.feed(b"\x1b)Z"); // unknown final: ignored
    h.feed(b"\x1b)5"); // Finnish: not implemented, ignored
    h.feed(b"\x0eq");
    h.assert_row(0, BOX_HORIZONTAL);
}

/// An unknown final consumes only its own bytes: text after it is printed.
#[test]
fn unknown_designator_final_does_not_swallow_text() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"a\x1b(Zb");
    h.assert_row(0, "ab");
}

// ─── GR is tracked, never applied ────────────────────────────────────────────

/// `ESC ~ } |` invoke G1/G2/G3 into GR. GR is never applied: UTF-8 text with
/// bytes at or above 0x80 prints as itself, and GL is untouched.
#[test]
fn gr_invocation_is_stored_but_not_applied() {
    for invoke in [&b"\x1b~"[..], b"\x1b}", b"\x1b|"] {
        let mut h = VtTestHelper::new_default();
        h.feed(b"\x1b)0\x1b*0\x1b+0");
        h.feed(invoke);
        h.feed("\u{e9}q\u{3c0}".as_bytes());
        h.assert_row(0, "\u{e9}q\u{3c0}");
    }
}

// ─── Single shifts ───────────────────────────────────────────────────────────

#[test]
fn seven_bit_ss2_affects_exactly_one_character() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b*0");
    h.feed(b"\x1bNqqq");
    h.assert_row(0, "\u{2500}qq");
}

#[test]
fn seven_bit_ss3_affects_exactly_one_character() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b+0");
    h.feed(b"\x1bOqqq");
    h.assert_row(0, "\u{2500}qq");
}

#[test]
fn single_shift_overrides_gl_for_one_character_and_leaves_gl_locked() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0\x1b*B"); // G0 = DEC Special, G2 = ASCII
    h.feed(b"q\x1bNqq");
    h.assert_row(0, "\u{2500}q\u{2500}");
}

#[test]
fn single_shift_applies_across_separate_reads() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b*0");
    h.feed(b"\x1bN");
    h.feed(b"q");
    h.feed(b"q");
    h.assert_row(0, "\u{2500}q");
}

#[test]
fn single_shift_is_consumed_by_a_non_ascii_character() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b*0");
    h.feed(b"\x1bN");
    h.feed("\u{e9}qq".as_bytes());
    h.assert_row(0, "\u{e9}qq");
}

#[test]
fn eight_bit_ss2_affects_exactly_one_character_under_s8c1t() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b G"); // S8C1T
    h.feed(b"\x1b*0");
    h.feed(b"\x8eqqq");
    h.assert_row(0, "\u{2500}qq");
}

#[test]
fn eight_bit_ss3_affects_exactly_one_character_under_s8c1t() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b G"); // S8C1T
    h.feed(b"\x1b+0");
    h.feed(b"\x8fqqq");
    h.assert_row(0, "\u{2500}qq");
}

// ─── DECSC / DECRC ───────────────────────────────────────────────────────────

/// DECSC saves G0–G3, GL and the pending single shift; DECRC restores them.
#[test]
fn decsc_decrc_round_trips_the_charset_state_including_the_single_shift() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b*A"); // G2 = UK
    h.feed(b"\x1b)0"); // G1 = DEC Special
    h.feed(b"\x0e"); // GL = G1
    h.feed(b"\x1bN"); // single shift pending (G2)
    h.feed(b"\x1b7"); // DECSC

    // Disturb everything: ASCII G1, GL = G0, and consume the shift.
    h.feed(b"\x1b)B\x0fx");
    h.feed(b"\x1b8"); // DECRC: back to col 0 with the saved state

    // The restored single shift maps the first '#' through G2 (UK); the
    // restored GL = G1 (DEC Special) maps 'q' and leaves a second '#' alone.
    h.feed(b"#q#");
    h.assert_row(0, "\u{a3}\u{2500}#");
}

#[test]
fn decrc_restores_the_gl_invocation_and_designators() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b+0\x1bo"); // G3 = DEC Special, GL = G3
    h.feed(b"\x1b7");
    h.feed(b"\x0f\x1b+B"); // GL = G0, G3 = ASCII
    h.feed(b"\x1b8");
    h.feed(b"q");
    h.assert_row(0, "\u{2500}");
}

#[test]
fn decrc_with_nothing_saved_restores_the_default_charset_state() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b)0\x1b*0\x1b+0\x0e\x1bN");
    h.feed(b"\x1b8"); // nothing saved on this screen
    h.feed(b"qq");
    h.assert_row(0, "qq");
}

// ─── RIS and DECSTR ──────────────────────────────────────────────────────────

#[test]
fn ris_resets_every_slot_gl_and_the_single_shift() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0\x1b)0\x1b*0\x1b+0\x0e\x1b}\x1bN");
    h.feed(b"\x1bc");
    h.feed(b"qq");
    h.assert_row(0, "qq");

    // And each slot is back to ASCII, not just the invoked one.
    for invoke in [&b"\x0e"[..], b"\x1bn", b"\x1bo", b"\x0f"] {
        h.feed(invoke);
        h.feed(b"q");
    }
    h.assert_row(0, "qqqqqq");
}

#[test]
fn ris_clears_the_saved_charset_state() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0\x1b7"); // DECSC with DEC Special in G0
    h.feed(b"\x1bc");
    h.feed(b"\x1b8"); // nothing saved: defaults
    h.feed(b"q");
    h.assert_row(0, "q");
}

#[test]
fn decstr_resets_every_slot_gl_and_the_single_shift() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b(0\x1b)0\x1b*0\x1b+0\x0e\x1bN");
    h.feed(b"\x1b[!p");
    h.feed(b"qq");
    for invoke in [&b"\x0e"[..], b"\x1bn", b"\x1bo", b"\x0f"] {
        h.feed(invoke);
        h.feed(b"q");
    }
    h.assert_row(0, "qqqqqq");
}

#[test]
fn decstr_sets_the_saved_charset_state_to_the_default() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b)0\x0e\x1b7"); // DECSC with GL = G1 (DEC Special)
    h.feed(b"\x1b[!p"); // DECSTR records the default state as the saved one
    h.feed(b"\x1b)0\x0e"); // designate and invoke again
    h.feed(b"\x1b8"); // DECRC: defaults
    h.feed(b"q");
    h.assert_row(0, "q");
}

// ─── VT52 ────────────────────────────────────────────────────────────────────

/// Entering VT52 mode saves the ANSI charset state and starts from the
/// defaults; leaving restores it, and `ESC F` / `ESC G` do not leak out.
#[test]
fn vt52_saves_and_restores_the_ansi_charset_state() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b)0\x0e"); // ANSI: G1 = DEC Special, GL = G1
    h.feed(b"q"); // '─'

    h.feed(b"\x1b[?2l"); // enter VT52
    h.feed(b"\x1bH"); // home
    h.feed(b"q"); // defaults in VT52: literal 'q'
    h.feed(b"\x1bF"); // graphics on (G0 = DEC Special)
    h.feed(b"q"); // '─'
    h.feed(b"\x1b<"); // back to ANSI

    h.feed(b"\x1b[H");
    h.feed(b"q"); // restored: GL = G1 (DEC Special) → '─'
    h.feed(b"\x0fq"); // SI → G0, which the VT52 ESC F did not leak into: 'q'

    h.assert_row(0, "\u{2500}q");
}

#[test]
fn vt52_esc_g_selects_ascii_in_g0() {
    let mut h = VtTestHelper::new_default();
    h.feed(b"\x1b[?2l");
    h.feed(b"\x1bH\x1bFq\x1bGq");
    h.assert_row(0, "\u{2500}q");
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
