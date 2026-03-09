// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests enabling and disabling DEC private and regular modes.
//! Focuses on parser-level correctness of emitted `TerminalOutput` variants.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn test_enable_disable_known_modes() {
    println!("Testing DECCOLM enable/disable");
    let enable_outputs = push_seq("\x1b[?3h");
    for o in &enable_outputs {
        println!("enable variant: {:?}", o);
    }
    assert!(
        enable_outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. })),
        "Expected Mode variant when enabling DECCOLM"
    );

    let disable_outputs = push_seq("\x1b[?3l");
    for o in &disable_outputs {
        println!("disable variant: {:?}", o);
    }
    assert!(
        disable_outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. })),
        "Expected Mode variant when disabling DECCOLM"
    );
}

#[test]
fn test_regular_modes() {
    println!("Testing LNM (line feed/new line mode)");
    let enable_outputs = push_seq("\x1b[20h");
    dbg!(&enable_outputs);
    assert!(
        enable_outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. })),
        "Expected Mode variant for LNM enable"
    );

    let disable_outputs = push_seq("\x1b[20l");
    dbg!(&disable_outputs);
    assert!(
        disable_outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. })),
        "Expected Mode variant for LNM disable"
    );
}

#[test]
fn test_unknown_or_invalid_modes() {
    println!("Testing invalid/unknown private mode");
    let outputs = push_seq("\x1b[?9999h");
    dbg!(&outputs);
    assert!(
        !outputs.is_empty(),
        "Expected graceful handling of unknown private mode"
    );

    let outputs_invalid = push_seq("\x1b[?x3l");
    println!("invalid mode output: {:?}", outputs_invalid);
    assert!(
        !outputs_invalid.is_empty(),
        "Expected non-empty output on malformed mode sequence"
    );
}
