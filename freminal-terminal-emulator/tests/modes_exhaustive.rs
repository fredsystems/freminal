// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: DEC private modes and queries (broad set)

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

fn feed(p: &mut AnsiCsiParser, s: &str) {
    for &b in s.as_bytes() {
        let _ = p.push(b);
    }
}

#[test]
fn dec_private_modes_toggle_common() {
    let mut p = AnsiCsiParser::default();
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
        feed(&mut p, seq);
        assert!(p.current_trace_str().contains('?'));
        p.clear_trace();
    }
}

#[test]
fn dec_mode_reports_regular_and_private() {
    let mut p = AnsiCsiParser::default();
    for seq in ["\x1b[1$p", "\x1b[2$p", "\x1b[?25$p", "\x1b[?1049$p"] {
        feed(&mut p, seq);
        assert!(p.current_trace_str().contains("$p"));
        p.clear_trace();
    }
}

#[test]
fn device_attributes_and_xtversion() {
    let mut p = AnsiCsiParser::default();
    for seq in ["\x1b[c", "\x1b[>c", "\x1b[>0q"] {
        feed(&mut p, seq);
        assert!(!p.current_trace_str().is_empty());
        p.clear_trace();
    }
}
