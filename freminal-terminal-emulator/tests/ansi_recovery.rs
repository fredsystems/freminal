// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: Root parser recovery (switching between parsers and error handling)

use freminal_terminal_emulator::ansi::FreminalAnsiParser;

#[test]
fn switches_between_standard_csi_osc_and_recovers() {
    let mut p = FreminalAnsiParser::default();
    let seq = b"\x1b[31mRed\x1b[0m plain \x1b]2;Title\x07 more \x1b[H";
    let mut all = Vec::new();
    all.extend_from_slice(seq);
    // add some invalid tail to trigger error + reset
    all.extend_from_slice(b"\x1b]9999;Oops\x07");
    for chunk in all.chunks(3) {
        let _outs = p.push(chunk);
    }
    let trace = p.current_trace_str();
    assert!(!trace.is_empty());
}
