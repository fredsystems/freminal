// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Generic mode queries (DECRQM)

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn decrqm_private_and_regular() {
    let mut p = AnsiCsiParser::default();
    for seq in ["\x1b[?25$p", "\x1b[1$p"] {
        for &b in seq.as_bytes() {
            let _ = p.push(b);
        }
        let t = p.current_trace_str();
        assert!(t.contains("$p"));
        p.clear_trace();
    }
}
