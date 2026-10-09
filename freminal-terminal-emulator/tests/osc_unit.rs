// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;

#[test]
fn osc_valid_title_and_terminate_with_bel() {
    let mut p = AnsiOscParser::default();
    let seq = b"0;My Title\x07";
    for &b in seq {
        let _ = p.push(b);
    }
    let recent = p.trace_str();
    // The terminator is stripped once the sequence finishes.
    assert_eq!(recent, "0;My Title");
}

#[test]
fn osc_valid_title_and_terminate_with_st() {
    let mut p = AnsiOscParser::default();
    let seq = b"0;Title\x1b\\";
    for &b in seq {
        let _ = p.push(b);
    }
    let s = p.trace_str();
    assert_eq!(s, "0;Title");
}

#[test]
fn osc_invalid_param_enters_invalid_state_but_keeps_trace() {
    let mut p = AnsiOscParser::default();
    let seq = b"1337;Unknown=1\x07";
    for &b in seq {
        let _ = p.push(b);
    }
    let recent = p.trace_str();
    assert!(!recent.is_empty(), "Trace should not be empty on invalid");
}
