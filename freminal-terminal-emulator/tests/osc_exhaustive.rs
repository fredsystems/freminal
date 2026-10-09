// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: OSC coverage (0/1/2 titles, 4/10/11 colors, 8 hyperlinks, 1337 extras)

use freminal_terminal_emulator::ansi_components::osc::AnsiOscParser;

/// Feed the OSC body (everything after the `ESC ]` introducer, which the real
/// parser never forwards to the OSC sub-parser) into a fresh parser.
fn feed(s: &str) -> AnsiOscParser {
    let mut p = AnsiOscParser::default();
    for &b in s.strip_prefix("\x1b]").unwrap_or(s).as_bytes() {
        let _ = p.push(b);
    }
    p
}

#[test]
fn osc_titles_with_both_terminators() {
    for seq in [
        "\x1b]0;Title BEL",
        "\x1b]1;Icon Title BEL",
        "\x1b]2;Window Title BEL",
    ] {
        let seq = seq.replace(" BEL", "\x07");
        let p = feed(&seq);
        assert!(p.trace_str().contains("Title"));
    }
    for seq in ["\x1b]0;Title\x1b\\", "\x1b]2;X\x1b\\"] {
        let p = feed(seq);
        assert!(p.trace_str() == "0;Title" || p.trace_str() == "2;X");
    }
}

#[test]
fn osc8_hyperlink_valid_and_malformed() {
    // valid
    let p = feed("\x1b]8;;https://example.com\x07Click\x1b]8;;\x07");
    assert!(p.trace_str().contains("https://example.com"));
    // malformed (missing end)
    let p = feed("\x1b]8;;https://broken.example");
    assert!(!p.trace_str().is_empty());
}

#[test]
fn osc_palette_and_iterm_extensions() {
    for seq in [
        "\x1b]4;10;#112233\x07",
        "\x1b]10;#445566\x07",
        "\x1b]11;#778899\x07",
        "\x1b]1337;File=name=a.png;size=1;inline=1:\x07",
    ] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}
