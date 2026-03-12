// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level tests for **DECSTBM** (“Set Top and Bottom Margins”).
//! Covers valid, default, swapped, malformed, and recovery behavior.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decstbm_valid_and_default() {
    let cases = [
        "\x1b[1;24r", // typical full-screen
        "\x1b[r",     // default (entire screen)
        "\x1b[5;10r", // mid-range
        "\x1b[10r",   // single param — top margin only, bottom defaults to page size
        "\x1b[;r",    // both params omitted — full screen
    ];
    for s in cases {
        println!("DECSTBM valid case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(
            outs.iter()
                .any(|o| matches!(o, TerminalOutput::SetTopAndBottomMargins { .. })),
            "Expected SetTopAndBottomMargins for valid DECSTBM {:?}, got: {:?}",
            s,
            outs,
        );
    }
}

#[test]
fn decstbm_single_param_sets_top_margin_only() {
    let outs = push_seq("\x1b[3r");
    let margin = outs.iter().find_map(|o| match o {
        TerminalOutput::SetTopAndBottomMargins {
            top_margin,
            bottom_margin,
        } => Some((*top_margin, *bottom_margin)),
        _ => None,
    });
    assert_eq!(
        margin,
        Some((3, usize::MAX)),
        "Single param '3r' should set top=3, bottom=MAX: got {outs:?}"
    );
}

#[test]
fn decstbm_swapped_and_malformed() {
    // swapped and malformed param sets
    let bad = [
        "\x1b[20;10r", // swapped values (should clamp or reset)
        "\x1b[x;r",    // invalid ascii
    ];
    for s in bad {
        let outs = push_seq(s);
        println!("DECSTBM malformed {:?} -> {:?}", s, outs);
        // parser must remain stable
        assert!(
            std::panic::catch_unwind(|| push_seq(s)).is_ok(),
            "Parser panicked for malformed {:?}",
            s
        );
    }
}

#[test]
fn decstbm_recovery_after_invalid() {
    let seq = "\x1b[20;10r\x1b[1;24r"; // first invalid, second valid
    let outs = push_seq(seq);
    println!("DECSTBM recovery outs -> {:?}", outs);

    // Parser must not panic, and must emit Invalid + SetTopAndBottomMargins
    assert!(
        std::panic::catch_unwind(|| push_seq(seq)).is_ok(),
        "Parser panicked during DECSTBM recovery"
    );

    // Expect an Invalid followed by a valid margin command
    assert!(
        outs.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
        "Expected Invalid output for malformed first sequence, got: {:?}",
        outs
    );
    assert!(
        outs.iter()
            .any(|o| matches!(o, TerminalOutput::SetTopAndBottomMargins { .. })),
        "Expected SetTopAndBottomMargins output for valid recovery, got: {:?}",
        outs
    );
}
