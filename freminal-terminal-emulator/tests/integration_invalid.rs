// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for invalid and truncated sequence recovery

use freminal_terminal_emulator::ansi::FreminalAnsiParser;

#[test]
fn truncated_sequence_resets_cleanly() {
    let mut parser = FreminalAnsiParser::default();
    // Incomplete OSC, should not panic and should recover
    let seq = b"\x1b]1337;File=name=test";
    for b in seq {
        parser.push(&[*b]);
    }
    let before = parser.current_trace_str();
    assert!(!before.is_empty(), "Trace should capture partial sequence");

    // Feed a new valid sequence after truncation
    let valid = b"\x1b[0m";
    for b in valid {
        parser.push(&[*b]);
    }
    let after = parser.current_trace_str();
    assert!(
        after.contains("[0m"),
        "Parser should recover and parse new sequence"
    );
}

#[test]
fn invalid_bytes_do_not_panic() {
    let mut parser = FreminalAnsiParser::default();
    let invalid = [0x1b, 0x9b, 0x00, 0xff, 0x1b];
    for b in invalid {
        parser.push(&[b]);
    }
    let trace = parser.current_trace_str();
    assert!(
        trace.len() < 256,
        "Trace length reasonable after invalid input"
    );
}

#[test]
fn invalid_then_valid_sequence_parses_normally() {
    let mut parser = FreminalAnsiParser::default();
    let seq = b"\x1b]9999;Invalid\x07\x1b[31mOK\x1b[0m";
    for b in seq {
        parser.push(&[*b]);
    }
    let recent = parser.current_trace_str();
    assert!(
        recent.contains("OK"),
        "Should recover and capture valid text"
    );
}
