// Copyright (C) 2024-2026 Fred Clausen
// MIT license, see LICENSE file.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decom_enable_disable() {
    let enable = push_seq("\x1b[?6h");
    let disable = push_seq("\x1b[?6l");
    println!("enable {:?}\ndisable {:?}", enable, disable);
    assert!(
        enable
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. }))
    );
    assert!(
        disable
            .iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. }))
    );
}

#[test]
fn decom_invalid_and_recovery() {
    let seq = "\x1b[?xh\x1b[?6h";
    let outs = push_seq(seq);
    println!("recovery -> {:?}", outs);
    assert!(
        outs.iter()
            .any(|o| matches!(o, TerminalOutput::Mode { .. }))
    );
}
