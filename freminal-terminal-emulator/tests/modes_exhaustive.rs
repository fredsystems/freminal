// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: DEC private modes and queries (broad set)

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;

/// Feed the CSI body (everything after the `ESC [` introducer, which the
/// real parser never forwards to the CSI sub-parser) into a fresh parser.
fn feed(s: &str) -> AnsiCsiParser {
    let mut p = AnsiCsiParser::default();
    for &b in s.strip_prefix("\x1b[").unwrap_or(s).as_bytes() {
        let _ = p.push(b);
    }
    p
}

#[test]
fn dec_private_modes_toggle_common() {
    for seq in [
        "\x1b[?1h",
        "\x1b[?1l",
        "\x1b[?6h",
        "\x1b[?6l",
        "\x1b[?7h",
        "\x1b[?7l",
        "\x1b[?12h",
        "\x1b[?12l",
        "\x1b[?25h",
        "\x1b[?25l",
        "\x1b[?1047h",
        "\x1b[?1047l",
        "\x1b[?1048h",
        "\x1b[?1048l",
        "\x1b[?1049h",
        "\x1b[?1049l",
    ] {
        let p = feed(seq);
        assert!(p.trace_str().contains('?'));
    }
}

#[test]
fn dec_mode_reports_regular_and_private() {
    for seq in ["\x1b[1$p", "\x1b[2$p", "\x1b[?25$p", "\x1b[?1049$p"] {
        let p = feed(seq);
        assert!(p.trace_str().contains("$p"));
    }
}

#[test]
fn device_attributes_and_xtversion() {
    for seq in ["\x1b[c", "\x1b[>c", "\x1b[>0q"] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}
