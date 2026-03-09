// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Parser-level tests for **XT Version and Device Attribute queries**.
//! This validates:
//! - ESC[c (DA1)
//! - ESC[>c (XTVERSION query)
//! - ESC[>0c / ESC[>1c (DA2)
//! - ESC[>0q (XTerm style version report)
//!   and malformed cases with recovery.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn report_xtversion_and_device_attributes() {
    let cases = [
        (
            "\x1b[>c",
            "XTVERSION no param (RequestXtVersion)",
            TerminalOutput::RequestXtVersion,
        ),
        (
            "\x1b[>0c",
            "DA2 explicit 0 param",
            TerminalOutput::RequestSecondaryDeviceAttributes { param: 0 },
        ),
        (
            "\x1b[>1c",
            "DA2 explicit 1 param",
            TerminalOutput::RequestSecondaryDeviceAttributes { param: 1 },
        ),
        (
            "\x1b[>0q",
            "XTerm style >0q (RequestDeviceNameAndVersion)",
            TerminalOutput::RequestDeviceNameAndVersion,
        ),
        (
            "\x1b[c",
            "Primary Device Attributes (DA1)",
            TerminalOutput::RequestDeviceAttributes,
        ),
    ];

    for (seq, desc, expected) in cases {
        println!("Testing {:?}: {}", seq, desc);
        let outs = push_seq(seq);
        for o in &outs {
            println!("variant: {:?}", o);
        }

        // Parser should never panic
        assert!(
            std::panic::catch_unwind(|| push_seq(seq)).is_ok(),
            "Parser panicked for {} ({:?})",
            desc,
            seq
        );

        // Expect exactly one of our known variants
        assert!(
            outs.iter().any(|o| match (o, &expected) {
                (TerminalOutput::RequestXtVersion, TerminalOutput::RequestXtVersion)
                | (
                    TerminalOutput::RequestDeviceNameAndVersion,
                    TerminalOutput::RequestDeviceNameAndVersion,
                )
                | (
                    TerminalOutput::RequestDeviceAttributes,
                    TerminalOutput::RequestDeviceAttributes,
                ) => true,
                (
                    TerminalOutput::RequestSecondaryDeviceAttributes { param: a },
                    TerminalOutput::RequestSecondaryDeviceAttributes { param: b },
                ) if a == b => true,
                _ => false,
            }),
            "Expected {:?} for {}, got {:?}",
            expected,
            desc,
            outs
        );
    }
}

#[test]
fn report_xtversion_malformed_and_recovery() {
    let bad = [
        "\x1b[>x",    // bad param
        "\x1b[>1;2c", // unsupported multi-param
        "\x1b[>",     // truncated
    ];

    for s in bad {
        let outs = push_seq(s);
        println!("XTVersion malformed {:?} -> {:?}", s, outs);
        assert!(
            std::panic::catch_unwind(|| push_seq(s)).is_ok(),
            "Parser panicked for malformed {:?}",
            s
        );
        // Parser may emit Invalid or nothing
        assert!(
            outs.is_empty() || outs.iter().any(|o| matches!(o, TerminalOutput::Invalid)),
            "Expected graceful handling for malformed {:?}, got {:?}",
            s,
            outs
        );
    }

    // Recovery check
    let rec = push_seq("\x1b[>x\x1b[>c\x1b[>0q");
    println!("XTVersion recovery -> {:?}", rec);
    // Parser should return to stable output; expect at least one valid variant
    assert!(
        rec.iter().any(|o| matches!(
            o,
            TerminalOutput::RequestXtVersion
                | TerminalOutput::RequestSecondaryDeviceAttributes { .. }
                | TerminalOutput::RequestDeviceNameAndVersion
        )),
        "Expected valid XTVersion or DA2 output after recovery, got {:?}",
        rec
    );
}
