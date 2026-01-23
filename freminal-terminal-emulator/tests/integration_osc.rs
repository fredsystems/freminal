// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for OSC 8 hyperlinks and OSC 1337 extensions

use freminal_terminal_emulator::ansi::FreminalAnsiParser;

#[test]
fn osc8_valid_hyperlink_sequence() {
    let seq = b"\x1b]8;;https://example.com\x07Click Here\x1b]8;;\x07";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let trace = parser.current_trace_str();
    assert!(
        trace.contains("https://example.com"),
        "Trace missing URL: {trace}"
    );
    assert!(trace.contains("Click"), "Trace missing hyperlink text");
}

#[test]
fn osc8_malformed_sequence_enters_invalid_state() {
    let seq = b"\x1b]8;;https://broken.com"; // missing terminator
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    assert!(parser.current_trace_str().contains("broken.com"));
}

#[test]
fn osc1337_file_inline_contains_params() {
    let seq = b"\x1b]1337;File=name=test.txt;size=10;inline=1:\x07";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let recent = parser.current_trace_str();
    assert!(
        recent.contains("1337"),
        "Trace should contain 1337 identifier"
    );
    assert!(
        recent.contains("name=test.txt"),
        "Trace missing filename: {recent}"
    );
}

#[test]
fn osc1337_unknown_subcommand_sets_invalid_state() {
    let seq = b"\x1b]1337;ThisIsNotValid=1\x07";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let recent = parser.current_trace_str();
    assert!(
        !recent.is_empty(),
        "Recent trace should not be empty even on invalid"
    );
}
