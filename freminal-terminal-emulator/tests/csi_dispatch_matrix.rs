// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Exhaustive coverage of the strict CSI router (Task 128.3).
//!
//! A CSI sequence is identified by its private-marker prefix, intermediate
//! byte(s) and final byte *together*. The router routes only the keys listed
//! in [`routes`]; every other syntactically valid key is recognised-but-
//! unhandled and must emit no output.
//!
//! The matrix drives every combination of
//! 5 prefixes (none, `?`, `>`, `<`, `=`) x 22 intermediate shapes (none, all
//! sixteen single intermediates `0x20..=0x2F`, and five two-byte forms) x
//! every final byte `0x40..=0x7E`, and checks:
//!
//! - a key not in the routed table emits nothing and returns
//!   `ParserOutcome::Finished`, both with a parameter and with none;
//! - a key in the routed table emits *exactly* the output its handler
//!   produces for the table's parameters, and returns
//!   `ParserOutcome::Finished`. Pinning the exact `TerminalOutput` is what
//!   catches two routed keys with their handlers swapped (for example the four
//!   kitty keyboard `u` keys, or DA2 against DA3).
//!
//! The named regression tests below pin each misroute the old final-byte-only
//! dispatch produced.

use freminal_common::buffer_states::mode::Mode;
use freminal_common::buffer_states::modes::dectcem::Dectcem;
use freminal_common::buffer_states::modes::irm::Irm;
use freminal_common::buffer_states::terminal_output::{TabClearMode, TerminalOutput};
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::cursor::CursorVisualStyle;
use freminal_common::sgr::SelectGraphicRendition;
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

/// One routed CSI key and what its handler must produce.
///
/// `params` are the parameter bytes *after* the private-marker prefix. The
/// expectations are derived from each handler's source, not from observed
/// output. Every route is expected to yield `ParserOutcome::Finished`, as
/// every handler does for valid parameters.
struct Route {
    prefix: Option<u8>,
    intermediates: &'static [u8],
    final_byte: u8,
    params: &'static [u8],
    expected: Vec<TerminalOutput>,
}

fn route(
    prefix: Option<u8>,
    intermediates: &'static [u8],
    final_byte: u8,
    params: &'static [u8],
    expected: Vec<TerminalOutput>,
) -> Route {
    Route {
        prefix,
        intermediates,
        final_byte,
        params,
        expected,
    }
}

fn rel(x: Option<i32>, y: Option<i32>) -> TerminalOutput {
    TerminalOutput::SetCursorPosRel { x, y }
}

fn pos(x: Option<usize>, y: Option<usize>) -> TerminalOutput {
    TerminalOutput::SetCursorPos { x, y }
}

/// The routed table, in three groups. Counts are `3` wherever a handler maps
/// `0`/`1` to its default of `1`, so a handler that ignored its parameter would
/// be caught.
fn routes() -> Vec<Route> {
    let mut all = cursor_and_edit_routes();
    all.extend(mode_and_report_routes());
    all.extend(prefixed_and_intermediate_routes());
    all
}

/// Plain keys that move the cursor or edit text (24 keys).
fn cursor_and_edit_routes() -> Vec<Route> {
    vec![
        route(None, b"", b'A', b"3", vec![rel(None, Some(-3))]),
        route(None, b"", b'B', b"3", vec![rel(None, Some(3))]),
        route(None, b"", b'C', b"3", vec![rel(Some(3), None)]),
        route(None, b"", b'D', b"3", vec![rel(Some(-3), None)]),
        route(
            None,
            b"",
            b'E',
            b"3",
            vec![rel(None, Some(3)), pos(Some(1), None)],
        ),
        route(
            None,
            b"",
            b'F',
            b"3",
            vec![rel(None, Some(-3)), pos(Some(1), None)],
        ),
        route(None, b"", b'G', b"3", vec![pos(Some(3), None)]),
        route(None, b"", b'H', b"3;4", vec![pos(Some(4), Some(3))]),
        route(
            None,
            b"",
            b'I',
            b"3",
            vec![TerminalOutput::CursorForwardTab(3)],
        ),
        route(None, b"", b'J', b"2", vec![TerminalOutput::ClearDisplay]),
        route(None, b"", b'K', b"2", vec![TerminalOutput::ClearLine]),
        route(None, b"", b'L', b"3", vec![TerminalOutput::InsertLines(3)]),
        route(None, b"", b'M', b"3", vec![TerminalOutput::DeleteLines(3)]),
        route(None, b"", b'P', b"3", vec![TerminalOutput::Delete(3)]),
        route(None, b"", b'S', b"3", vec![TerminalOutput::ScrollUp(3)]),
        route(None, b"", b'T', b"3", vec![TerminalOutput::ScrollDown(3)]),
        route(None, b"", b'X', b"3", vec![TerminalOutput::Erase(3)]),
        route(
            None,
            b"",
            b'Z',
            b"3",
            vec![TerminalOutput::CursorBackwardTab(3)],
        ),
        route(None, b"", b'@', b"3", vec![TerminalOutput::InsertSpaces(3)]),
        // HPA is CHA under another final byte.
        route(None, b"", b'`', b"3", vec![pos(Some(3), None)]),
        route(
            None,
            b"",
            b'b',
            b"3",
            vec![TerminalOutput::RepeatCharacter(3)],
        ),
        route(None, b"", b'd', b"3", vec![pos(None, Some(3))]),
        // HVP is CUP under another final byte.
        route(None, b"", b'f', b"3;4", vec![pos(Some(4), Some(3))]),
        route(
            None,
            b"",
            b'g',
            b"3",
            vec![TerminalOutput::TabClear(TabClearMode::AllCharacter)],
        ),
    ]
}

