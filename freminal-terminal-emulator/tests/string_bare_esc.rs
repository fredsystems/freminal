// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! A bare ESC ends an OSC, DCS or APC string.
//!
//! An ESC inside a string that is not followed by `\` (ST) ends the string:
//! the body received so far is dispatched, and the ESC starts a new escape
//! sequence, so the byte after it is parsed as part of that sequence. This
//! follows the Williams VT state machine (Ghostty, `WezTerm`). The one exception
//! is `DCS tmux;` passthrough, where a doubled `ESC ESC` is data.

use crossbeam_channel::{Receiver, unbounded};
use freminal_common::buffer_states::osc::AnsiOscType;
use freminal_common::buffer_states::terminal_output::TerminalOutput;
use freminal_common::colors::TerminalColor;
use freminal_common::pty_write::PtyWrite;
use freminal_common::sgr::SelectGraphicRendition;
use freminal_terminal_emulator::ansi::FreminalAnsiParser;
use freminal_terminal_emulator::ansi_components::osc::MAX_OSC_BYTES;
use freminal_terminal_emulator::state::internal::TerminalState;

fn title(text: &str) -> TerminalOutput {
    TerminalOutput::OscResponse(AnsiOscType::SetTitleBar(text.to_owned()))
}

const fn red() -> TerminalOutput {
    TerminalOutput::Sgr(SelectGraphicRendition::Foreground(TerminalColor::Red))
}

fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.set_win_size(80, 24, 8, 16);
    (state, rx)
}

fn replies(rx: &Receiver<PtyWrite>) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        match msg {
            PtyWrite::Write(bytes) => out.push(bytes),
            PtyWrite::Resize(_) | PtyWrite::ApplyPendingResize => {
                panic!("unexpected resize message")
            }
        }
    }
    out
}

// ─── OSC ────────────────────────────────────────────────────────────────────

#[test]
fn unterminated_osc_title_is_set_and_the_following_csi_is_applied() {
    let output = FreminalAnsiParser::new().push(b"\x1b]2;title\x1b[31m");
    assert_eq!(output, vec![title("title"), red()]);
}

#[test]
fn osc_ended_by_a_bare_esc_may_be_followed_by_text() {
    let output = FreminalAnsiParser::new().push(b"\x1b]2;title\x1b[31mhello");
    assert_eq!(
        output,
        vec![
            title("title"),
            red(),
            TerminalOutput::Data(b"hello".to_vec())
        ]
    );
}

#[test]
fn osc_ended_by_a_bare_esc_may_be_followed_by_another_string() {
    // ESC P after the bare ESC starts a DCS, as it would after any ESC.
    let output = FreminalAnsiParser::new().push(b"\x1b]2;t\x1bP$qm\x1b\\");
    assert_eq!(
        output,
        vec![
            title("t"),
            TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec())
        ]
    );
}

#[test]
fn osc_ended_by_esc_esc_then_csi() {
    // The first ESC ends the string. The second ESC is the byte after it, and
    // restarts the escape state, exactly as ESC ESC does anywhere else.
    let output = FreminalAnsiParser::new().push(b"\x1b]2;t\x1b\x1b[31m");
    assert_eq!(output, vec![title("t"), red()]);
}

#[test]
fn osc_still_terminates_at_st_and_bel() {
    let output = FreminalAnsiParser::new().push(b"\x1b]2;a\x1b\\x\x1b]2;b\x07y");
    assert_eq!(
        output,
        vec![
            title("a"),
            TerminalOutput::Data(b"x".to_vec()),
            title("b"),
            TerminalOutput::Data(b"y".to_vec()),
        ]
    );
}

#[test]
fn osc_ended_by_a_bare_esc_is_the_same_split_at_every_read_boundary() {
    let wire = b"\x1b]2;title\x1b[31mhello";
    let expected = FreminalAnsiParser::new().push(wire);
    assert_eq!(expected.len(), 3, "{expected:?}");

    for split in 1..wire.len() {
        let mut parser = FreminalAnsiParser::new();
        let mut output = parser.push(&wire[..split]);
        output.extend(parser.push(&wire[split..]));
        // Plain text may be flushed in two `Data` events across a boundary;
        // only the text is compared, not how it is chunked.
        let flatten = |outputs: Vec<TerminalOutput>| {
            let mut flat = Vec::new();
            for o in outputs {
                match (flat.last_mut(), o) {
                    (Some(TerminalOutput::Data(a)), TerminalOutput::Data(b)) => a.extend(b),
                    (_, o) => flat.push(o),
                }
            }
            flat
        };
        assert_eq!(flatten(output), expected, "split at {split}");
    }
}

