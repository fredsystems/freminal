// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! End-to-end tests for the OSC 99 `p=?` capability query and the `p=alive`
//! gating, answered on the PTY thread from the host capabilities (Task 130.5).
//!
//! Each test feeds raw bytes through `TerminalState::handle_incoming_data` and
//! inspects the bytes written to the PTY and the window commands produced.
//!
//! # What these tests do and do not pin
//!
//! The capability value the GUI really sends lives in the `freminal` binary
//! crate (`gui::host_capabilities`), which an integration test of this crate
//! cannot reach, and the reply builder is private to the handler. So each test
//! here supplies its own [`Osc99Features`] and compares against an explicit
//! expected reply string. They pin the handler's mapping from features to
//! reply (key order, key omission, value lists), one test per distinct feature
//! set; they do **not** notice a change in what the production host
//! advertises. That is covered where the value is built, by the tests in
//! `freminal`'s `gui::host_capabilities`. The expected strings are literal on
//! purpose: a helper that assembled them would repeat the production logic
//! and could not catch a regression in it.

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::osc99_notification::Osc99ControlKind;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::host_capabilities::{
    HostCapabilities, Osc99ActivationReport, Osc99AutoExpiry, Osc99BodySupport, Osc99ButtonSupport,
    Osc99CloseEvents, Osc99Features, Osc99FocusSupport, Osc99IconSupport, Osc99SoundNames,
    Osc99Support, Osc99UrgencySupport,
};
use freminal_common::pty_write::PtyWrite;
use freminal_terminal_emulator::state::internal::TerminalState;

/// The reply for [`toast_like`]: every optional feature but icons.
const TOAST_REPLY: &str = "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent";

/// The features of the in-window toast leg, which is what a host without a
/// desktop-notification daemon advertises: activation and close reported,
/// focus, expiry, urgency, body and buttons, but no icons and only the
/// well-known sound names. Mirrors `toast_leg()` in `freminal`'s
/// `gui::host_capabilities`, kept in step by hand because that crate is not
/// reachable from here.
const fn toast_like() -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::Reported,
        close_events: Osc99CloseEvents::Reported,
        activation_focus: Osc99FocusSupport::Supported,
        auto_expiry: Osc99AutoExpiry::Supported,
        urgency: Osc99UrgencySupport::Supported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Supported,
        buttons: Osc99ButtonSupport::Supported,
        icons: Osc99IconSupport::Unsupported,
    }
}

const fn caps(features: Osc99Features) -> HostCapabilities {
    HostCapabilities {
        osc99: Osc99Support::Supported(features),
    }
}

const fn full_caps() -> HostCapabilities {
    caps(toast_like())
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

/// Send `p=?` for id `abc` to a host with `features` and return the exact
/// bytes written back, asserting that no window command was produced.
fn query_reply_bytes(features: Osc99Features) -> Vec<u8> {
    let (mut state, rx) = make_state(caps(features));
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?;\x1b\\");
    assert!(state.window_commands.is_empty());
    drain(&rx)
}

/// The full wire form of a reply whose capability list is `capabilities`.
fn framed(capabilities: &str) -> Vec<u8> {
    format!("\x1b]99;i=abc:p=?;{capabilities}\x1b\\").into_bytes()
}

#[test]
fn query_with_toast_like_features_replies_with_exact_bytes_and_no_command() {
    assert_eq!(query_reply_bytes(toast_like()), framed(TOAST_REPLY));
}

#[test]
fn query_with_activation_not_reported_keeps_focus_in_the_a_key() {
    let features = Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        ..toast_like()
    };
    assert_eq!(
        query_reply_bytes(features),
        framed(
            "a=focus:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent"
        )
    );
}

#[test]
fn query_with_close_not_reported_omits_c_key() {
    let features = Osc99Features {
        close_events: Osc99CloseEvents::NotReported,
        ..toast_like()
    };
    assert_eq!(
        query_reply_bytes(features),
        framed(
            "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:w=1:s=system,silent"
        )
    );
}

#[test]
fn query_with_no_supported_action_starts_with_occasion_key() {
    let features = Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        activation_focus: Osc99FocusSupport::Unsupported,
        ..toast_like()
    };
    assert_eq!(
        query_reply_bytes(features),
        framed(
            "o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent"
        )
    );
}

