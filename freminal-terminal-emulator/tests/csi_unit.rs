// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn csi_cursor_move_home() {
    let mut p = AnsiCsiParser::default();
    for &b in b"\x1b[1;1H" {
        let _ = p.push(b);
    }
    let s = p.current_trace_str();
    assert!(s.contains("[1;1H"));
}

#[test]
fn csi_select_graphic_rendition_truecolor() {
    let mut p = AnsiCsiParser::default();
    for &b in b"\x1b[38;2;1;2;3m" {
        let _ = p.push(b);
    }
    assert!(p.current_trace_str().contains("38;2;1;2;3"));
}

#[test]
fn csi_invalid_sequence_sets_invalid_but_keeps_trace() {
    let mut p = AnsiCsiParser::default();
    for &b in b"\x1b[99;99;X" {
        let _ = p.push(b);
    } // invalid final
    assert!(!p.current_trace_str().is_empty());
}
