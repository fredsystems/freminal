// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end tests for the OSC 99 `p=?` capability query and the `p=alive`
//! gating, answered on the PTY thread from the host capabilities (Task 130.5).
//!
//! Each test feeds raw bytes through `TerminalState::handle_incoming_data` and
//! inspects the bytes written to the PTY and the window commands produced.

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::window_manipulation::{Osc99ControlKind, WindowManipulation};
use freminal_common::host_capabilities::{
    HostCapabilities, Osc99ActivationReport, Osc99CloseEvents, Osc99Features, Osc99Support,
};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

const TAIL: &str = "o=always,unfocused,invisible:p=title,body,icon,buttons,alive,close,?:s=system,silent:u=0,1,2:w=1";

fn caps(
    activation_report: Osc99ActivationReport,
    close_events: Osc99CloseEvents,
) -> HostCapabilities {
    HostCapabilities {
        osc99: Osc99Support::Supported(Osc99Features {
            activation_report,
            close_events,
        }),
    }
}

fn full_caps() -> HostCapabilities {
    caps(Osc99ActivationReport::Reported, Osc99CloseEvents::Reported)
}

/// A state with the given capabilities and the PTY write receiver.
fn make_state(capabilities: HostCapabilities) -> (TerminalState, Receiver<PtyWrite>) {
    let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
    let mut state = TerminalState::new(tx, None);
    state.handler.set_host_capabilities(capabilities);
    (state, rx)
}

/// Drain every `PtyWrite::Write` currently queued and concatenate the bytes.
fn drain(rx: &Receiver<PtyWrite>) -> Vec<u8> {
    let mut buf = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let PtyWrite::Write(bytes) = msg {
            buf.extend_from_slice(&bytes);
        }
    }
    buf
}

fn osc99_window_commands(state: &TerminalState) -> usize {
    state
        .window_commands
        .iter()
        .filter(|c| matches!(c, WindowManipulation::Osc99Control { .. }))
        .count()
}

#[test]
fn query_while_unsupported_writes_nothing_and_pushes_no_command() {
    let (mut state, rx) = make_state(HostCapabilities::default());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    assert!(drain(&rx).is_empty());
    assert!(state.window_commands.is_empty());
}

#[test]
fn query_with_full_features_replies_with_exact_bytes_and_no_command() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=abc:p=?;a=report:c=1:{TAIL}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
    assert!(state.window_commands.is_empty());
}

#[test]
fn query_with_activation_not_reported_omits_a_key() {
    let (mut state, rx) = make_state(caps(
        Osc99ActivationReport::NotReported,
        Osc99CloseEvents::Reported,
    ));
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=abc:p=?;c=1:{TAIL}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
}

#[test]
fn query_with_close_not_reported_omits_c_key() {
    let (mut state, rx) = make_state(caps(
        Osc99ActivationReport::Reported,
        Osc99CloseEvents::NotReported,
    ));
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=abc:p=?;a=report:{TAIL}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
}

#[test]
fn query_with_neither_reported_starts_with_occasion_key() {
    let (mut state, rx) = make_state(caps(
        Osc99ActivationReport::NotReported,
        Osc99CloseEvents::NotReported,
    ));
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=abc:p=?;{TAIL}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
}

#[test]
fn query_without_id_replies_with_id_zero() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;p=?;\x1b\\");
    let reply = drain(&rx);
    assert!(
        reply.starts_with(b"\x1b]99;i=0:p=?;"),
        "unexpected reply: {reply:?}"
    );
}

#[test]
fn query_reply_precedes_da1_reply_in_one_buffer() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\\x1b[c");
    let reply = drain(&rx);
    let osc_pos = reply
        .windows(b"\x1b]99;i=abc:p=?;".len())
        .position(|w| w == b"\x1b]99;i=abc:p=?;")
        .expect("OSC 99 reply present");
    let da1_pos = reply
        .windows(b"\x1b[?".len())
        .position(|w| w == b"\x1b[?")
        .expect("DA1 reply present");
    assert_eq!(osc_pos, 0, "OSC 99 reply must come first");
    assert!(osc_pos < da1_pos, "OSC 99 reply must precede DA1");
}

#[test]
fn query_reply_uses_c1_framing_in_s8c1t_mode() {
    let (mut state, rx) = make_state(full_caps());
    // ESC SP G selects 8-bit C1 controls (S8C1T).
    state.handle_incoming_data(b"\x1b G");
    let _ = drain(&rx);
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    let reply = drain(&rx);
    assert_eq!(reply.first(), Some(&0x9d), "reply: {reply:?}");
    assert_eq!(reply.last(), Some(&0x9c), "reply: {reply:?}");
    assert!(reply.windows(10).any(|w| w == b"99;i=abc:p"));
}

