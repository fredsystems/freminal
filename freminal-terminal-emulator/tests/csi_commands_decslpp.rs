// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level tests for **DECSLPP** (“Set Lines Per Page”).
//! Valid and malformed parameter cases with recovery checks.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decslpp_valid_and_default() {
    let cases = [
        "\x1b[24t", // typical 24 rows
        "\x1b[0t",  // zero/default
        "\x1b[t",   // no param → default
    ];
    for s in cases {
        println!("DECSLPP valid case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(
            !outs.is_empty(),
            "Expected output for valid DECSLPP {:?}",
            s
        );
    }
}

#[test]
fn decslpp_overflow_and_malformed() {
    // overflow and malformed params
    let bad = [
        "\x1b[9999t", // overflow
        "\x1b[x t",   // bad ASCII param
        "\x1b[;t",    // empty param
    ];
    for s in bad {
        let outs = push_seq(s);
        println!("DECSLPP malformed {:?} -> {:?}", s, outs);
        assert!(
            std::panic::catch_unwind(|| push_seq(s)).is_ok(),
            "Parser panicked for malformed {:?}",
            s
        );
    }
}

#[test]
fn decslpp_recovery_after_malformed() {
    // ensure parser recovers and handles next sequence
    let seq = "\x1b[x t\x1b[24t";
    let outs = push_seq(seq);
    println!("DECSLPP recovery outputs -> {:?}", outs);
    assert!(
        outs.iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. })
                || matches!(o, TerminalOutput::Data(_))),
        "Expected Mode/Data output after recovery"
    );
}
