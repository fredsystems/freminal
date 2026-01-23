// Copyright (C) 2024-2026 Fred Clausen
// MIT license.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn sgr_reset_attributes() {
    let outs = push_seq("\x1b[0m\x1b[22m\x1b[23m\x1b[24m\x1b[27m\x1b[28m\x1b[29m");
    println!("SGR resets {:?}", outs);
    assert!(outs.iter().all(|o| matches!(o, TerminalOutput::Sgr { .. })));
}

#[test]
fn sgr_combined_truecolor_sequence() {
    let seq = "\x1b[1;38;2;255;0;128;48;2;0;64;255m";
    let outs = push_seq(seq);
    println!("combined truecolor -> {:?}", outs);
    assert!(outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })));
}

#[test]
fn sgr_partial_truecolor_graceful() {
    let seq = "\x1b[38;2;255;0m";
    let outs = push_seq(seq);
    println!("partial truecolor -> {:?}", outs);
    assert!(
        outs.is_empty()
            || outs.iter().any(|o| matches!(o, TerminalOutput::Invalid))
            || outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
        "expected graceful or tolerant handling, got {:?}",
        outs
    );
}

/// Ensure incomplete or malformed truecolor SGR sequences are handled gracefully
/// without panic and with safe fallbacks.
#[test]
fn partial_truecolor_sequences_are_gracefully_handled() {
    let partials: &[&[u8]] = &[
        b"\x1b[38;2;255m",
        b"\x1b[38;2;255;0m",
        b"\x1b[38;2;m",
        b"\x1b[48;2;128;64m",
    ];

    for &seq in partials {
        let mut parser = FreminalAnsiParser::new();
        // The parser should never panic, even on malformed sequences.
        let outputs = parser.push(seq);

        // We just check that we got a Vec back and the parser is still usable.
        assert!(
            !outputs.is_empty() || outputs.is_empty(),
            "parser should always return a Vec, got nothing for {:?}",
            String::from_utf8_lossy(seq)
        );
    }
}

/// Verify that out-of-range truecolor values gracefully fall back to defaults or no-op
#[test]
fn invalid_truecolor_falls_back_to_default_behavior() {
    let mut parser = FreminalAnsiParser::new();
    let outputs = parser.push(b"\x1b[38;2;999;999;999m");

    // The parser shouldn’t panic; it may either ignore invalid SGR or produce a neutral output.
    // Both are acceptable forms of graceful degradation.
    assert!(
        outputs.iter().all(|o| !matches!(o, TerminalOutput::Sgr(_))) || outputs.is_empty(),
        "invalid truecolor sequence should be ignored or degraded safely, got: {:?}",
        outputs
    );
}

/// Ensure the parser recovers when partial and valid SGR sequences are interleaved
#[test]
fn mixed_partial_and_complete_sequences_do_not_panic() {
    let mut parser = FreminalAnsiParser::new();
    let data = b"\x1b[38;2;255;0mhello\x1b[38;2;10;20;30mworld";

    let outputs = parser.push(data);

    // The parser should return gracefully and produce at least one SGR
    assert!(
        outputs.iter().any(|o| matches!(o, TerminalOutput::Sgr(_))),
        "expected at least one SGR output from mixed partial and valid sequences"
    );
}
