// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Plain (untracked) notifications through the OSC 99 D-Bus service
//! (Task 149.2).
//!
//! OSC 9, OSC 777, OSC 133 `D` command-finished and the Settings "Test
//! Notification" button show a summary and a body and need nothing back: no
//! actions, no `Shown` / `Closed` / `Activated` events, no registry entry. They
//! are carried as [`DbusCommand::ShowPlain`] so they reuse the dispatcher's
//! connection, circuit breaker, reconnect loop and the daemon's capabilities,
//! instead of opening a connection of their own per notification.
//!
//! The GUI hands one over with a [`PlainNotifySender`]. A notification that
//! cannot be shown (the daemon is unresponsive, the service is reconnecting,
//! the queue is full, the call fails) is dropped: a desktop notification is
//! best effort, and nobody is waiting for an answer.

use std::time::Instant;

use crossbeam_channel::{Sender, TrySendError};
use freminal_common::buffer_states::osc99_notification::NotificationUrgency;
use zbus::Connection;

use super::state::{DaemonGate, DispatcherState};
use super::wire::notify;
use super::{COMMAND_QUEUE_CAPACITY, DbusCommand, Osc99DbusHandle};

/// A notification that has a summary, a body and an urgency, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainNotification {
    /// The notification's title. Raw: the dispatcher escapes it as the
    /// daemon's capabilities call for.
    pub summary: String,
    /// The notification's text. Raw, like the summary.
    pub body: String,
    /// The urgency hint: `Critical` persists on most daemons and pops as a
    /// banner.
    pub urgency: NotificationUrgency,
}

/// The GUI's way to show a [`PlainNotification`]: a handle on the service's
/// command queue that can be cloned and kept, and that never blocks.
#[derive(Debug, Clone)]
pub struct PlainNotifySender {
    commands: Sender<DbusCommand>,
}

impl PlainNotifySender {
    /// Queue `notification` for the dispatcher without blocking. One that does
    /// not fit, or that has no dispatcher to go to, is dropped with a debug
    /// log: a program that emits OSC 9 in a loop must not grow the queue.
    pub fn send(&self, notification: PlainNotification) {
        let command = DbusCommand::ShowPlain {
            notification: Box::new(notification),
        };
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::debug!(
                    "desktop notification service queue is full ({COMMAND_QUEUE_CAPACITY}); \
                     notification dropped"
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::debug!("desktop notification service is gone; notification dropped");
            }
        }
    }
}

impl Osc99DbusHandle {
    /// A sender for plain notifications on this service's queue.
    #[must_use]
    pub fn plain_sender(&self) -> PlainNotifySender {
        PlainNotifySender {
            commands: self.commands.clone(),
        }
    }
}

