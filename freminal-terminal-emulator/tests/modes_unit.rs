// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for mode structures and reporting

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn dec_private_mode_enable_disable() {
    let mut p = AnsiCsiParser::default();
    // DEC Private Mode Set/Reset examples
    for seq in ["\x1b[?25h", "\x1b[?25l", "\x1b[?1049h", "\x1b[?1049l"] {
        for &b in seq.as_bytes() {
            let _ = p.push(b);
        }
        assert!(p.current_trace_str().contains("?"));
        p.clear_trace();
    }
}

#[test]
fn device_attributes_primary_and_secondary() {
    let mut p = AnsiCsiParser::default();
    for seq in ["\x1b[c", "\x1b[>c"] {
        for &b in seq.as_bytes() {
            let _ = p.push(b);
        }
        assert!(p.current_trace_str().contains('c'));
        p.clear_trace();
    }
}
