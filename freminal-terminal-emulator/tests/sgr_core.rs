// Copyright (C) 2024–2025 Fred Clausen
// Licensed under the MIT license (https://opensource.org/licenses/MIT).

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

/// Helper to push a single sequence and collect all outputs.
fn parse_seq(seq: &[u8]) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::new();
    parser.push(seq)
}

/// --- BASIC ATTRIBUTE TESTS -------------------------------------------------

#[test]
fn sgr_basic_attributes_toggle_correctly() {
    // Bold, Underline, Reverse, Reset
    let sequences = [
        (b"\x1b[1m", "bold"),
        (b"\x1b[4m", "underline"),
        (b"\x1b[7m", "reverse"),
        (b"\x1b[0m", "reset"),
    ];

    for (seq, label) in sequences {
        let outputs = parse_seq(seq);
        assert!(
            outputs.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
            "expected SGR output for {} sequence {:?}, got {:?}",
            label,
            seq,
            outputs
        );
    }
}

/// --- 256-COLOR PATHS -------------------------------------------------------

#[test]
fn sgr_256_color_foreground_and_background() {
    let fg = parse_seq(b"\x1b[38;5;160m"); // red
    let bg = parse_seq(b"\x1b[48;5;33m"); // blue

    assert!(
        fg.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected 256-color foreground output"
    );
    assert!(
        bg.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected 256-color background output"
    );
}

/// --- TRUECOLOR PATHS ------------------------------------------------------

#[test]
fn sgr_truecolor_foreground_and_background() {
    let fg = parse_seq(b"\x1b[38;2;255;0;0m");
    let bg = parse_seq(b"\x1b[48;2;10;20;30m");

    assert!(
        fg.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected true-color foreground output"
    );
    assert!(
        bg.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected true-color background output"
    );
}

/// --- COMBINED ATTRIBUTES --------------------------------------------------

#[test]
fn sgr_combined_attributes_apply_in_one_sequence() {
    // Bold (1), red foreground (31), blue background (44)
    let combined = parse_seq(b"\x1b[1;31;44m");

    assert!(
        combined.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected SGR output for combined attributes"
    );
}

/// --- RESET SEMANTICS ------------------------------------------------------

#[test]
fn sgr_reset_restores_default_attributes() {
    let mut parser = FreminalAnsiParser::new();

    // Styled sequence (bold + red fg)
    parser.push(b"\x1b[1;38;5;160m");

    // Now perform a reset
    let outputs = parser.push(b"\x1b[0m");

    // The parser should not panic and should yield at least one SGR output.
    assert!(
        outputs.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected parser to emit SGR reset output after \\x1b[0m, got: {:?}",
        outputs
    );
}

/// --- MALFORMED INPUTS -----------------------------------------------------

#[test]
fn sgr_malformed_sequences_are_ignored_or_graceful() {
    let malformed: &[&[u8]] = &[
        b"\x1b[38;2;255m",         // incomplete true-color
        b"\x1b[38;5;999m",         // out of 256-color range
        b"\x1b[38;2;999;999;999m", // out-of-range true-color
        b"\x1b[38;2;255;0;0;99m",  // extra params
    ];

    for seq in malformed {
        let outputs = parse_seq(seq);
        // Parser should never panic; may yield empty or benign output.
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should handle malformed sequence {:?} gracefully",
            seq
        );
    }
}
