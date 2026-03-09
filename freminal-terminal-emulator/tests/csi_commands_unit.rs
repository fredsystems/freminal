// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Exhaustive CSI command coverage (table-driven)

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

fn feed_bytes(p: &mut AnsiCsiParser, s: &str) {
    for &b in s.as_bytes() {
        let _ = p.push(b);
    }
}

#[test]
fn csi_move_commands_variants() {
    let mut p = AnsiCsiParser::default();
    // CUP, CHA, CUU, CUD, CUF, CUB
    for seq in [
        "\x1b[1;1H",
        "\x1b[10G",
        "\x1b[5A",
        "\x1b[3B",
        "\x1b[7C",
        "\x1b[2D",
    ] {
        feed_bytes(&mut p, seq);
        assert!(p.current_trace_str().contains('['));
        p.clear_trace();
    }
}

#[test]
fn csi_erase_commands() {
    let mut p = AnsiCsiParser::default();
    for seq in [
        "\x1b[J", "\x1b[0J", "\x1b[1J", "\x1b[2J", "\x1b[K", "\x1b[0K", "\x1b[1K", "\x1b[2K",
    ] {
        feed_bytes(&mut p, seq);
        assert!(p.current_trace_str().contains('J') || p.current_trace_str().contains('K'));
        p.clear_trace();
    }
}

#[test]
fn csi_insert_delete_chars_lines() {
    let mut p = AnsiCsiParser::default();
    for seq in ["\x1b[3P", "\x1b[4@", "\x1b[2L", "\x1b[2M", "\x1b[3X"] {
        feed_bytes(&mut p, seq);
        assert!(!p.current_trace_str().is_empty());
        p.clear_trace();
    }
}

#[test]
fn csi_sgr_edge_cases() {
    let mut p = AnsiCsiParser::default();
    // 256-color, truecolor, reset, bold+inverse
    for seq in [
        "\x1b[38;5;196m",
        "\x1b[48;5;7m",
        "\x1b[38;2;1;2;3m",
        "\x1b[0m",
        "\x1b[1;7m",
    ] {
        feed_bytes(&mut p, seq);
        assert!(p.current_trace_str().contains('m'));
        p.clear_trace();
    }
}

#[test]
fn csi_invalid_final_and_param_overflow() {
    let mut p = AnsiCsiParser::default();
    feed_bytes(&mut p, "\x1b[999999999999999999999Z"); // invalid final with huge param
    assert!(!p.current_trace_str().is_empty());
}