/// Plain keys for SGR, modes, reports, margins and window operations (10
/// keys).
fn mode_and_report_routes() -> Vec<Route> {
    let wm = WindowManipulation::ResizeWindowToLinesAndColumns(24, 80);
    vec![
        route(
            None,
            b"",
            b'm',
            b"1",
            vec![TerminalOutput::Sgr(SelectGraphicRendition::Bold)],
        ),
        // ANSI (not DEC private) set/reset: IRM is ANSI mode 4.
        route(
            None,
            b"",
            b'h',
            b"4",
            vec![TerminalOutput::Mode(Mode::Irm(Irm::Insert))],
        ),
        route(
            None,
            b"",
            b'l',
            b"4",
            vec![TerminalOutput::Mode(Mode::Irm(Irm::Replace))],
        ),
        route(
            None,
            b"",
            b'n',
            b"5",
            vec![TerminalOutput::DeviceStatusReport],
        ),
        route(
            None,
            b"",
            b'c',
            b"",
            vec![TerminalOutput::RequestDeviceAttributes],
        ),
        route(
            None,
            b"",
            b'r',
            b"3;9",
            vec![TerminalOutput::SetTopAndBottomMargins {
                top_margin: 3,
                bottom_margin: 9,
            }],
        ),
        // With no parameters `CSI s` is SCOSC; with parameters it is DECSLRM
        // (covered by `csi_s_with_parameters_is_decslrm`).
        route(None, b"", b's', b"", vec![TerminalOutput::SaveCursor]),
        route(None, b"", b'u', b"", vec![TerminalOutput::RestoreCursor]),
        route(
            None,
            b"",
            b't',
            b"8;24;80",
            vec![TerminalOutput::WindowManipulation(wm)],
        ),
        route(
            None,
            b"",
            b'x',
            b"0",
            vec![TerminalOutput::RequestTerminalParameters(0)],
        ),
    ]
}

