// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level exhaustive SGR coverage:
//! - single attributes
//! - combined attributes
//! - 256-color foreground/background
//! - true-color RGB foreground/background
//! - malformed/partial sequences & recovery

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn sgr_single_attributes() {
    // Reset + common attributes
    let singles = [
        "\x1b[0m",  // reset
        "\x1b[1m",  // bold
        "\x1b[2m",  // faint
        "\x1b[3m",  // italic
        "\x1b[4m",  // underline
        "\x1b[5m",  // blink
        "\x1b[7m",  // inverse
        "\x1b[8m",  // hidden
        "\x1b[9m",  // crossed-out
        "\x1b[22m", // normal intensity
        "\x1b[24m", // underline off
        "\x1b[27m", // inverse off
        "\x1b[29m", // crossed-out off
    ];

    for s in singles {
        let outs = push_seq(s);
        println!("SGR single {:?} -> {:?}", s, outs);
        assert!(
            !outs.is_empty(),
            "SGR single produced no output for {:?}",
            s
        );
        // If your enum has TerminalOutput::Sgr { .. }, assert it:
        assert!(
            outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
            "Expected Sgr variant for {:?}, got {:?}",
            s,
            outs
        );
    }
}

#[test]
fn sgr_combined_attributes() {
    // Combine attributes & color
    let combos = [
        "\x1b[1;4m",      // bold + underline
        "\x1b[1;31m",     // bold + red (standard color)
        "\x1b[0;1;4;33m", // reset + bold + underline + yellow
        "\x1b[3;9m",      // italic + crossed-out
        "\x1b[7;27m",     // inverse + inverse off
    ];

    for s in combos {
        let outs = push_seq(s);
        println!("SGR combo {:?} -> {:?}", s, outs);
        assert!(
            outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
            "Expected Sgr variant for combo {:?}, got {:?}",
            s,
            outs
        );
    }
}

#[test]
fn sgr_256_color_fg_bg() {
    // 256-color palette: 38;5;NUM (FG) and 48;5;NUM (BG)
    let cases = [
        "\x1b[38;5;196m", // bright red FG
        "\x1b[48;5;23m",  // teal-ish BG
        "\x1b[38;5;0m",   // black FG
        "\x1b[48;5;255m", // white BG
    ];
    for s in cases {
        let outs = push_seq(s);
        println!("SGR 256 {:?} -> {:?}", s, outs);
        assert!(
            outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
            "Expected Sgr for 256-color {:?}, got {:?}",
            s,
            outs
        );
    }
}

#[test]
fn sgr_truecolor_fg_bg() {
    // True-color RGB: 38;2;R;G;B and 48;2;R;G;B
    let cases = [
        "\x1b[38;2;255;0;128m",   // magenta-ish FG
        "\x1b[48;2;0;64;255m",    // blue-ish BG
        "\x1b[38;2;12;34;56m",    // arbitrary FG
        "\x1b[48;2;200;150;100m", // arbitrary BG
    ];
    for s in cases {
        let outs = push_seq(s);
        println!("SGR truecolor {:?} -> {:?}", s, outs);
        assert!(
            outs.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
            "Expected Sgr for truecolor {:?}, got {:?}",
            s,
            outs
        );
    }
}

#[test]
fn sgr_malformed_and_recovery() {
    // Malformed/partial sequences should not panic and should drive error paths:
    let bad = [
        "\x1b[38;2;255;255m", // missing B component
        "\x1b[38;5m",         // missing palette index
        "\x1b[38;2;256;0;0m", // out-of-range (implementation typically clamps/invalid)
        "\x1b[xyzm",          // garbage params
    ];
    for s in bad {
        let outs = push_seq(s);
        println!("SGR malformed {:?} -> {:?}", s, outs);
        assert!(
            !outs.is_empty(),
            "Expected some invalid/error output for malformed {:?}",
            s
        );
    }

    // Recovery: after malformed inputs, a valid chain should still parse fine.
    let rec = push_seq("\x1b[38;5m\x1b[1;4m\x1b[0m");
    println!("SGR recovery -> {:?}", rec);
    assert!(
        rec.iter().any(|o| matches!(o, TerminalOutput::Sgr { .. })),
        "Expected Sgr during recovery sequence"
    );
}
