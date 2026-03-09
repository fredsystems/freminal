// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! SGR permutations and resets

use freminal_terminal_emulator::ansi_components::csi::AnsiCsiParser;
use freminal_terminal_emulator::ansi_components::tracer::SequenceTraceable;

#[test]
fn sgr_all_resets_and_styles() {
    let mut p = AnsiCsiParser::default();
    let seqs = [
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
        "\x1b[37m",
        "\x1b[90m",
        "\x1b[97m",
        "\x1b[39m",
        "\x1b[40m",
        "\x1b[47m",
        "\x1b[100m",
        "\x1b[107m",
        "\x1b[49m",
        "\x1b[38;5;200m",
        "\x1b[48;5;45m",
        "\x1b[38;2;12;34;56m",
        "\x1b[48;2;0;128;255m",
    ];
    for s in seqs {
        for &b in s.as_bytes() {
            let _ = p.push(b);
        }
        assert!(p.current_trace_str().contains('m'));
        p.clear_trace();
    }
}
