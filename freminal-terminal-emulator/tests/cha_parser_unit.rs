// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::ParserOutcome;
use freminal_terminal_emulator::ansi_components::csi_commands::cha::ansi_parser_inner_csi_finished_set_cursor_position_g;
use freminal_terminal_emulator::error::ParserFailures::UnhandledCHACommand;

#[test]
fn valid_param_normal_number() {
    let mut output = Vec::new();
    let res = ansi_parser_inner_csi_finished_set_cursor_position_g(b"42", &mut output);
    assert_eq!(res, ParserOutcome::Finished);
    assert_eq!(
        output,
        vec![TerminalOutput::SetCursorPos {
            x: Some(42),
            y: None
        }]
    );
}

#[test]
fn valid_param_zero_treated_as_one() {
    let mut output = Vec::new();
    let res = ansi_parser_inner_csi_finished_set_cursor_position_g(b"0", &mut output);
    assert_eq!(res, ParserOutcome::Finished);
    assert_eq!(
        output,
        vec![TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None
        }]
    );
}

#[test]
fn valid_param_one_treated_as_one() {
    let mut output = Vec::new();
    let res = ansi_parser_inner_csi_finished_set_cursor_position_g(b"1", &mut output);
    assert_eq!(res, ParserOutcome::Finished);
    assert_eq!(
        output,
        vec![TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None
        }]
    );
}

#[test]
fn empty_param_defaults_to_one() {
    let mut output = Vec::new();
    let res = ansi_parser_inner_csi_finished_set_cursor_position_g(b"", &mut output);
    assert_eq!(res, ParserOutcome::Finished);
    assert_eq!(
        output,
        vec![TerminalOutput::SetCursorPos {
            x: Some(1),
            y: None
        }]
    );
}

#[test]
fn invalid_ascii_param_results_in_error_and_invalid_output() {
    let mut output = Vec::new();
    let err = ansi_parser_inner_csi_finished_set_cursor_position_g(b"abc", &mut output);
    assert_eq!(
        err,
        ParserOutcome::InvalidParserFailure(UnhandledCHACommand("abc".to_string()))
    );
    assert_eq!(output, vec![]);

    // ✅ Updated to match real ParserFailures Display
    let msg = err.to_string();
    assert!(
        msg.contains("UnhandledCHACommand"),
        "Unexpected error message: {msg}"
    );
}

#[test]
fn invalid_utf8_param_results_in_error_and_invalid_output() {
    let mut output = Vec::new();
    let err = ansi_parser_inner_csi_finished_set_cursor_position_g(&[0xFF], &mut output);
    assert_eq!(
        err,
        ParserOutcome::InvalidParserFailure(UnhandledCHACommand(
            String::from_utf8_lossy(&[0xFF]).to_string()
        ))
    );
    assert_eq!(output, vec![]);

    // ✅ Same substring check as above
    let msg = err.to_string();
    assert!(
        msg.contains("UnhandledCHACommand"),
        "Unexpected error message: {msg}"
    );
}

#[test]
fn correct_error_type_is_parser_failures() {
    let mut output = Vec::new();
    let err = ansi_parser_inner_csi_finished_set_cursor_position_g(b"x", &mut output);
    assert_eq!(
        err,
        ParserOutcome::InvalidParserFailure(UnhandledCHACommand("x".into()))
    );
}
