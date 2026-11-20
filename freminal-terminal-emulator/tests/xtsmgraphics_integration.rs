// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`) tests, plus XTSMTITLE /
//! XTRMTITLE (`CSI > Pm t` / `CSI > Pm T`) being recognised and ignored.
//!
//! Raw escape bytes go through `TerminalState::handle_incoming_data` and the
//! reply is read from the PTY write channel. The expected replies follow
//! xterm built with sixel but without `ReGIS` (`charproc.c`,
//! `CASE_GRAPHICS_ATTRIBUTES`).

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::sixel::MAX_PALETTE;
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

fn make_state() -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    (TerminalState::new(tx, None), rx)
}

fn feed(state: &mut TerminalState, rx: &Receiver<PtyWrite>, input: &[u8]) -> Vec<u8> {
    while rx.try_recv().is_ok() {}
    state.handle_incoming_data(input);
    let mut buf = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    buf
}

fn query(request: &str) -> Vec<u8> {
    let (mut state, rx) = make_state();
    feed(&mut state, &rx, request.as_bytes())
}

#[test]
fn colour_registers_read_reset_and_maximum_report_the_fixed_palette() {
    let expected = format!("\x1b[?1;0;{MAX_PALETTE}S");
    assert_eq!(MAX_PALETTE, 256);
    for request in ["\x1b[?1;1;0S", "\x1b[?1;2;0S", "\x1b[?1;4;0S"] {
        assert_eq!(query(request), expected.as_bytes(), "request {request:?}");
    }
}

#[test]
fn colour_registers_set_fails_because_the_count_is_fixed() {
    assert_eq!(query("\x1b[?1;3;64S"), b"\x1b[?1;3S");
}

#[test]
fn colour_registers_unknown_action_is_error_in_pa() {
    assert_eq!(query("\x1b[?1;9;0S"), b"\x1b[?1;2S");
    assert_eq!(query("\x1b[?1;0;0S"), b"\x1b[?1;2S");
}

#[test]
fn sixel_geometry_read_is_text_area_pixel_size() {
    let (mut state, rx) = make_state();
    state.set_win_size(100, 40, 8, 16);
    assert_eq!(
        feed(&mut state, &rx, b"\x1b[?2;1;0S"),
        b"\x1b[?2;0;800;640S"
    );
}

#[test]
fn sixel_geometry_read_is_clamped_to_the_decoder_maximum() {
    let (mut state, rx) = make_state();
    state.set_win_size(1000, 1000, 20, 40);
    assert_eq!(
        feed(&mut state, &rx, b"\x1b[?2;1;0S"),
        b"\x1b[?2;0;4096;4096S"
    );
}

#[test]
fn sixel_geometry_maximum_is_the_decoder_limit() {
    assert_eq!(query("\x1b[?2;4;0S"), b"\x1b[?2;0;4096;4096S");
}

#[test]
fn sixel_geometry_reset_and_set_fail() {
    assert_eq!(query("\x1b[?2;2;0S"), b"\x1b[?2;3S");
    assert_eq!(query("\x1b[?2;3;640S"), b"\x1b[?2;3S");
}

#[test]
fn sixel_geometry_set_with_width_and_height_is_malformed_as_in_xterm() {
    // xterm requires exactly three parameters (`nparam != 3` is dropped), so
    // the documented `Pv = width ; height` form of a set gets no reply.
    assert_eq!(query("\x1b[?2;3;640;480S"), b"");
}

#[test]
fn sixel_geometry_unknown_action_is_error_in_pa() {
    assert_eq!(query("\x1b[?2;5;0S"), b"\x1b[?2;2S");
}

#[test]
fn regis_geometry_is_an_unknown_item_as_in_xterm_without_regis() {
    for pa in 1..=4 {
        assert_eq!(query(&format!("\x1b[?3;{pa};0S")), b"\x1b[?3;1S", "Pa={pa}");
    }
}

#[test]
fn unknown_item_is_error_in_pi() {
    assert_eq!(query("\x1b[?4;1;0S"), b"\x1b[?4;1S");
    assert_eq!(query("\x1b[?0;1;0S"), b"\x1b[?0;1S");
}

#[test]
fn wrong_parameter_count_gets_no_reply() {
    for request in [
        "\x1b[?S",
        "\x1b[?1S",
        "\x1b[?1;1S",
        "\x1b[?1;1;0;0S",
        "\x1b[?;;;S",
    ] {
        assert_eq!(query(request), b"", "request {request:?}");
    }
}

#[test]
fn missing_pi_or_pa_gets_no_reply() {
    for request in ["\x1b[?;1;0S", "\x1b[?1;;0S"] {
        assert_eq!(query(request), b"", "request {request:?}");
    }
}

#[test]
fn reply_follows_s8c1t() {
    let (mut state, rx) = make_state();
    state.handle_incoming_data(b"\x1b G");
    assert_eq!(
        feed(&mut state, &rx, b"\x1b[?1;1;0S"),
        b"\x9b?1;0;256S".to_vec()
    );
    state.handle_incoming_data(b"\x1b F");
    assert_eq!(feed(&mut state, &rx, b"\x1b[?1;1;0S"), b"\x1b[?1;0;256S");
}

#[test]
fn plain_su_is_not_confused_with_xtsmgraphics() {
    // `CSI 3 S` is SU (scroll up); it must not produce a reply.
    assert_eq!(query("\x1b[3S"), b"");
}

#[test]
fn xtsmtitle_and_xtrmtitle_are_ignored_without_reply() {
    for request in [
        "\x1b[>t",
        "\x1b[>0;1t",
        "\x1b[>2t",
        "\x1b[>T",
        "\x1b[>0;1T",
        "\x1b[>3T",
    ] {
        let (mut state, rx) = make_state();
        assert_eq!(
            feed(&mut state, &rx, request.as_bytes()),
            b"",
            "{request:?}"
        );
        // The parser recovers: a following query still works.
        assert_eq!(
            feed(&mut state, &rx, b"\x1b[?1;1;0S"),
            b"\x1b[?1;0;256S",
            "{request:?}"
        );
    }
}
