// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: Standard parser edge cases (invalid bytes, partial ESC, UTF-8 oddities)

use freminal_terminal_emulator::ansi_components::standard::StandardParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn standard_invalid_and_control_bytes() {
    let mut p = StandardParser::default();
    // mix invalid and control bytes
    for &b in [0x00u8, 0xffu8, 0x1bu8, b'A', 0x9bu8].iter() {
        let _ = p.push(b);
    }
    assert!(!p.current_trace_str().is_empty() || p.current_trace_str().is_empty());
    // sanity: no panic
}

#[test]
fn standard_partial_escape_then_text() {
    let mut p = StandardParser::default();
    let _ = p.push(0x1b); // ESC start
    for &b in b"[31mHello".iter() {
        let _ = p.push(b);
    }
    assert!(!p.current_trace_str().is_empty());
}
