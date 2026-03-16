// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level unit tests for six single-parameter CSI commands:
//! CPL, ICH (ict), SD, DL, IL, and SU.
//!
//! Each parser is exercised with: empty params (default), explicit zero,
//! an explicit value > 1, and an invalid non-numeric param.

#![allow(clippy::unwrap_used)]

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::ParserOutcome;
use freminal_terminal_emulator::ansi_components::csi_commands::cpl::ansi_parser_inner_csi_finished_cpl;
use freminal_terminal_emulator::ansi_components::csi_commands::dl::ansi_parser_inner_csi_finished_dl;
use freminal_terminal_emulator::ansi_components::csi_commands::ict::ansi_parser_inner_csi_finished_ich;
use freminal_terminal_emulator::ansi_components::csi_commands::il::ansi_parser_inner_csi_finished_set_position_l;
use freminal_terminal_emulator::ansi_components::csi_commands::sd::ansi_parser_inner_csi_finished_sd;
use freminal_terminal_emulator::ansi_components::csi_commands::su::ansi_parser_inner_csi_finished_su;

// ---------------------------------------------------------------------------
// CPL — Cursor Previous Line
// ---------------------------------------------------------------------------

#[test]
fn cpl_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_cpl(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![
            TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1),
            },
            TerminalOutput::SetCursorPos {
                x: Some(1),
                y: None,
            },
        ],
        "empty params should emit param=1"
    );
}

#[test]
fn cpl_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_cpl(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![
            TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-1),
            },
            TerminalOutput::SetCursorPos {
                x: Some(1),
                y: None,
            },
        ],
        "param=0 should normalize to 1"
    );
}

#[test]
fn cpl_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_cpl(b"3", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![
            TerminalOutput::SetCursorPosRel {
                x: None,
                y: Some(-3),
            },
            TerminalOutput::SetCursorPos {
                x: Some(1),
                y: None,
            },
        ],
        "param=3 should emit y=Some(-3)"
    );
}

#[test]
fn cpl_invalid_non_numeric_returns_failure_with_empty_output() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_cpl(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert!(
        output.is_empty(),
        "CPL must not push Invalid on parse failure; got {output:?}"
    );
}

// ---------------------------------------------------------------------------
// ICH — Insert Blank Character(s)
// ---------------------------------------------------------------------------

#[test]
fn ich_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_ich(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertSpaces(1)],
        "empty params should emit InsertSpaces(1)"
    );
}

#[test]
fn ich_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_ich(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertSpaces(1)],
        "param=0 should normalize to InsertSpaces(1)"
    );
}

#[test]
fn ich_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_ich(b"5", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertSpaces(5)],
        "param=5 should emit InsertSpaces(5)"
    );
}

#[test]
fn ich_invalid_non_numeric_pushes_invalid_and_returns_failure() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_ich(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::Invalid],
        "ICH must push Invalid on parse failure; got {output:?}"
    );
}

// ---------------------------------------------------------------------------
// SD — Scroll Down
// ---------------------------------------------------------------------------

#[test]
fn sd_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_sd(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollDown(1)],
        "empty params should emit ScrollDown(1)"
    );
}

#[test]
fn sd_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_sd(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollDown(1)],
        "param=0 should normalize to ScrollDown(1)"
    );
}

#[test]
fn sd_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_sd(b"5", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollDown(5)],
        "param=5 should emit ScrollDown(5)"
    );
}

#[test]
fn sd_invalid_non_numeric_pushes_invalid_and_returns_failure() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_sd(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::Invalid],
        "SD must push Invalid on parse failure; got {output:?}"
    );
}

// ---------------------------------------------------------------------------
// DL — Delete Lines
// ---------------------------------------------------------------------------

#[test]
fn dl_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_dl(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::DeleteLines(1)],
        "empty params should emit DeleteLines(1)"
    );
}

#[test]
fn dl_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_dl(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::DeleteLines(1)],
        "param=0 should normalize to DeleteLines(1)"
    );
}

#[test]
fn dl_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_dl(b"5", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::DeleteLines(5)],
        "param=5 should emit DeleteLines(5)"
    );
}

#[test]
fn dl_invalid_non_numeric_pushes_invalid_and_returns_failure() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_dl(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::Invalid],
        "DL must push Invalid on parse failure; got {output:?}"
    );
}

// ---------------------------------------------------------------------------
// IL — Insert Lines
// ---------------------------------------------------------------------------

#[test]
fn il_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_set_position_l(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertLines(1)],
        "empty params should emit InsertLines(1)"
    );
}

#[test]
fn il_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_set_position_l(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertLines(1)],
        "param=0 should normalize to InsertLines(1)"
    );
}

#[test]
fn il_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_set_position_l(b"5", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::InsertLines(5)],
        "param=5 should emit InsertLines(5)"
    );
}

#[test]
fn il_invalid_non_numeric_pushes_invalid_and_returns_failure() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_set_position_l(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::Invalid],
        "IL must push Invalid on parse failure; got {output:?}"
    );
}

// ---------------------------------------------------------------------------
// SU — Scroll Up
// ---------------------------------------------------------------------------

#[test]
fn su_empty_params_defaults_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_su(b"", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollUp(1)],
        "empty params should emit ScrollUp(1)"
    );
}

#[test]
fn su_explicit_zero_normalizes_to_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_su(b"0", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollUp(1)],
        "param=0 should normalize to ScrollUp(1)"
    );
}

#[test]
fn su_explicit_value_greater_than_one() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_su(b"5", &mut output);
    assert!(
        matches!(result, ParserOutcome::Finished),
        "expected Finished, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::ScrollUp(5)],
        "param=5 should emit ScrollUp(5)"
    );
}

#[test]
fn su_invalid_non_numeric_pushes_invalid_and_returns_failure() {
    let mut output = Vec::new();
    let result = ansi_parser_inner_csi_finished_su(b"abc", &mut output);
    assert!(
        matches!(result, ParserOutcome::InvalidParserFailure(_)),
        "expected InvalidParserFailure, got {result:?}"
    );
    assert_eq!(
        output,
        vec![TerminalOutput::Invalid],
        "SU must push Invalid on parse failure; got {output:?}"
    );
}
