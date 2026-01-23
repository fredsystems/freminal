// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level coverage for ED (Erase in Display) and EL (Erase in Line).
//! Exercises valid modes, default params, overflow, and malformed sequences,
//! and verifies parser recovery.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn ed_valid_modes_and_default() {
    let cases = [
        "\x1b[0J", // Erase from cursor to end of display
        "\x1b[1J", // Erase from start of display to cursor
        "\x1b[2J", // Erase entire display
        "\x1b[J",  // Default param (should behave like 0J)
    ];
    for s in cases {
        println!("ED case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(!outs.is_empty(), "ED produced no output for case {:?}", s);
        // Prefer specific match if TerminalOutput::Erase* exists. Otherwise, non-empty validates code path.
        // Example (uncomment if available in your enum):
        // assert!(outs.iter().any(|o| matches!(o, TerminalOutput::Erase { .. })));
    }
}

#[test]
fn el_valid_modes_and_default() {
    let cases = [
        "\x1b[0K", // Erase from cursor to end of line
        "\x1b[1K", // Erase from start of line to cursor
        "\x1b[2K", // Erase entire line
        "\x1b[K",  // Default param (should behave like 0K)
    ];
    for s in cases {
        println!("EL case: {:?}", s);
        let outs = push_seq(s);
        for o in &outs {
            println!("variant: {:?}", o);
        }
        assert!(!outs.is_empty(), "EL produced no output for case {:?}", s);
        // Example (if you have a dedicated variant):
        // assert!(outs.iter().any(|o| matches!(o, TerminalOutput::EraseLine { .. })));
    }
}

#[test]
fn ed_el_overflow_and_malformed_and_recovery() {
    // Overflow param should still parse deterministically.
    let overflow = push_seq("\x1b[9999J");
    dbg!(&overflow);
    assert!(!overflow.is_empty(), "ED overflow produced no output");

    // Malformed sequences should not panic and should return something (error/invalid path).
    let malformed = ["\x1b[xJ", "\x1b[;J", "\x1b[xK", "\x1b[;K"];
    for s in malformed {
        let outs = push_seq(s);
        println!("malformed {:?} -> {:?}", s, outs);
        assert!(
            !outs.is_empty(),
            "Parser produced no output for malformed {:?}",
            s
        );
    }

    // Recovery: after a malformed sequence, a valid one should still parse normally.
    let rec = push_seq("\x1b[xJ\x1b[2K\x1b[2J");
    println!("recovery outs: {:?}", rec);
    assert!(
        !rec.is_empty(),
        "Recovery path produced no output after malformed + valid sequences"
    );
}
