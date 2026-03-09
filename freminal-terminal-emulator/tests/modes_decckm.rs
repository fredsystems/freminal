// Copyright (C) 2024-2026 Fred Clausen
// MIT license, see LICENSE file.

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decckm_enable_disable() {
    let enable = push_seq("\x1b[?1h");
    let disable = push_seq("\x1b[?1l");
    println!("DECCKM enable {:?} disable {:?}", enable, disable);
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
