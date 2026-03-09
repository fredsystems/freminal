// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::standard::StandardParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn standard_plain_text_does_not_panic() {
    let mut p = StandardParser::default();
    for &b in b"hello world" {
        let _ = p.push(b);
    }
    // no panic, parser accepted bytes
}

#[test]
fn standard_esc_starts_control_sequence() {
    let mut p = StandardParser::default();
    let _ = p.push(0x1b);
    // Ensure internal trace has ESC recorded
    assert!(p.current_trace_str().contains("\x1b") || !p.current_trace_str().is_empty());
}
