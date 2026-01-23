// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Integration tests for CSI cursor movement and SGR attribute combinations

use freminal_terminal_emulator::ansi::FreminalAnsiParser;

#[test]
fn csi_cursor_and_color_sequence() {
    let seq = b"\x1b[1;1H\x1b[38;5;196mRED\x1b[0m";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let trace = parser.current_trace_str();
    assert!(
        trace.contains("38;5;196"),
        "Trace missing 256-color sequence: {trace}"
    );
    assert!(trace.contains("[1;1H"), "Trace missing cursor move");
}

#[test]
fn csi_truecolor_and_reset() {
    let seq = b"\x1b[38;2;255;0;0mRedText\x1b[0m";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let trace = parser.current_trace_str();
    assert!(
        trace.contains("38;2;255;0;0"),
        "Trace missing truecolor spec: {trace}"
    );
    assert!(
        trace.ends_with("m") || trace.contains("\x1b[0m"),
        "Trace did not complete with reset"
    );
}

#[test]
fn csi_mix_sgr_and_cursor_commands() {
    let seq = b"\x1b[1m\x1b[7mBoldInverse\x1b[10C\x1b[0m";
    let mut parser = FreminalAnsiParser::default();
    for b in seq {
        parser.push(&[*b]);
    }
    let recent = parser.current_trace_str();
    assert!(recent.contains("1m"));
    assert!(recent.contains("7m"));
    assert!(recent.contains("10C"));
}
