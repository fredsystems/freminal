// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;

#[test]
fn osc_trace_str_reflects_pushed_bytes() {
    let mut p = AnsiOscParser::default();
    assert!(p.trace_str().is_empty());
    let _ = p.push(b'A');
    let _ = p.push(b'B');
    assert_eq!(p.trace_str(), "AB");
}
