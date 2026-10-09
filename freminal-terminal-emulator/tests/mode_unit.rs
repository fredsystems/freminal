// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Generic mode queries (DECRQM)

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;

#[test]
fn decrqm_private_and_regular() {
    for seq in ["\x1b[?25$p", "\x1b[1$p"] {
        // The real parser never forwards the `ESC [` introducer.
        let mut p = AnsiCsiParser::default();
        for &b in seq.strip_prefix("\x1b[").unwrap_or(seq).as_bytes() {
            let _ = p.push(b);
        }
        let t = p.trace_str();
        assert!(t.contains("$p"));
    }
}
