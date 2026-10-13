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

use super::state::{CloseTarget, DaemonGate, DispatcherState};
use super::test_rig::*;
use super::{
    EventSink, Osc99DbusEvent, close_instance, close_replaced, refresh_capabilities,
    show_notification,
};

/// A rig, a state whose breaker is open, and a sink with the receiver of what
/// it emits.
struct Fixture {
    rig: Rig,
    state: DispatcherState,
    sink: EventSink,
    events: Receiver<Osc99DbusEvent>,
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

    fn queued(&self) -> Vec<Osc99DbusEvent> {
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
        [Osc99DbusEvent::ShowFailed {
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
        [Osc99DbusEvent::Shown {
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
        [Osc99DbusEvent::ShowFailed {
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
        [Osc99DbusEvent::Closed {
            instance: instance(1)
        }]
    );
    assert_eq!(f.state.close_target(instance(1)), CloseTarget::AlreadyGone);
    f.assert_daemon_untouched();

    // Control: with the breaker closed the daemon is called, and it is the
    // daemon's signal, not the handler, that reports the close.
    f.close_the_breaker();
    f.state.record_shown(instance(2), None, peer_reply(8));
    close_instance(&f.rig.client, &mut f.state, &f.sink, instance(2));
    assert_eq!(f.rig.closes(), [8]);
    assert_eq!(f.queued(), []);
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
        [Osc99DbusEvent::Capabilities(_)]
    ));
    assert_eq!(
        f.rig.calls(),
        [("GetCapabilities".to_owned(), Activation::Suppressed)]
    );
    Ok(())
}