#[test]
fn query_with_system_leg_features_adds_icons_and_standard_sounds() {
    let features = Osc99Features {
        sound_names: Osc99SoundNames::Standard,
        icons: Osc99IconSupport::Supported,
        ..toast_like()
    };
    assert_eq!(
        query_reply_bytes(features),
        framed(
            "a=focus,report:o=always,unfocused,invisible:u=0,1,2:p=title,body,?,close,icon,alive,buttons:c=1:w=1:s=system,silent,error,info,question,warn,warning"
        )
    );
}

#[test]
fn query_with_only_the_always_present_features_is_minimal() {
    let features = Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        close_events: Osc99CloseEvents::NotReported,
        activation_focus: Osc99FocusSupport::Unsupported,
        auto_expiry: Osc99AutoExpiry::Unsupported,
        urgency: Osc99UrgencySupport::Unsupported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Unsupported,
        buttons: Osc99ButtonSupport::Unsupported,
        icons: Osc99IconSupport::Unsupported,
    };
    assert_eq!(
        query_reply_bytes(features),
        framed("o=always,unfocused,invisible:p=title,?,close,alive:s=system,silent")
    );
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
            assert_eq!(id, "abc");
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
            assert_eq!(data.title, "hello");
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
            assert_eq!(id, "abc");
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
    let expected = format!("\x1b]99;i=1:p=?;{TOAST_REPLY}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
    assert!(state.window_commands.is_empty());
}

#[test]
fn query_for_a_tombstoned_id_leaves_the_tombstone_in_place() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=1:d=0:p=title:e=1;@@@@\x1b\\");
    state.handle_incoming_data(b"\x1b]99;i=1:p=?;\x1b\\");
    assert!(!drain(&rx).is_empty());

    // The query did not revive the id: the tail of the dropped transfer is
    // still discarded.
    state.handle_incoming_data(b"\x1b]99;i=1;tail\x1b\\");
    assert!(state.window_commands.is_empty());
}

#[test]
fn query_with_d0_is_answered_at_once() {
    let (mut state, rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=abc:p=?:d=0;\x1b\\");
    let expected = format!("\x1b]99;i=abc:p=?;{TOAST_REPLY}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
    assert!(state.window_commands.is_empty());
}

#[test]
fn query_on_a_saturated_pending_map_is_answered() {
    let (mut state, rx) = make_state(full_caps());
    for i in 0..128 {
        let seq = format!("\x1b]99;i=id-{i}:d=0;x\x1b\\");
        state.handle_incoming_data(seq.as_bytes());
    }
    assert!(drain(&rx).is_empty());

    state.handle_incoming_data(b"\x1b]99;i=q:p=?;\x1b\\");
    let expected = format!("\x1b]99;i=q:p=?;{TOAST_REPLY}\x1b\\");
    assert_eq!(drain(&rx), expected.into_bytes());
    assert!(state.window_commands.is_empty());
}

#[test]
fn alive_and_close_without_an_id_push_no_window_command() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;p=alive;\x1b\\");
    state.handle_incoming_data(b"\x1b]99;p=close;\x1b\\");
    assert!(state.window_commands.is_empty());
}

#[test]
fn close_with_d0_is_forwarded_at_once() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;i=c:p=close:d=0;\x1b\\");
    assert_eq!(
        state.window_commands,
        [WindowManipulation::Osc99Control {
            id: "c".to_owned(),
            kind: Osc99ControlKind::Close,
        }]
    );
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
    Some(first)
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

#[test]
fn unidentified_transfer_is_discarded_when_support_is_disabled() {
    let (mut state, _rx) = make_state(full_caps());
    state.handle_incoming_data(b"\x1b]99;d=0;aaa\x1b\\");
    assert!(state.window_commands.is_empty());

    state
        .handler
        .set_host_capabilities(HostCapabilities::default());
    state.handler.set_host_capabilities(full_caps());
    state.handle_incoming_data(b"\x1b]99;;bbb\x1b\\");

    assert_eq!(
        only_notification_title(&state).as_deref(),
        Some("bbb"),
        "disabling OSC 99 must discard the unidentified slot too"
    );
}
