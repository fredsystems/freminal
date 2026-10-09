// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Exhaustive coverage of the strict CSI router (Task 128.3).
//!
//! A CSI sequence is identified by its private-marker prefix, intermediate
//! byte(s) and final byte *together*. The router routes only the keys in the
//! table mirrored by [`routed`]; every other syntactically valid key is
//! recognised-but-unhandled and must emit no output.
//!
//! The matrix drives every combination of
//! 5 prefixes (none, `?`, `>`, `<`, `=`) x 20 intermediate shapes (none, all
//! sixteen single intermediates `0x20..=0x2F`, and several two-byte forms) x
//! every final byte `0x40..=0x7E`, and checks:
//!
//! - a key not in the routed set emits nothing and returns
//!   `ParserOutcome::Finished`;
//! - a key in the routed set emits output (any output, `Invalid` included).
//!
//! The named regression tests below pin each misroute the old final-byte-only
//! dispatch produced.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::ParserOutcome;
use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;

/// Feed a CSI body (everything after `ESC [`) to a fresh parser; return the
/// emitted outputs and the outcome of the final byte.
fn parse(body: &[u8]) -> (Vec<TerminalOutput>, ParserOutcome) {
    let mut parser = AnsiCsiParser::new();
    let mut output = Vec::new();
    let mut last = ParserOutcome::Continue;
    for &b in body {
        last = parser.ansiparser_inner_csi(b, &mut output);
    }
    (output, last)
}

/// Mirror of the routing table in `csi_dispatch.rs`. `prefix` is the private
/// marker byte, if any; `intermediates` is the full intermediate string.
fn routed(prefix: Option<u8>, intermediates: &[u8], final_byte: u8) -> bool {
    match (prefix, intermediates) {
        (None, []) => b"ABCDEFGHIJKLMPSTXZ@`bdfgmhlncrsutx".contains(&final_byte),
        (None, b"$" | b"!") | (Some(b'?'), b"$") => final_byte == b'p',
        (None, b" ") => final_byte == b'q',
        (Some(b'?'), []) => b"hlnu".contains(&final_byte),
        (Some(b'>'), []) => b"cqmu".contains(&final_byte),
        (Some(b'<'), []) => final_byte == b'u',
        (Some(b'='), []) => b"cu".contains(&final_byte),
        _ => false,
    }
}

/// Parameter digits (after the prefix byte) for which the handler of a routed
/// key emits at least one `TerminalOutput`.
///
/// Most handlers emit output for `1`. These keys need something else, because
/// with `1` the handler returns a parser failure and pushes nothing:
///
/// - DA1 (`CSI c`), DA2 (`CSI > c`), DA3 (`CSI = c`): empty (DA1 rejects a
///   non-zero parameter).
/// - DECSTR (`CSI ! p`): empty (any parameter is rejected).
/// - SCOSC / SCORC / kitty query (`CSI s`, `CSI u`, `CSI ? u`): empty (a bare
///   `CSI s` is SCOSC; `CSI u` is SCORC).
/// - XTVERSION (`CSI > q`): empty (only `>` or `>0` is accepted).
/// - XTMODKEYS (`CSI > m`): `4;1` (`Ps = 4` selects modifyOtherKeys).
/// - DECREQTPARM (`CSI x`): `0` (the documented default; `1` also works).
fn params_for(prefix: Option<u8>, intermediates: &[u8], final_byte: u8) -> &'static [u8] {
    match (prefix, intermediates, final_byte) {
        (None, [], b'c' | b's' | b'u') | (None, b"!", b'p') => b"",
        (Some(b'?'), [], b'u') | (Some(b'>'), [], b'c' | b'q') | (Some(b'='), [], b'c') => b"",
        (Some(b'>'), [], b'm') => b"4;1",
        (None, [], b'x') => b"0",
        _ => b"1",
    }
}

fn prefix_label(prefix: Option<u8>) -> String {
    prefix.map_or_else(|| "none".to_string(), |p| char::from(p).to_string())
}

/// Every intermediate shape exercised: none, each single byte, and a spread
/// of two-byte forms (which no route accepts).
fn intermediate_shapes() -> Vec<Vec<u8>> {
    let mut shapes: Vec<Vec<u8>> = vec![Vec::new()];
    shapes.extend((0x20u8..=0x2F).map(|b| vec![b]));
    shapes.extend([
        b"!$".to_vec(),
        b"$!".to_vec(),
        b"$$".to_vec(),
        b"  ".to_vec(),
        b"$ ".to_vec(),
    ]);
    shapes
}

#[test]
fn matrix_prefix_x_intermediate_x_final() {
    let mut routed_count = 0usize;
    let mut unrouted_count = 0usize;

    for prefix in [None, Some(b'?'), Some(b'>'), Some(b'<'), Some(b'=')] {
        for intermediates in intermediate_shapes() {
            for final_byte in 0x40u8..=0x7E {
                let mut body: Vec<u8> = Vec::new();
                body.extend(prefix);
                body.extend_from_slice(params_for(prefix, &intermediates, final_byte));
                body.extend_from_slice(&intermediates);
                body.push(final_byte);

                let (output, outcome) = parse(&body);
                let label = format!(
                    "prefix={} intermediates={intermediates:?} final={:?} body={body:?}",
                    prefix_label(prefix),
                    char::from(final_byte),
                );

                if routed(prefix, &intermediates, final_byte) {
                    routed_count += 1;
                    assert!(!output.is_empty(), "routed key produced no output: {label}");
                } else {
                    unrouted_count += 1;
                    assert_eq!(output, [], "unrouted key produced output: {label}");
                    assert_eq!(
                        outcome,
                        ParserOutcome::Finished,
                        "unrouted key must finish cleanly: {label}"
                    );
                }
            }
        }
    }

    // The routed table has 34 + 1 + 1 + 1 + 4 + 1 + 4 + 1 + 2 = 49 keys.
    assert_eq!(routed_count, 49, "routed set size drifted");
    assert!(unrouted_count > 5000, "matrix did not cover enough keys");
}

