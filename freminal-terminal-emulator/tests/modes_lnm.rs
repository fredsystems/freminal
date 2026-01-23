// Copyright (C) 2024-2025 Fred Clausen
// MIT license, see LICENSE file.

use freminal_terminal_emulator::ansi::*;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn lnm_enable_disable() {
    let enable = push_seq("\x1b[20h");
    let disable = push_seq("\x1b[20l");
    println!("LNM enable {:?} disable {:?}", enable, disable);
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
