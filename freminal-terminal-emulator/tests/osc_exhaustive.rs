// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: OSC coverage (0/1/2 titles, 4/10/11 colors, 8 hyperlinks, 1337 extras)

use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

fn feed(p: &mut AnsiOscParser, s: &str) {
    for &b in s.as_bytes() {
        let _ = p.push(b);
    }
}

#[test]
fn osc_titles_with_both_terminators() {
    let mut p = AnsiOscParser::default();
    for seq in [
        "\x1b]0;Title BEL",
        "\x1b]1;Icon Title BEL",
        "\x1b]2;Window Title BEL",
    ] {
        let seq = seq.replace(" BEL", "\x07");
        feed(&mut p, &seq);
        assert!(p.current_trace_str().contains("Title"));
        p.clear_trace();
    }
    for seq in ["\x1b]0;Title\x1b\\", "\x1b]2;X\x1b\\"] {
        feed(&mut p, seq);
        assert!(p.current_trace_str().contains("]0;") || p.current_trace_str().contains("]2;"));
        p.clear_trace();
    }
}

#[test]
fn osc8_hyperlink_valid_and_malformed() {
    let mut p = AnsiOscParser::default();
    // valid
    feed(&mut p, "\x1b]8;;https://example.com\x07Click\x1b]8;;\x07");
    assert!(p.current_trace_str().contains("https://example.com"));
    p.clear_trace();
    // malformed (missing end)
    feed(&mut p, "\x1b]8;;https://broken.example");
    assert!(!p.current_trace_str().is_empty());
}

#[test]
fn osc_palette_and_iterm_extensions() {
    let mut p = AnsiOscParser::default();
    for seq in [
        "\x1b]4;10;#112233\x07",
        "\x1b]10;#445566\x07",
        "\x1b]11;#778899\x07",
        "\x1b]1337;File=name=a.png;size=1;inline=1:\x07",
    ] {
        feed(&mut p, seq);
        assert!(!p.current_trace_str().is_empty());
        p.clear_trace();
    }
}
