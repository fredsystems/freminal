// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level coverage for DECRQM (Request Mode) and DECSCUSR (Cursor Style).
//! Includes private ('?') and regular forms, valid ranges, and malformed params.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decrqm_private_and_regular_valid() {
    // Private mode query: ?25$p (cursor visibility), and a regular mode like 26$p.
    let cases = [
        "\x1b[?25$p",
        "\x1b[26$p",
        // Add a second private/regular for breadth:
        "\x1b[?3$p", // DECCOLM query (80/132 col)
        "\x1b[20$p", // LNM query
    ];

    for s in cases {
        println!("DECRQM case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(!outs.is_empty(), "DECRQM produced no output for {:?}", s);
        // If you emit a specific variant (e.g., TerminalOutput::ModeReport),
        // assert it here:
        // assert!(outs.iter().any(|o| matches!(o, TerminalOutput::ModeReport { .. })));
    }
}

#[test]
fn decrqm_malformed_graceful() {
    // Bad param & punctuation placements
    let bad = ["\x1b[?$p", "\x1b[?x$p", "\x1b[$p", "\x1b[?25$"];

    for s in bad {
        let outs = push_seq(s);
        println!("DECRQM malformed {:?} -> {:?}", s, outs);

        // parser must not panic and should produce either an empty result (truncated)
        // or some placeholder variant like Mode(UnknownQuery([])) or Data(...)
        assert!(
            outs.is_empty()
                || outs
                    .iter()
                    .any(|o| matches!(o, TerminalOutput::Mode { .. } | TerminalOutput::Data(_))),
            "Unexpected output for malformed {:?}: {:?}",
            s,
            outs
        );
    }
}

#[test]
fn decscusr_valid_range_and_default_and_invalid() {
    // DECSCUSR is CSI Ps ' ' q (note the SPACE before 'q')
    // Typical values:
    // 0 or 1: blinking block
    // 2: steady block
    // 3: blinking underline
    // 4: steady underline
    // 5: blinking bar
    // 6: steady bar
    let cases = [
        "\x1b q", // default
        "\x1b[0 q", "\x1b[1 q", "\x1b[2 q", "\x1b[3 q", "\x1b[4 q", "\x1b[5 q", "\x1b[6 q",
    ];

    for s in cases {
        println!("DECSCUSR case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(!outs.is_empty(), "DECSCUSR produced no output for {:?}", s);
        // If a dedicated variant exists (e.g., TerminalOutput::CursorStyle),
        // assert that here.
    }

    // Malformed param should not panic
    let bad = ["\x1b[x q", "\x1b[; q", "\x1b[q"]; // missing space param
    for s in bad {
        let outs = push_seq(s);
        println!("DECSCUSR malformed {:?} -> {:?}", s, outs);
        assert!(
            !outs.is_empty(),
            "Expected some invalid/error output for malformed {:?}",
            s
        );
    }
}
