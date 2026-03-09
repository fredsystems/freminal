// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Line drawing transitions via ESC (0 and B) if supported

use freminal_terminal_emulator::ansi_components::standard::StandardParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn line_draw_enable_disable() {
    let mut p = StandardParser::default();
    for &b in b"\x1b(0" {
        let _ = p.push(b);
    } // enable line draw
    assert!(!p.current_trace_str().is_empty());
    p.clear_trace();
    for &b in b"\x1b(B" {
        let _ = p.push(b);
    } // disable line draw
    assert!(!p.current_trace_str().is_empty());
}
