// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for mode structures and reporting

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;

#[test]
fn dec_private_mode_enable_disable() {
    // DEC Private Mode Set/Reset examples
    for seq in ["\x1b[?25h", "\x1b[?25l", "\x1b[?1049h", "\x1b[?1049l"] {
        // The real parser never forwards the `ESC [` introducer.
        let mut p = AnsiCsiParser::default();
        for &b in seq.strip_prefix("\x1b[").unwrap_or(seq).as_bytes() {
            let _ = p.push(b);
        }
        assert!(p.trace_str().contains("?"));
    }
}

#[test]
fn device_attributes_primary_and_secondary() {
    for seq in ["\x1b[c", "\x1b[>c"] {
        // The real parser never forwards the `ESC [` introducer.
        let mut p = AnsiCsiParser::default();
        for &b in seq.strip_prefix("\x1b[").unwrap_or(seq).as_bytes() {
            let _ = p.push(b);
        }
        assert!(p.trace_str().contains('c'));
    }
}
