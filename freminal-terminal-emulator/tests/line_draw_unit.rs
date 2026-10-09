// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Line drawing transitions via ESC (0 and B) if supported

use freminal_terminal_emulator::ansi_components::standard::StandardParser;

#[test]
fn line_draw_enable_disable() {
    // The real parser never forwards the leading ESC to the sub-parser.
    let mut p = StandardParser::default();
    for &b in b"(0" {
        let _ = p.push(b);
    } // enable line draw
    assert_eq!(p.trace_str(), "(0");
    let mut p = StandardParser::default();
    for &b in b"(B" {
        let _ = p.push(b);
    } // disable line draw
    assert_eq!(p.trace_str(), "(B");
}
