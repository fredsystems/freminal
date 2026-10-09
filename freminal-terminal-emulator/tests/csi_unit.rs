// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;

#[test]
fn csi_cursor_move_home() {
    let mut p = AnsiCsiParser::default();
    // The real parser never forwards the `ESC [` introducer to the sub-parser.
    for &b in b"1;1H" {
        let _ = p.push(b);
    }
    assert_eq!(p.trace_str(), "1;1H");
}

#[test]
fn csi_select_graphic_rendition_truecolor() {
    let mut p = AnsiCsiParser::default();
    for &b in b"38;2;1;2;3m" {
        let _ = p.push(b);
    }
    assert_eq!(p.trace_str(), "38;2;1;2;3m");
}

#[test]
fn csi_invalid_sequence_sets_invalid_but_keeps_trace() {
    let mut p = AnsiCsiParser::default();
    for &b in b"99;99;\x01X" {
        let _ = p.push(b);
    } // invalid intermediate/param byte
    assert!(!p.trace_str().is_empty());
}
