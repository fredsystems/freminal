// Copyright (C) 2024-2026 Fred Clausen
// MIT license, see LICENSE file.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn unknown_mode_fallback() {
    let outs = push_seq("\x1b[?9999h");
    println!("unknown enable -> {:?}", outs);
    // parser may mark as Invalid or Mode(Unknown)
    assert!(
        outs.iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. } | TerminalOutput::Invalid)),
        "Expected Mode or Invalid for unknown private mode, got {:?}",
        outs
    );
}
