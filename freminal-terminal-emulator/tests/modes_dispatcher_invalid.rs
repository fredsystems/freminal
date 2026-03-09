// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn unknown_private_mode_sequence_is_invalid() {
    let outs = push_seq("\x1b[?9999l");
    println!("unknown private disable -> {:?}", outs);
    assert!(
        outs.iter()
            .any(|o| matches!(o, TerminalOutput::Invalid | TerminalOutput::Mode { .. })),
        "Expected Invalid or Mode for unknown mode"
    );
}
