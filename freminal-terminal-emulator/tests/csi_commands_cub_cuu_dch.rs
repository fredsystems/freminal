// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level tests for CUB, CUU, and DCH CSI commands.
//! These ensure deterministic `TerminalOutput` emission and error path stability.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

/// Pushes an ANSI sequence through the parser and returns the resulting outputs.
fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn test_cub_normal_and_default_param() {
    println!("Testing ESC[D (CUB)");
    let outputs = push_seq("\x1b[3D");
    for o in &outputs {
        println!("variant: {:?}", o);
    }
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPosRel { .. })),
        "Expected SetCursorPosRel variant, got: {:?}",
        outputs
    );

    // Default param
    let outputs_default = push_seq("\x1b[D");
    dbg!(&outputs_default);
    assert!(
        outputs_default
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPosRel { .. })),
        "Expected SetCursorPosRel with default param"
    );
}

#[test]
fn test_cuu_normal_and_default_param() {
    println!("Testing ESC[A (CUU)");
    let outputs = push_seq("\x1b[4A");
    for o in &outputs {
        println!("variant: {:?}", o);
    }
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPosRel { .. })),
        "Expected SetCursorPosRel variant, got: {:?}",
        outputs
    );

    // Default param
    let outputs_default = push_seq("\x1b[A");
    dbg!(&outputs_default);
    assert!(
        outputs_default
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPosRel { .. })),
        "Expected SetCursorPosRel with default param"
    );
}

#[test]
fn test_dch_valid_and_invalid() {
    println!("Testing ESC[P (DCH)");
    let outputs = push_seq("\x1b[2P");
    dbg!(&outputs);
    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::Delete { .. })),
        "Expected Delete variant, got: {:?}",
        outputs
    );

    // Invalid param should not panic
    let outputs_invalid = push_seq("\x1b[xP");
    println!("invalid param output: {:?}", outputs_invalid);
    assert!(
        !outputs_invalid.is_empty(),
        "Parser returned no output for invalid param"
    );
}

#[test]
fn test_overflow_and_empty_cases() {
    println!("Testing overflow and empty params for CUB/CUU/DCH");
    let outputs_overflow = push_seq("\x1b[99999A");
    assert!(
        outputs_overflow
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPosRel { .. })),
        "Expected SetCursorPosRel even with large param"
    );

    let outputs_empty = push_seq("\x1b[");
    assert!(
        outputs_empty.is_empty(),
        "Parser produced output for incomplete sequence. Got: {:?}",
        outputs_empty
    );
}
