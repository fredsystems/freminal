// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Exhaustive CSI command coverage (table-driven)

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
fn csi_move_commands_variants() {
    // CUP, CHA, CUU, CUD, CUF, CUB
    for seq in [
        "\x1b[1;1H",
        "\x1b[10G",
        "\x1b[5A",
        "\x1b[3B",
        "\x1b[7C",
        "\x1b[2D",
    ] {
        let p = feed(seq);
        // The trace is exactly the CSI body (the introducer is never forwarded).
        assert_eq!(p.trace_str(), seq.strip_prefix("\x1b[").unwrap_or(seq));
    }
}

#[test]
fn csi_erase_commands() {
    for seq in [
        "\x1b[J", "\x1b[0J", "\x1b[1J", "\x1b[2J", "\x1b[K", "\x1b[0K", "\x1b[1K", "\x1b[2K",
    ] {
        let p = feed(seq);
        assert!(p.trace_str().contains('J') || p.trace_str().contains('K'));
    }
}

#[test]
fn csi_insert_delete_chars_lines() {
    for seq in ["\x1b[3P", "\x1b[4@", "\x1b[2L", "\x1b[2M", "\x1b[3X"] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}

#[test]
fn csi_sgr_edge_cases() {
    // 256-color, truecolor, reset, bold+inverse
    for seq in [
        "\x1b[38;5;196m",
        "\x1b[48;5;7m",
        "\x1b[38;2;1;2;3m",
        "\x1b[0m",
        "\x1b[1;7m",
    ] {
        let p = feed(seq);
        assert!(p.trace_str().contains('m'));
    }
}

#[test]
fn csi_invalid_final_and_param_overflow() {
    let p = feed("\x1b[999999999999999999999Z"); // invalid final with huge param
    assert!(!p.trace_str().is_empty());
}