#[test]
fn alive_while_unsupported_pushes_no_window_command() {
    let (mut state, _rx) = make_state(HostCapabilities::default());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=alive;\x1b\\");
    assert_eq!(osc99_window_commands(&state), 0);
    assert!(state.window_commands.is_empty());
}

#[test]
fn alive_while_supported_pushes_one_alive_control() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=alive;\x1b\\");
    assert_eq!(state.window_commands.len(), 1);
    match &state.window_commands[0] {
        WindowManipulation::Osc99Control { id, kind } => {
            assert_eq!(id.as_deref(), Some("abc"));
            assert_eq!(*kind, Osc99ControlKind::Alive);
        }
        other => panic!("expected Osc99Control, got: {other:?}"),
    }
}

// ── While unsupported, every request is dropped (130 review finding 14) ──────

#[test]
fn display_notification_while_unsupported_pushes_no_window_command() {
    let (mut state, rx) = make_state(HostCapabilities::default());
    state.handle_incoming_data(b"\x1b]99;i=1;hello\x1b\\");
    assert!(state.window_commands.is_empty());
    assert!(drain(&rx).is_empty());
}

#[test]
fn display_notification_while_supported_pushes_one_notification() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=1;hello\x1b\\");
    assert_eq!(state.window_commands.len(), 1);
    match &state.window_commands[0] {
        WindowManipulation::Notification99(data) => {
            assert_eq!(data.title.as_deref(), Some("hello"));
        }
        other => panic!("expected Notification99, got: {other:?}"),
    }
}

#[test]
fn close_while_unsupported_pushes_no_window_command() {
    let (mut state, _rx) = make_state(HostCapabilities::default());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=close;\x1b\\");
    assert!(state.window_commands.is_empty());
}

#[test]
fn close_while_supported_pushes_one_close_control() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=close;\x1b\\");
    assert_eq!(state.window_commands.len(), 1);
    match &state.window_commands[0] {
        WindowManipulation::Osc99Control { id, kind } => {
            assert_eq!(id.as_deref(), Some("abc"));
            assert_eq!(*kind, Osc99ControlKind::Close);
        }
        other => panic!("expected Osc99Control, got: {other:?}"),
    }
}

// ── p=? edge cases ───────────────────────────────────────────────────────────

#[test]
fn query_with_empty_id_replies_with_id_zero() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=:p=?;\x1b\\");
    let reply = drain(&rx);
    assert!(
        reply.starts_with(b"\x1b]99;i=0:p=?;"),
        "unexpected reply: {reply:?}"
    );
}

#[test]
fn query_for_a_tombstoned_id_is_answered_while_supported() {
    let (mut state, rx) = make_state(full_caps());
    // A non-final title chunk with invalid base64 drops the transfer and
    // leaves a tombstone for id `1`.
    state.handle_incoming_data(b"\x1b]99;i=1:d=0:p=title:e=1;@@@@\x1b\\");
    assert!(drain(&rx).is_empty());
    assert!(state.window_commands.is_empty());

    state.handle_incoming_data(b"\x1b]99;i=1:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=1:p=?;a=report:c=1:{TAIL}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
    assert!(state.window_commands.is_empty());
}

// ── Gate before reassembly; pending cleared on disable (130 review N5) ──────

/// The title of the single `Notification99` command in `state`, if there is
/// exactly one.
fn only_notification_title(state: &TerminalState) -> Option<String> {
    let mut titles = state.window_commands.iter().filter_map(|c| match c {
        WindowManipulation::Notification99(data) => Some(data.title.clone()),
        _ => None,
    });
    let first = titles.next()?;
    assert!(titles.next().is_none(), "expected exactly one notification");
    first
}

#[test]
fn chunk_received_while_unsupported_is_never_accumulated() {
    let (mut state, _rx) = make_state(HostCapabilities::default());
    state.handle_incoming_data(b"\x1b]99;i=1:d=0;aaa\x1b\\");
    assert!(state.window_commands.is_empty());

    state.handler.set_host_capabilities(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=1;bbb\x1b\\");

    assert_eq!(
        only_notification_title(&state).as_deref(),
        Some("bbb"),
        "the chunk dropped while unsupported must not be spliced in"
    );
}

#[test]
fn pending_transfer_is_discarded_when_support_is_disabled() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=2:d=0;aaa\x1b\\");
    assert!(state.window_commands.is_empty());

    state
        .handler
        .set_host_capabilities(HostCapabilities::default());
    state.handler.set_host_capabilities(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=2;bbb\x1b\\");

    assert_eq!(
        only_notification_title(&state).as_deref(),
        Some("bbb"),
        "disabling OSC 99 must discard the in-flight accumulator"
    );
}
