// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Phase 13: Exhaustive CSI command coverage (movement, erase, insert/delete, scroll, tabs)

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
fn csi_move_family_full_matrix() {
    // CUP (H), CHA (G), CUU (A), CUD (B), CUF (C), CUB (D), HVP (f)
    for seq in [
        "\x1b[1;1H",
        "\x1b[24;80H",
        "\x1b[10G",
        "\x1b[A",
        "\x1b[5A",
        "\x1b[B",
        "\x1b[7B",
        "\x1b[C",
        "\x1b[12C",
        "\x1b[D",
        "\x1b[3D",
        "\x1b[5;10f",
    ] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}

#[test]
fn csi_erase_family() {
    for seq in [
        "\x1b[J", "\x1b[0J", "\x1b[1J", "\x1b[2J", "\x1b[K", "\x1b[0K", "\x1b[1K", "\x1b[2K",
    ] {
        let p = feed(seq);
        assert!(p.trace_str().contains('J') || p.trace_str().contains('K'));
    }
}

#[test]
fn csi_insert_delete_chars_and_lines() {
    // DCH (P), ICH (@), IL (L), DL (M), ECH (X)
    for seq in [
        "\x1b[P", "\x1b[3P", "\x1b[@", "\x1b[4@", "\x1b[2L", "\x1b[2M", "\x1b[3X",
    ] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}

#[test]
fn csi_scroll_region_and_scroll() {
    // DECSTBM: set top/bottom; SU (S), SD (T)
    for seq in [
        "\x1b[3;20r",
        "\x1b[r",
        "\x1b[S",
        "\x1b[5S",
        "\x1b[T",
        "\x1b[4T",
    ] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}

#[test]
fn csi_tab_stops() {
    // HTS (H), TBC (g) with params 0/3
    for seq in ["\x1bH", "\x1b[0g", "\x1b[3g"] {
        let p = feed(seq);
        assert!(!p.trace_str().is_empty());
    }
}

#[test]
fn csi_sgr_big_matrix() {
    for seq in [
        "\x1b[0m",
        "\x1b[1m",
        "\x1b[2m",
        "\x1b[3m",
        "\x1b[4m",
        "\x1b[5m",
        "\x1b[7m",
        "\x1b[9m",
        "\x1b[21m",
        "\x1b[22m",
        "\x1b[23m",
        "\x1b[24m",
        "\x1b[25m",
        "\x1b[27m",
        "\x1b[29m",
        "\x1b[30m",
        "\x1b[31m",
        "\x1b[32m",
        "\x1b[33m",
        "\x1b[34m",
        "\x1b[35m",
        "\x1b[36m",
        "\x1b[37m",
        "\x1b[90m",
        "\x1b[97m",
        "\x1b[39m",
        "\x1b[40m",
        "\x1b[41m",
        "\x1b[42m",
        "\x1b[43m",
        "\x1b[44m",
        "\x1b[45m",
        "\x1b[46m",
        "\x1b[47m",
        "\x1b[100m",
        "\x1b[107m",
        "\x1b[49m",
        "\x1b[38;5;200m",
        "\x1b[48;5;45m",
        "\x1b[38;2;12;34;56m",
        "\x1b[48;2;0;128;255m",
    ] {
        let p = feed(seq);
        assert!(p.trace_str().contains('m'));
    }
}

#[test]
fn csi_error_paths_cover_invalid_finals_and_overflows() {
    // invalid final
    let p = feed("\x1b[12;24Z");
    assert!(!p.trace_str().is_empty());
    // param overflow
    let p = feed("\x1b[9999999999999999999A");
    assert!(!p.trace_str().is_empty());
}