/// Keys with an intermediate (3) and keys with a private-marker prefix (12).
fn prefixed_and_intermediate_routes() -> Vec<Route> {
    vec![
        route(
            None,
            b"$",
            b'p',
            b"4",
            vec![TerminalOutput::Mode(Mode::Irm(Irm::Query))],
        ),
        route(None, b"!", b'p', b"", vec![TerminalOutput::SoftReset]),
        route(
            None,
            b" ",
            b'q',
            b"5",
            vec![TerminalOutput::CursorVisualStyle(
                CursorVisualStyle::VerticalLineCursorBlink,
            )],
        ),
        // `?` prefix.
        route(
            Some(b'?'),
            b"",
            b'h',
            b"25",
            vec![TerminalOutput::Mode(Mode::Dectem(Dectcem::Show))],
        ),
        route(
            Some(b'?'),
            b"",
            b'l',
            b"25",
            vec![TerminalOutput::Mode(Mode::Dectem(Dectcem::Hide))],
        ),
        route(
            Some(b'?'),
            b"",
            b'n',
            b"996",
            vec![TerminalOutput::ColorThemeReport],
        ),
        route(
            Some(b'?'),
            b"",
            b'u',
            b"",
            vec![TerminalOutput::KittyKeyboardQuery],
        ),
        route(
            Some(b'?'),
            b"$",
            b'p',
            b"25",
            vec![TerminalOutput::Mode(Mode::Dectem(Dectcem::Query))],
        ),
        // `>` prefix.
        route(
            Some(b'>'),
            b"",
            b'c',
            b"",
            vec![TerminalOutput::RequestSecondaryDeviceAttributes { param: 0 }],
        ),
        route(
            Some(b'>'),
            b"",
            b'q',
            b"",
            vec![TerminalOutput::RequestDeviceNameAndVersion],
        ),
        route(
            Some(b'>'),
            b"",
            b'm',
            b"4;1",
            vec![TerminalOutput::ModifyOtherKeys(1)],
        ),
        route(
            Some(b'>'),
            b"",
            b'u',
            b"5",
            vec![TerminalOutput::KittyKeyboardPush(5)],
        ),
        // `<` prefix.
        route(
            Some(b'<'),
            b"",
            b'u',
            b"2",
            vec![TerminalOutput::KittyKeyboardPop(2)],
        ),
        // `=` prefix.
        route(
            Some(b'='),
            b"",
            b'c',
            b"",
            vec![TerminalOutput::RequestTertiaryDeviceAttributes],
        ),
        route(
            Some(b'='),
            b"",
            b'u',
            b"5;2",
            vec![TerminalOutput::KittyKeyboardSet { flags: 5, mode: 2 }],
        ),
    ]
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

/// Build a CSI body: optional prefix, parameters, intermediates, final byte.
fn body_of(prefix: Option<u8>, params: &[u8], intermediates: &[u8], final_byte: u8) -> Vec<u8> {
    let mut body: Vec<u8> = Vec::new();
    body.extend(prefix);
    body.extend_from_slice(params);
    body.extend_from_slice(intermediates);
    body.push(final_byte);
    body
}

#[test]
fn matrix_prefix_x_intermediate_x_final() {
    let routes = routes();
    let mut routed_seen = 0usize;
    let mut unrouted_count = 0usize;

    for prefix in [None, Some(b'?'), Some(b'>'), Some(b'<'), Some(b'=')] {
        for intermediates in intermediate_shapes() {
            for final_byte in 0x40u8..=0x7E {
                let found = routes.iter().find(|r| {
                    r.prefix == prefix
                        && r.intermediates == intermediates.as_slice()
                        && r.final_byte == final_byte
                });

                if let Some(r) = found {
                    routed_seen += 1;
                    let body = body_of(prefix, r.params, &intermediates, final_byte);
                    let (output, outcome) = parse(&body);
                    let label = format!(
                        "prefix={} intermediates={intermediates:?} final={:?} body={body:?}",
                        prefix_label(prefix),
                        char::from(final_byte),
                    );
                    assert_eq!(output, r.expected, "routed key output wrong: {label}");
                    assert_eq!(
                        outcome,
                        ParserOutcome::Finished,
                        "routed key must finish cleanly: {label}"
                    );
                } else {
                    for params in [&b"1"[..], b""] {
                        unrouted_count += 1;
                        let body = body_of(prefix, params, &intermediates, final_byte);
                        let (output, outcome) = parse(&body);
                        let label = format!(
                            "prefix={} intermediates={intermediates:?} final={:?} body={body:?}",
                            prefix_label(prefix),
                            char::from(final_byte),
                        );
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
    }

    // 34 plain + 3 intermediate + 5 `?` + 4 `>` + 1 `<` + 2 `=` = 49 keys.
    assert_eq!(routes.len(), 49, "routed table size drifted");
    assert_eq!(
        routed_seen, 49,
        "a routed key was not reached by the matrix"
    );
    assert!(unrouted_count > 10_000, "matrix did not cover enough keys");
}

#[test]
fn routed_table_has_no_duplicate_keys() {
    let routes = routes();
    for (i, a) in routes.iter().enumerate() {
        for b in &routes[i + 1..] {
            assert!(
                !(a.prefix == b.prefix
                    && a.intermediates == b.intermediates
                    && a.final_byte == b.final_byte),
                "duplicate route: prefix={} intermediates={:?} final={:?}",
                prefix_label(a.prefix),
                a.intermediates,
                char::from(a.final_byte),
            );
        }
    }
}

#[test]
fn csi_s_with_parameters_is_decslrm() {
    // The one routed key with two handlers: `CSI s` is SCOSC, `CSI Pl ; Pr s`
    // is DECSLRM.
    let (output, outcome) = parse(b"3;7s");
    assert_eq!(
        output,
        [TerminalOutput::SetLeftAndRightMargins {
            left_margin: 3,
            right_margin: 7,
        }]
    );
    assert_eq!(outcome, ParserOutcome::Finished);
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
        !output.iter().any(|o| matches!(
            o,
            TerminalOutput::CursorVisualStyle(_) | TerminalOutput::CursorVisualStyleDefault
        )),
        "DECSCA must not set a cursor style: {output:?}"
    );
    assert_eq!(output, []);
    assert_eq!(outcome, ParserOutcome::Finished);
}

#[test]
fn csi_decll_q_is_not_cursor_style() {
    let (output, outcome) = parse(b"1q");
    assert!(
        !output.iter().any(|o| matches!(
            o,
            TerminalOutput::CursorVisualStyle(_) | TerminalOutput::CursorVisualStyleDefault
        )),
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