/// Carry out a `ShowPlain`: call `Notify` and note how the daemon fared.
///
/// While the daemon is in its cooldown nothing is called and the notification
/// is dropped. Nothing is recorded and no event is emitted, whatever the
/// outcome: the notification is not tracked, so the daemon's later signals for
/// it are ignored as signals for an unknown id.
pub(super) fn show_plain(
    connection: &Connection,
    state: &mut DispatcherState,
    notification: &PlainNotification,
) {
    if state.daemon_gate(Instant::now()) == DaemonGate::Skip {
        tracing::debug!("the notification daemon is unresponsive; dropping a notification");
        return;
    }
    let args = state.plain_notify_args(notification);
    let outcome = notify(connection, &args);
    state.note_call(&outcome, Instant::now());
    if let Err(e) = outcome {
        tracing::warn!("failed to show desktop notification: {e}");
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use freminal_common::buffer_states::osc::OscNotifySource;
    use freminal_common::buffer_states::window_manipulation::NotificationKind;
    use freminal_common::config::{NotificationRouting, NotificationsConfig};

    use super::super::test_rig::*;
    use super::super::{Osc99DbusEvent, start};
    use super::*;
    use crate::gui::notifications::{NotificationRequest, NotificationRouter, SystemNotifySink};
    use crate::gui::toast::ToastStack;

    fn plain(summary: &str, body: &str, urgency: NotificationUrgency) -> PlainNotification {
        PlainNotification {
            summary: summary.to_owned(),
            body: body.to_owned(),
            urgency,
        }
    }

    /// Show `notification` through a started service and wait until the
    /// dispatcher has handled it: a command that was queued earlier is handled
    /// earlier, so the sentinel's event proves the `ShowPlain` is done, and
    /// that nothing else was emitted.
    fn show_and_settle(
        handle: &mut Osc99DbusHandle,
        notification: PlainNotification,
    ) -> TestResult {
        handle.plain_sender().send(notification);
        assert_no_queued_events(handle)
    }

    /// The `<` / `&` text that must be escaped for a daemon that parses markup.
    const MARKUP: &str = "a<b>&c";
    const ESCAPED_MARKUP: &str = "a<\u{200c}b>&\u{200c}c";

    #[test]
    fn a_plain_notification_is_one_notify_call_with_the_legacy_content() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "actions", "body-markup"])?;

        show_and_settle(
            &mut handle,
            plain("Build", "finished", NotificationUrgency::Normal),
        )?;

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1, "exactly one Notify call");
        let notify = &recorded[0];
        assert_eq!(notify.app_name, "freminal");
        assert_eq!(notify.app_icon, "freminal");
        assert_eq!(notify.replaces_id, 0);
        assert_eq!(notify.summary, "Build");
        assert_eq!(notify.body, "finished");
        assert_eq!(
            notify.actions,
            Vec::<String>::new(),
            "no actions, although the daemon advertises them"
        );
        assert_eq!(notify.expire_timeout, -1);
        assert_eq!(notify.urgency, Some(1));
        assert_eq!(notify.desktop_entry.as_deref(), Some("freminal"));
        assert_eq!(notify.category, None);
        assert_eq!(notify.sound_name, None);
        assert_eq!(
            notify.hint_keys,
            strings(&["desktop-entry", "image-data", "urgency"]),
            "no category, no sound hint, no suppress-sound"
        );
        let (width, height, rowstride, has_alpha, bits, channels, len) = notify
            .image
            .ok_or("the embedded freminal icon is sent as image-data")?;
        assert!(0 < width && width <= 256, "width {width}");
        assert!(0 < height && height <= 256, "height {height}");
        assert_eq!(rowstride, width * 4);
        assert!(has_alpha);
        assert_eq!((bits, channels), (8, 4));
        assert_eq!(i32::try_from(len)?, width * height * 4);
        Ok(())
    }

    #[test]
    fn the_urgency_hint_follows_the_notification() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        show_and_settle(
            &mut handle,
            plain("Failed", "exit 1", NotificationUrgency::Critical),
        )?;
        show_and_settle(
            &mut handle,
            plain("Done", "exit 0", NotificationUrgency::Normal),
        )?;

        let urgencies: Vec<_> = rig.recorded().iter().map(|notify| notify.urgency).collect();
        assert_eq!(urgencies, [Some(2), Some(1)]);
        Ok(())
    }

    #[test]
    fn summary_and_body_are_escaped_only_for_a_daemon_with_body_markup() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "body-markup"])?;
        show_and_settle(
            &mut handle,
            plain(MARKUP, MARKUP, NotificationUrgency::Normal),
        )?;
        let escaped = &rig.recorded()[0];
        assert_eq!(escaped.summary, ESCAPED_MARKUP);
        assert_eq!(escaped.body, ESCAPED_MARKUP);
        Ok(())
    }

    #[test]
    fn summary_and_body_are_raw_for_a_daemon_without_body_markup() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;
        show_and_settle(
            &mut handle,
            plain(MARKUP, MARKUP, NotificationUrgency::Normal),
        )?;
        let raw = &rig.recorded()[0];
        assert_eq!(raw.summary, MARKUP);
        assert_eq!(raw.body, MARKUP);
        Ok(())
    }

    #[test]
    fn a_plain_notification_produces_no_event_and_no_tracking() -> TestResult {
        let (rig, mut handle, woken) = started(&["body", "actions"])?;

        // `show_and_settle` fails on any event queued before its sentinel.
        show_and_settle(
            &mut handle,
            plain("Title", "Body", NotificationUrgency::Normal),
        )?;
        assert_eq!(rig.recorded().len(), 1);

        // Untracked: the daemon's signals for the id it assigned are ignored.
        let dbus_id = rig.recorded()[0].assigned_id;
        rig.emit_action(dbus_id, "default")?;
        rig.emit_closed(dbus_id, 1)?;
        show_and_settle(
            &mut handle,
            plain("Next", "Body", NotificationUrgency::Normal),
        )?;
        assert_eq!(rig.closes(), Vec::<u32>::new(), "nothing was closed");

        // The only wakes are the Capabilities event and the two sentinels.
        for _ in 0..3 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        assert!(woken.try_recv().is_err(), "a plain show wakes nobody");
        Ok(())
    }

    #[test]
    fn a_plain_notification_the_daemon_refuses_is_dropped_quietly() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        show_and_settle(
            &mut handle,
            plain(REJECTED_SUMMARY, "x", NotificationUrgency::Normal),
        )?;

        // The refusal did not stop the service: the next one is shown.
        show_and_settle(
            &mut handle,
            plain("After", "x", NotificationUrgency::Normal),
        )?;
        assert_eq!(rig.recorded().len(), 1);
        assert_eq!(rig.recorded()[0].summary, "After");
        Ok(())
    }

    #[test]
    fn a_plain_notification_while_the_service_is_unavailable_is_dropped_and_unanswered()
    -> TestResult {
        let (waker, woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        // Queued ahead of the sentinel, so the dispatcher has already
        // dropped it when the sentinel's `Closed` comes.
        handle
            .plain_sender()
            .send(plain("Title", "Body", NotificationUrgency::Normal));
        assert_no_queued_events(&mut handle)?;

        // `Unavailable` and the sentinel's `Closed`: nothing for the plain one.
        for _ in 0..2 {
            woken.recv_timeout(EVENT_TIMEOUT)?;
        }
        assert!(woken.try_recv().is_err());
        Ok(())
    }

    #[test]
    fn a_plain_notification_with_the_breaker_open_does_not_call_notify() -> TestResult {
        let rig = rig(&["body", "actions"])?;
        let mut state = DispatcherState::default();
        state.note_call::<()>(&Err(timeout_error()), Instant::now());
        assert_eq!(state.daemon_gate(Instant::now()), DaemonGate::Skip);
        let notification = plain("Title", "Body", NotificationUrgency::Normal);

        show_plain(&rig.client, &mut state, &notification);

        assert_eq!(rig.calls(), [], "the daemon was not called");
        assert_eq!(rig.recorded(), []);

        // Control: with the breaker closed the same call reaches the daemon.
        state.note_call(&Ok::<(), zbus::Error>(()), Instant::now());
        show_plain(&rig.client, &mut state, &notification);
        assert_eq!(rig.recorded().len(), 1);
        Ok(())
    }

    // ── End to end: the router, through the sink, to the fake daemon ────

    fn notifications(routing: NotificationRouting) -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            routing_osc_text: routing,
            routing_error: routing,
            routing_command_finished: routing,
            ..NotificationsConfig::default()
        }
    }

    #[test]
    fn an_osc_9_request_routed_to_system_is_one_escaped_notify_with_normal_urgency() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body", "body-markup"])?;
        let request = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: Some("a<b>".to_owned()),
            body: "x & y".to_owned(),
        };
        let mut toasts = ToastStack::default();

        NotificationRouter::route(
            &request,
            &notifications(NotificationRouting::System),
            false,
            &mut toasts,
            &SystemNotifySink::through(handle.plain_sender()),
        );
        assert_no_queued_events(&mut handle)?;

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1, "exactly one Notify call");
        assert_eq!(recorded[0].summary, "a<\u{200c}b>");
        assert_eq!(recorded[0].body, "x &\u{200c} y");
        assert_eq!(recorded[0].urgency, Some(1));
        assert_eq!(toasts.len(), 0, "System routing pushes no toast");
        Ok(())
    }

    #[test]
    fn an_error_request_routed_through_its_category_has_critical_urgency() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;
        let request = NotificationRequest {
            kind: NotificationKind::Error,
            source: None,
            title: None,
            body: "exit 2".to_owned(),
        };
        let config = NotificationsConfig {
            routing_error: NotificationRouting::System,
            routing_osc_text: NotificationRouting::Disabled,
            ..notifications(NotificationRouting::Disabled)
        };

        NotificationRouter::route(
            &request,
            &config,
            false,
            &mut ToastStack::default(),
            &SystemNotifySink::through(handle.plain_sender()),
        );
        assert_no_queued_events(&mut handle)?;

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].summary, "Error");
        assert_eq!(recorded[0].body, "exit 2");
        assert_eq!(recorded[0].urgency, Some(2));
        Ok(())
    }

    #[test]
    fn the_test_button_shows_a_notification_even_with_the_master_switch_off() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;
        let config = NotificationsConfig {
            enabled: false,
            ..notifications(NotificationRouting::System)
        };
        let sink = SystemNotifySink::through(handle.plain_sender());
        let request = NotificationRequest::sample();

        // `route` honours the master switch: nothing is shown.
        NotificationRouter::route(&request, &config, true, &mut ToastStack::default(), &sink);
        assert_no_queued_events(&mut handle)?;
        assert_eq!(rig.recorded(), []);

        NotificationRouter::route_test(&request, &config, true, &mut ToastStack::default(), &sink);
        assert_no_queued_events(&mut handle)?;

        let recorded = rig.recorded();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].summary, "Freminal");
        assert_eq!(recorded[0].body, "Test notification");
        assert_eq!(recorded[0].urgency, Some(1));
        Ok(())
    }

    #[test]
    fn toast_only_routing_never_reaches_the_daemon() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;
        let request = NotificationRequest {
            kind: NotificationKind::OscText,
            source: Some(OscNotifySource::Osc9),
            title: Some("Hello".to_owned()),
            body: "world".to_owned(),
        };
        let mut toasts = ToastStack::default();

        NotificationRouter::route(
            &request,
            &notifications(NotificationRouting::Toast),
            false,
            &mut toasts,
            &SystemNotifySink::through(handle.plain_sender()),
        );
        assert_no_queued_events(&mut handle)?;

        assert_eq!(toasts.len(), 1, "the toast leg ran");
        assert_eq!(rig.recorded(), [], "the system leg did not");
        Ok(())
    }

    #[test]
    fn sending_to_a_full_queue_or_a_gone_dispatcher_does_not_block_or_panic() {
        let (full_tx, _full_rx) = crossbeam_channel::bounded(COMMAND_QUEUE_CAPACITY);
        let full = PlainNotifySender { commands: full_tx };
        // One more than fits: the last is dropped, without blocking.
        for _ in 0..=COMMAND_QUEUE_CAPACITY {
            full.send(plain("Title", "Body", NotificationUrgency::Normal));
        }

        let (dead_tx, dead_rx) = crossbeam_channel::unbounded();
        drop(dead_rx);
        let dead = PlainNotifySender { commands: dead_tx };
        dead.send(plain("Title", "Body", NotificationUrgency::Normal));
    }
}
