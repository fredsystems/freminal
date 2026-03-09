// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Tests for DEC Autowrap Mode (DECAWM, ?7h / ?7l)

use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;

fn push_seq(seq: &str) -> Vec<TerminalOutput> {
    let mut parser = FreminalAnsiParser::default();
    parser.push(seq.as_bytes())
}

#[test]
fn decawm_enable_disable() {
    let enable = push_seq("\x1b[?7h");
    let disable = push_seq("\x1b[?7l");
    println!("enable -> {:?}\ndisable -> {:?}", enable, disable);
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
fn decawm_malformed_graceful() {
    let bad = ["\x1b[?xh", "\x1b[?7x", "\x1b[?7"];
    for s in bad {
        let outs = push_seq(s);
        println!("malformed {:?} -> {:?}", s, outs);
        assert!(std::panic::catch_unwind(|| push_seq(s)).is_ok());
    }
}