#[test]
fn osc_esc_as_the_last_byte_of_a_read_waits_for_the_next_byte() {
    let mut parser = FreminalAnsiParser::new();
    // Nothing is dispatched yet: the ESC might still start ST.
    assert_eq!(parser.push(b"\x1b]2;title\x1b"), vec![]);
    // The next read decides: `[` makes the ESC bare.
    assert_eq!(parser.push(b"[31m"), vec![title("title"), red()]);

    let mut parser = FreminalAnsiParser::new();
    assert_eq!(parser.push(b"\x1b]2;title\x1b"), vec![]);
    assert_eq!(parser.push(b"\\"), vec![title("title")]);
}

#[test]
fn over_cap_osc_ended_by_a_bare_esc_is_dropped_and_the_next_sequence_runs() {
    let mut wire = b"\x1b]2;".to_vec();
    wire.extend(std::iter::repeat_n(b'T', MAX_OSC_BYTES + 10));
    wire.extend_from_slice(b"\x1b[31mhello");
    let output = FreminalAnsiParser::new().push(&wire);
    assert_eq!(output, vec![red(), TerminalOutput::Data(b"hello".to_vec())]);
}

// ─── DCS ────────────────────────────────────────────────────────────────────

#[test]
fn unterminated_dcs_is_dispatched_and_the_following_csi_is_applied() {
    let output = FreminalAnsiParser::new().push(b"\x1bP$qm\x1b[31m");
    assert_eq!(
        output,
        vec![
            // Dispatched exactly as if it had ended in ST.
            TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
            red(),
        ]
    );
}

#[test]
fn dcs_ended_by_esc_esc_then_st_dispatches_once() {
    // The second ESC ends the DCS and restarts the escape state; `\` is then
    // ST in the escape state and produces nothing.
    let output = FreminalAnsiParser::new().push(b"\x1bP$qm\x1b\x1b\\hello");
    assert_eq!(
        output,
        vec![
            TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
            TerminalOutput::Data(b"hello".to_vec()),
        ]
    );
}

#[test]
fn dcs_still_terminates_at_st() {
    let output = FreminalAnsiParser::new().push(b"\x1bP$qm\x1b\\hello");
    assert_eq!(
        output,
        vec![
            TerminalOutput::DeviceControlString(b"P$qm\x1b\\".to_vec()),
            TerminalOutput::Data(b"hello".to_vec()),
        ]
    );
}

#[test]
fn dcs_ended_by_a_bare_esc_is_the_same_split_at_every_read_boundary() {
    let wire = b"\x1bP$qm\x1b[31m";
    let expected = FreminalAnsiParser::new().push(wire);
    assert_eq!(expected.len(), 2, "{expected:?}");
    for split in 1..wire.len() {
        let mut parser = FreminalAnsiParser::new();
        let mut output = parser.push(&wire[..split]);
        output.extend(parser.push(&wire[split..]));
        assert_eq!(output, expected, "split at {split}");
    }
}

#[test]
fn dcs_decrqss_is_answered_and_the_following_sgr_is_applied() {
    let (mut state, rx) = make_state();
    // The unterminated DECRQSS is answered; the CSI after it then runs, which
    // the second (terminated) DECRQSS observes.
    state.handle_incoming_data(b"\x1bP$qm\x1b[31m\x1bP$qm\x1b\\");
    assert_eq!(
        replies(&rx),
        vec![
            b"\x1bP1$r0m\x1b\\".to_vec(),
            b"\x1bP1$r0;31m\x1b\\".to_vec()
        ]
    );
}