// ── Regression: each misroute the final-byte-only dispatch produced ──────────

#[test]
fn csi_3_plus_t_kitty_unscroll_is_not_sd() {
    let (output, outcome) = parse(b"3+T");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::ScrollDown(_))),
        "CSI 3 + T must not scroll: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_2_hash_p_xtpushcolors_is_not_dch() {
    let (output, outcome) = parse(b"2#P");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::Delete(_))),
        "CSI 2 # P must not delete characters: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_deccara_dollar_r_is_not_decstbm() {
    for body in [&b"1;1;5;5;1$r"[..], b"1;2$r"] {
        let (output, outcome) = parse(body);
        assert!(
            !output.iter().any(|o| matches!(
                o,
                TerminalOutput::SetTopAndBottomMargins { .. }
                    | TerminalOutput::SetLeftAndRightMargins { .. }
            )),
            "DECCARA {body:?} must not set margins: {output:?}"
        );
        assert_eq!(output, [], "body {body:?}");
        assert_eq!(outcome, ParserOutcome::Finished, "body {body:?}");
    }
}

#[test]
fn csi_1_star_x_decsace_is_not_decreqtparm() {
    let (output, outcome) = parse(b"1*x");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::RequestTerminalParameters(_))),
        "CSI 1 * x must not request terminal parameters: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decrara_dollar_t_is_not_window_ops() {
    let (output, outcome) = parse(b"1;2;3;4;8$t");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::WindowManipulation(_))),
        "DECRARA must not manipulate the window: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decswbv_space_t_is_not_window_ops() {
    let (output, outcome) = parse(b"3 t");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::WindowManipulation(_))),
        "DECSWBV must not manipulate the window: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decsca_quote_q_is_not_cursor_style() {
    let (output, outcome) = parse(b"1\"q");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::CursorVisualStyle(_))),
        "DECSCA must not set a cursor style: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decll_q_is_not_cursor_style() {
    let (output, outcome) = parse(b"1q");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::CursorVisualStyle(_))),
        "DECLL must not set a cursor style: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decsmbv_space_u_is_not_restore_cursor() {
    let (output, outcome) = parse(b"3 u");
    assert!(
        !output.contains(&TerminalOutput::RestoreCursor),
        "DECSMBV must not restore the cursor: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_sl_space_at_is_not_ich() {
    let (output, outcome) = parse(b"2 @");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::InsertSpaces(_))),
        "SL must not insert spaces: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_gt_space_c_is_not_da2() {
    let (output, outcome) = parse(b"> c");
    assert!(
        !output
            .iter()
            .any(|o| matches!(o, TerminalOutput::RequestSecondaryDeviceAttributes { .. })),
        "CSI > SP c must not request DA2: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_question_s_xtsave_is_not_decslrm_or_invalid() {
    let (output, outcome) = parse(b"?s");
    assert!(
        !output.iter().any(|o| matches!(
            o,
            TerminalOutput::SetLeftAndRightMargins { .. }
                | TerminalOutput::SaveCursor
                | TerminalOutput::Invalid
        )),
        "XTSAVE must not be DECSLRM/SCOSC/Invalid: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_question_r_xtrestore_is_not_decstbm_or_invalid() {
    for body in [&b"?r"[..], b"?1049r"] {
        let (output, outcome) = parse(body);
        assert!(
            !output.iter().any(|o| matches!(
                o,
                TerminalOutput::SetTopAndBottomMargins { .. } | TerminalOutput::Invalid
            )),
            "XTRESTORE {body:?} must not be DECSTBM/Invalid: {output:?}"
        );
        assert_eq!(output, [], "body {body:?}");
        assert_eq!(outcome, ParserOutcome::Finished, "body {body:?}");
    }
}

#[test]
fn csi_misplaced_private_marker_is_unhandled() {
    // `?` after the first parameter byte cannot be classified; it is logged
    // and emits nothing rather than reaching the plain `h` handler.
    let (output, outcome) = parse(b"1;?5h");
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_routed_keys_still_dispatch() {
    // Spot checks that the strict router did not drop real sequences.
    assert_eq!(parse(b"3T").0, [TerminalOutput::ScrollDown(3)]);
    assert_eq!(parse(b"s").0, [TerminalOutput::SaveCursor]);
    assert_eq!(parse(b"u").0, [TerminalOutput::RestoreCursor]);
    assert_eq!(
        parse(b">q").0,
        [TerminalOutput::RequestDeviceNameAndVersion]
    );
    assert!(matches!(
        parse(b"2 q").0.as_slice(),
        [TerminalOutput::CursorVisualStyle(_)]
    ));
    assert_eq!(parse(b"!p").0, [TerminalOutput::SoftReset]);
}
