// Copyright (C) 2024-2026 Fred Clausen
// MIT license, see LICENSE file.

//! Ensures the mode dispatcher in `ansi_components/mode.rs` correctly interprets
//! multiple mode sequences in a single stream.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn multiple_modes_in_stream() {
    let seq = "\x1b[?1h\x1b[?7h\x1b[?25l";
    let outs = push_seq(seq);
    println!("multi-mode stream -> {:?}", outs);
    assert!(
        outs.len() >= 3,
        "expected at least 3 mode outputs, got {:?}",
        outs
    );
}

#[test]
fn mixed_enable_disable_modes() {
    let seq = "\x1b[?1h\x1b[?7l\x1b[20h";
    let outs = push_seq(seq);
    println!("mixed -> {:?}", outs);
    assert!(
        outs.iter()
            .all(|o| matches!(o, TerminalOutput::Mode { .. }))
    );
}