#[test]
fn tmux_passthrough_with_doubled_esc_is_one_dcs_ended_only_by_the_real_st() {
    // tmux wraps `ESC [ 31 m` as `ESC ESC [ 31 m`: the doubled ESC is data.
    let output = FreminalAnsiParser::new().push(b"\x1bPtmux;\x1b\x1b[31m\x1b\\hello");
    assert_eq!(
        output,
        vec![
            TerminalOutput::DeviceControlString(b"Ptmux;\x1b\x1b[31m\x1b\\".to_vec()),
            TerminalOutput::Data(b"hello".to_vec()),
        ]
    );
}

#[test]
fn tmux_passthrough_with_doubled_esc_is_applied_through_the_state() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1bPtmux;\x1b\x1b[31m\x1b\\\x1bP$qm\x1b\\");
    assert_eq!(replies(&rx), vec![b"\x1bP1$r0;31m\x1b\\".to_vec()]);
}

#[test]
fn tmux_passthrough_split_inside_a_doubled_esc_is_still_one_dcs() {
    let wire = b"\x1bPtmux;\x1b\x1b[31m\x1b\\";
    let expected = FreminalAnsiParser::new().push(wire);
    assert_eq!(expected.len(), 1, "{expected:?}");
    for split in 1..wire.len() {
        let mut parser = FreminalAnsiParser::new();
        let mut output = parser.push(&wire[..split]);
        output.extend(parser.push(&wire[split..]));
        assert_eq!(output, expected, "split at {split}");
    }
}

#[test]
fn tmux_passthrough_with_an_unpaired_esc_ends_at_that_esc() {
    // A single ESC followed by something other than `\` is not tmux's
    // doubling, so it is a bare ESC and ends the string.
    let output = FreminalAnsiParser::new().push(b"\x1bPtmux;a\x1b[31m");
    assert_eq!(
        output,
        vec![
            TerminalOutput::DeviceControlString(b"Ptmux;a\x1b\\".to_vec()),
            red(),
        ]
    );
}

// ─── APC ────────────────────────────────────────────────────────────────────

#[test]
fn unterminated_apc_is_dispatched_and_the_following_csi_is_applied() {
    let output = FreminalAnsiParser::new().push(b"\x1b_Ga=q,i=1;AAAA\x1b[31m");
    assert_eq!(
        output,
        vec![
            TerminalOutput::ApplicationProgramCommand(b"_Ga=q,i=1;AAAA\x1b\\".to_vec()),
            red(),
        ]
    );
}

#[test]
fn apc_still_terminates_at_st() {
    let output = FreminalAnsiParser::new().push(b"\x1b_Ga=q,i=1;AAAA\x1b\\hello");
    assert_eq!(
        output,
        vec![
            TerminalOutput::ApplicationProgramCommand(b"_Ga=q,i=1;AAAA\x1b\\".to_vec()),
            TerminalOutput::Data(b"hello".to_vec()),
        ]
    );
}

#[test]
fn apc_ended_by_a_bare_esc_is_the_same_split_at_every_read_boundary() {
    let wire = b"\x1b_Ga=q,i=1;AAAA\x1b[31m";
    let expected = FreminalAnsiParser::new().push(wire);
    assert_eq!(expected.len(), 2, "{expected:?}");
    for split in 1..wire.len() {
        let mut parser = FreminalAnsiParser::new();
        let mut output = parser.push(&wire[..split]);
        output.extend(parser.push(&wire[split..]));
        assert_eq!(output, expected, "split at {split}");
    }
}

#[test]
fn apc_kitty_query_is_answered_before_the_following_csi_runs() {
    let (mut state, rx) = make_state();
    // The kitty graphics query ends at the bare ESC; the DA1 that follows is
    // answered after it, in stream order.
    state.handle_incoming_data(b"\x1b_Ga=q,f=24,i=31;AAAA\x1b[c");
    let got = replies(&rx);

    let (mut direct, direct_rx) = make_state();
    direct.handle_incoming_data(b"\x1b[c");
    let da1 = replies(&direct_rx);
    assert_eq!(da1.len(), 1, "{da1:?}");

    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0], b"\x1b_Gi=31;OK\x1b\\".to_vec());
    assert_eq!(got[1], da1[0]);
}
