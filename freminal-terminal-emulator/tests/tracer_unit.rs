// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn tracer_appends_and_clears() {
    let mut p = AnsiOscParser::default();
    p.append_trace(b'A');
    p.append_trace(b'B');
    assert_eq!(p.current_trace_str(), "AB");
    p.clear_trace();
    assert!(p.current_trace_str().is_empty());
}
