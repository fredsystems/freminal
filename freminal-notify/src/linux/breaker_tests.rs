// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The dispatcher's circuit breaker, seen from its command handlers.
//!
//! `state` unit-tests the breaker's transitions; these tests prove that every
//! handler that calls the daemon honours it: with the breaker open nothing is
//! sent, and the command is answered locally. Each test also runs the same
//! step with the breaker closed, so a handler that never called the daemon in
//! the first place cannot pass. The handlers block until the daemon replies,
//! so what the fake daemon has recorded when one returns is final.

use std::time::Instant;

use crossbeam_channel::Receiver;
use freminal_common::buffer_states::osc99_notification::Notification99Data;

use super::super::TrackedBackend;
use super::state::{CloseTarget, DaemonGate, DispatcherState};
use super::test_helpers::*;
use super::test_rig::*;
use super::{
    EventSink, SystemNotifyEvent, close_instance, close_replaced, refresh_capabilities,
    show_notification,
};

/// A rig, a state whose breaker is open, and a sink with the receiver of what
/// it emits.
struct Fixture {
    rig: Rig,
    state: DispatcherState,
    sink: EventSink,
    events: Receiver<SystemNotifyEvent>,
}

fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let (events_tx, events) = crossbeam_channel::unbounded();
    let mut state = DispatcherState::default();
    state.note_call::<()>(&Err(timeout_error()), Instant::now());
    assert_eq!(state.daemon_gate(Instant::now()), DaemonGate::Skip);
    Ok(Fixture {
        rig: rig(&["body", "actions"])?,
        state,
        sink: EventSink {
            events: events_tx,
            waker: Box::new(|| {}),
        },
        events,
    })
}

impl Fixture {
    /// The daemon answered a call: the breaker closes.
    fn close_the_breaker(&mut self) {
        self.state
            .note_call(&Ok::<(), zbus::Error>(()), Instant::now());
        assert_eq!(self.state.daemon_gate(Instant::now()), DaemonGate::Call);
    }

    fn queued(&self) -> Vec<SystemNotifyEvent> {
        self.events.try_iter().collect()
    }

    /// Nothing reached the fake daemon.
    fn assert_daemon_untouched(&self) {
        assert_eq!(self.rig.calls(), []);
        assert_eq!(self.rig.recorded(), []);
        assert_eq!(self.rig.closes(), Vec::<u32>::new());
    }
}

#[test]
fn a_show_with_the_breaker_open_fails_without_calling_notify() -> TestResult {
    let mut f = fixture()?;

    show_notification(
        &f.rig.client,
        &mut f.state,
        &f.sink,
        instance(1),
        None,
        &notification(),
    );

    assert_eq!(
        f.queued(),
        [SystemNotifyEvent::ShowFailed {
            instance: instance(1)
        }]
    );
    f.assert_daemon_untouched();

    // Control: with the breaker closed the same show reaches the daemon.
    f.close_the_breaker();
    show_notification(
        &f.rig.client,
        &mut f.state,
        &f.sink,
        instance(2),
        None,
        &notification(),
    );
    assert_eq!(
        f.queued(),
        [SystemNotifyEvent::Shown {
            instance: instance(2)
        }]
    );
    assert_eq!(f.rig.recorded().len(), 1);
    Ok(())
}

#[test]
fn a_replacing_show_with_the_breaker_open_fails_and_does_not_close_the_old_one() -> TestResult {
    let mut f = fixture()?;
    f.state.record_shown(instance(1), None, peer_reply(7));

    show_notification(
        &f.rig.client,
        &mut f.state,
        &f.sink,
        instance(2),
        Some(instance(1)),
        &notification(),
    );

    assert_eq!(
        f.queued(),
        [SystemNotifyEvent::ShowFailed {
            instance: instance(2)
        }]
    );
    // The replaced instance is forgotten, but its best-effort close is the
    // call the open breaker withholds.
    assert_eq!(f.state.dbus_id_of(instance(1)), None);
    f.assert_daemon_untouched();
    Ok(())
}

