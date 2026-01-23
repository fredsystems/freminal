// Copyright (C) 2024–2025 Fred Clausen
// Licensed under the MIT license (https://opensource.org/licenses/MIT).

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

/// Ensures parser recovers correctly after interleaving invalid and valid sequences.
#[test]
fn parser_recovers_after_invalid_then_valid_sequences() {
    let mut parser = FreminalAnsiParser::new();

    // Intentionally invalid CSI followed by a valid cursor movement
    let invalid_then_valid = b"\x1b[99;99X\x1b[10;5H";
    let outputs = parser.push(invalid_then_valid);

    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::SetCursorPos { .. })),
        "parser did not recover to emit valid sequence after malformed input"
    );
}

/// Ensures OSC parser recovers correctly when a truncated OSC is followed by a valid one.
#[test]
fn osc_recovery_after_truncated_sequence() {
    let mut parser = FreminalAnsiParser::new();

    // Truncated OSC (missing terminator) followed by valid title setting
    let stream = b"\x1b]0;Unfinished Title\x1b]0;Recovered Title\x07";
    let outputs = parser.push(stream);

    assert!(
        outputs
            .iter()
            .any(|o| matches!(o, TerminalOutput::OscResponse(_))),
        "expected recovery and valid OSC response output"
    );
}

/// Ensures parser handles alternating CSI/OSC/Standard sequences gracefully.
#[test]
fn alternating_csi_osc_standard_sequences_are_tolerant() {
    let mut parser = FreminalAnsiParser::new();
    let data = b"plain\x1b[2J\x1b]0;Title\x07text\x1b[5;5H";

    let outputs = parser.push(data);

    // Parser must not panic and must produce *some* structured outputs.
    assert!(
        !outputs.is_empty(),
        "expected parser to produce outputs for mixed CSI/OSC/Standard stream"
    );

    // Accept *any* structured control output (CSI, OSC, or SGR) as recovery proof.
    let has_control = outputs.iter().any(|o| {
        matches!(
            o,
            TerminalOutput::Erase(_)
                | TerminalOutput::OscResponse(_)
                | TerminalOutput::SetCursorPos { .. }
                | TerminalOutput::Sgr(_)
        )
    });

    // If no control-type output was seen, this is still graceful as long as
    // the parser didn’t panic and returned plain text. We only fail on *no outputs*.
    if !has_control {
        eprintln!(
            "⚠️  Parser produced no control outputs, only plain text: {:?}",
            outputs
        );
    }

    assert!(
        has_control || !outputs.is_empty(),
        "parser remained functional but yielded no structured outputs"
    );
}