#[test]
fn a_close_with_the_breaker_open_reports_closed_without_calling_the_daemon() -> TestResult {
    let mut f = fixture()?;
    f.state.record_shown(instance(1), None, peer_reply(7));

    close_instance(&f.rig.client, &mut f.state, &f.sink, instance(1));

    assert_eq!(
        f.queued(),
        [SystemNotifyEvent::Closed {
            instance: instance(1)
        }]
    );
    assert_eq!(f.state.close_target(instance(1)), CloseTarget::AlreadyGone);
    f.assert_daemon_untouched();

    // Control: with the breaker closed the daemon is called, and the
    // successful call itself reports the close (138.C23).
    f.close_the_breaker();
    f.state.record_shown(instance(2), None, peer_reply(8));
    close_instance(&f.rig.client, &mut f.state, &f.sink, instance(2));
    assert_eq!(f.rig.closes(), [8]);
    assert_eq!(
        f.queued(),
        [SystemNotifyEvent::Closed {
            instance: instance(2)
        }]
    );
    assert_eq!(f.state.close_target(instance(2)), CloseTarget::AlreadyGone);
    Ok(())
}

#[test]
fn closing_a_replaced_notification_with_the_breaker_open_makes_no_call() -> TestResult {
    let mut f = fixture()?;

    close_replaced(&f.rig.client, &mut f.state, 7);

    f.assert_daemon_untouched();

    // Control.
    f.close_the_breaker();
    close_replaced(&f.rig.client, &mut f.state, 7);
    assert_eq!(f.rig.closes(), [7]);
    Ok(())
}

#[test]
fn a_capability_refresh_with_the_breaker_open_makes_no_call() -> TestResult {
    let mut f = fixture()?;

    refresh_capabilities(&f.rig.client, &mut f.state, &f.sink);

    assert_eq!(f.queued(), []);
    f.assert_daemon_untouched();

    // Control.
    f.close_the_breaker();
    refresh_capabilities(&f.rig.client, &mut f.state, &f.sink);
    assert!(matches!(
        f.queued().as_slice(),
        [SystemNotifyEvent::Capabilities(_)]
    ));
    assert_eq!(
        f.rig.calls(),
        [("GetCapabilities".to_owned(), Activation::Suppressed)]
    );
    Ok(())
}

// ── The real timeout path (149.C2) ──────────────────────────────────────────
//
// The tests above feed the breaker a synthetic timeout error. This one lets a
// daemon really stay silent, against a client whose `method_timeout` is short,
// so the whole chain is covered: zbus's timeout error, `is_method_timeout`,
// `note_call`, and the gate.

#[test]
fn a_hung_notify_fails_the_show_and_the_breaker_skips_the_next_one() -> TestResult {
    let rig = rig_with_method_timeout(&["body", "actions"], SHORT_METHOD_TIMEOUT)?;
    let (rig, mut handle, _woken) = started_with(rig)?;

    let _ = handle.show(
        instance(1),
        None,
        &Notification99Data {
            title: UNANSWERED_SUMMARY.to_owned(),
            ..notification()
        },
    );
    assert_eq!(
        next_event(&handle)?,
        SystemNotifyEvent::ShowFailed {
            instance: instance(1)
        }
    );

    // The daemon would accept this one, but it is in its cooldown.
    let _ = handle.show(instance(2), None, &notification());
    assert_eq!(
        next_event(&handle)?,
        SystemNotifyEvent::ShowFailed {
            instance: instance(2)
        }
    );

    rig.settle()?;
    assert_eq!(
        rig.recorded().len(),
        1,
        "only the unanswered Notify was sent"
    );
    assert_eq!(rig.recorded()[0].summary, UNANSWERED_SUMMARY);
    assert_eq!(
        rig.calls(),
        [
            ("GetCapabilities".to_owned(), Activation::Suppressed),
            ("Notify".to_owned(), Activation::Allowed),
            ("GetCapabilities".to_owned(), Activation::Suppressed),
        ]
    );
    Ok(())
}
