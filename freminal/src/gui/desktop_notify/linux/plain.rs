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
//! more than [`PLAIN_NOTIFICATION_BUDGET`] are already waiting, the queue is
//! full, the call fails) is dropped: a desktop notification is best effort, and
//! nobody is waiting for an answer.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use crossbeam_channel::{Sender, TrySendError};
use freminal_common::buffer_states::osc99_notification::NotificationUrgency;
use zbus::Connection;

use super::state::{DaemonGate, DispatcherState};
use super::wire::notify;
use super::{COMMAND_QUEUE_CAPACITY, DbusCommand, Osc99DbusHandle};

/// How many plain notifications may be waiting for the dispatcher (or being
/// shown by it) at once.
///
/// Plain notifications share the command queue with OSC 99 `Show` / `Close`.
/// Sixteen is the size of the worker queue the macOS and Windows backends
/// still use, which is what OSC 9 / 777 / 133 `D` had here before they moved
/// onto this service: a program looping on OSC 9 cannot build a backlog behind
/// a slow daemon, and cannot take the queue slots OSC 99 needs.
pub(super) const PLAIN_NOTIFICATION_BUDGET: usize = 16;

/// The count of plain notifications in flight, shared by every
/// [`PlainNotifySender`] cloned from one handle.
#[derive(Debug, Clone, Default)]
pub(super) struct PlainBudget {
    in_flight: Arc<AtomicUsize>,
}

impl PlainBudget {
    /// Claim one of the [`PLAIN_NOTIFICATION_BUDGET`] slots, or `None` when
    /// all are taken. The count never exceeds the budget, even when several
    /// threads claim at once.
    fn reserve(&self) -> Option<PlainBudgetSlot> {
        self.in_flight
            .try_update(Ordering::AcqRel, Ordering::Acquire, |taken| {
                (taken < PLAIN_NOTIFICATION_BUDGET).then_some(taken.saturating_add(1))
            })
            .ok()
            .map(|_| PlainBudgetSlot {
                in_flight: Arc::clone(&self.in_flight),
            })
    }

    /// How many slots are taken.
    #[cfg(test)]
    pub(super) fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// One claimed slot of the [`PlainBudget`]. It travels inside the
/// [`DbusCommand::ShowPlain`] it was claimed for and is released when that
/// command is dropped: handled, answered unserved, discarded with the channel,
/// or handed back by a failed send.
#[derive(Debug)]
pub(super) struct PlainBudgetSlot {
    in_flight: Arc<AtomicUsize>,
}

impl Drop for PlainBudgetSlot {
    fn drop(&mut self) {
        self.in_flight.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Why a plain notification was dropped without being shown, for the one
/// warning each cause gets per process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlainDrop {
    /// The service has no connection (see `reconnect`).
    ServiceUnavailable,
    /// [`PLAIN_NOTIFICATION_BUDGET`] notifications were already in flight.
    OverBudget,
}

/// How loudly a dropped plain notification is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropLogLevel {
    /// The first drop for a cause in this process.
    Warn,
    /// Every later drop for that cause.
    Debug,
}

/// The level for the next drop counted by `counter`: [`DropLogLevel::Warn`]
/// for the first, [`DropLogLevel::Debug`] for every one after it.
fn drop_log_level(counter: &AtomicU64) -> DropLogLevel {
    if counter.fetch_add(1, Ordering::Relaxed) == 0 {
        DropLogLevel::Warn
    } else {
        DropLogLevel::Debug
    }
}

/// Log a dropped plain notification: a warning for the first one dropped for
/// `reason` in this process, debug afterwards, so a flood cannot also flood the
/// log.
///
/// The warning is once per process per cause, deliberately: the counters are
/// never reset, so a flood that follows a recovery (the service reconnects,
/// then drops again) is logged at debug level and does not flood the log a
/// second time.
pub(super) fn log_dropped_plain(reason: PlainDrop) {
    static UNAVAILABLE: AtomicU64 = AtomicU64::new(0);
    static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);
    let (counter, why) = match reason {
        PlainDrop::ServiceUnavailable => (&UNAVAILABLE, "the notification service is unavailable"),
        PlainDrop::OverBudget => (
            &OVER_BUDGET,
            "too many notifications are waiting for the notification service",
        ),
    };
    match drop_log_level(counter) {
        DropLogLevel::Warn => tracing::warn!(
            "desktop notifications: {why}; dropping notifications \
             (further drops are logged at debug level)"
        ),
        DropLogLevel::Debug => {
            tracing::debug!("desktop notifications: {why}; notification dropped");
        }
    }
}

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
/// command queue that can be cloned and kept, and that never blocks. Every
/// clone made from one [`Osc99DbusHandle`] draws on the same
/// [`PLAIN_NOTIFICATION_BUDGET`].
#[derive(Debug, Clone)]
pub struct PlainNotifySender {
    commands: Sender<DbusCommand>,
    budget: PlainBudget,
}

impl PlainNotifySender {
    /// Queue `notification` for the dispatcher without blocking. One that is
    /// over the [`PLAIN_NOTIFICATION_BUDGET`], does not fit the queue, or has
    /// no dispatcher to go to is dropped: a program that emits OSC 9 in a loop
    /// must neither grow the queue nor take the slots OSC 99 needs.
    pub fn send(&self, notification: PlainNotification) {
        let Some(slot) = self.budget.reserve() else {
            log_dropped_plain(PlainDrop::OverBudget);
            return;
        };
        let command = DbusCommand::ShowPlain {
            notification: Box::new(notification),
            slot,
        };
        // A command that is not sent comes back inside the error and is
        // dropped here, which releases its slot.
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
            budget: self.plain_budget.clone(),
        }
    }
}

/// Carry out a `ShowPlain`: call `Notify` and note how the daemon fared.
///
/// While the daemon is in its cooldown nothing is called and the notification
/// is dropped. Nothing is recorded and no event is emitted, whatever the
/// outcome: the notification is not tracked, so the daemon's later signals for
/// it are ignored as signals for an unknown id. The one thing a successful call
/// does leave behind is the daemon's identity: with no known owner, the sender
/// of the reply is adopted as it is for an OSC 99 show (see
/// [`DispatcherState::adopt_reply_owner`]), so a service used only by plain
/// notifications still learns the daemon's capabilities.
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
    match outcome {
        Ok(reply) => state.adopt_reply_owner(&reply),
        Err(e) => tracing::warn!("failed to show desktop notification: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crossbeam_channel::Receiver;

    use super::super::super::test_support::test_assets;
    use super::super::super::{
        Osc99DaemonCapabilities, Osc99DaemonSupport, Osc99SystemBackend, Osc99SystemDispatch,
    };
    use super::super::state::OwnerTracking;
    use super::super::test_rig::*;
    use super::super::{Osc99DbusEvent, start};
    use super::*;

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

    /// Markup as a program could emit it (issue #560): tags, an entity start
    /// and a link. The characters the daemon must not see as markup are the
    /// `<` and `&`.
    const MARKUP: &str = r#"<b>x</b> & <a href="http://e">y</a>"#;

    /// The ZWNJ `escape_body_markup` puts after `<` and `&`.
    const ZWNJ: char = '\u{200c}';

    /// Whether every `<` and `&` of `text` is followed by a ZWNJ, and a ZWNJ
    /// follows nothing else.
    fn escapes_every_angle_and_ampersand(text: &str) -> bool {
        let mut chars = text.chars().peekable();
        let mut after_escapable = false;
        while let Some(c) = chars.next() {
            let escapable = matches!(c, '<' | '&');
            if (c == ZWNJ && !after_escapable) || (escapable && chars.peek() != Some(&ZWNJ)) {
                return false;
            }
            after_escapable = escapable;
        }
        true
    }

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
        for text in [&escaped.summary, &escaped.body] {
            assert!(escapes_every_angle_and_ampersand(text), "{text:?}");
            // Four `<` and one `&`: five joiners, and nothing else was added.
            assert_eq!(text.matches(ZWNJ).count(), 5, "{text:?}");
            assert_eq!(text.replace(ZWNJ, ""), MARKUP);
        }
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
        assert!(!raw.summary.contains(ZWNJ));
        assert!(!raw.body.contains(ZWNJ));
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
            test_assets().app_icon_png,
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

    // ── Drop logging ────────────────────────────────────────────────────

    #[test]
    fn only_the_first_drop_for_a_cause_is_a_warning() {
        let counter = AtomicU64::new(0);

        assert_eq!(drop_log_level(&counter), DropLogLevel::Warn);
        for _ in 0..3 {
            assert_eq!(drop_log_level(&counter), DropLogLevel::Debug);
        }
    }

    #[test]
    fn each_cause_counts_its_own_drops() {
        let (unavailable, over_budget) = (AtomicU64::new(0), AtomicU64::new(0));

        assert_eq!(drop_log_level(&unavailable), DropLogLevel::Warn);
        assert_eq!(drop_log_level(&unavailable), DropLogLevel::Debug);
        assert_eq!(drop_log_level(&over_budget), DropLogLevel::Warn);
    }

    // ── The queue and the budget ────────────────────────────────────────

    /// A handle whose dispatcher never drains its queue, as when it is stuck
    /// on a hung daemon: the commands stay in the returned receiver.
    fn stalled_handle() -> (Osc99DbusHandle, Receiver<DbusCommand>) {
        let (commands, queue) = crossbeam_channel::bounded(COMMAND_QUEUE_CAPACITY);
        let (_events_tx, events) = crossbeam_channel::unbounded();
        let handle = Osc99DbusHandle {
            commands,
            events,
            plain_budget: PlainBudget::default(),
        };
        (handle, queue)
    }

    fn send_plain(sender: &PlainNotifySender) {
        sender.send(plain("Title", "Body", NotificationUrgency::Normal));
    }

    #[test]
    fn plain_notifications_are_capped_and_a_full_budget_still_lets_an_osc_99_show_queue() {
        let (mut handle, queue) = stalled_handle();
        let sender = handle.plain_sender();

        for _ in 0..PLAIN_NOTIFICATION_BUDGET {
            send_plain(&sender);
        }
        assert_eq!(queue.len(), PLAIN_NOTIFICATION_BUDGET);
        assert_eq!(handle.plain_budget.in_flight(), PLAIN_NOTIFICATION_BUDGET);

        // One more is dropped, from this sender and from any other made from
        // the same handle: they share the budget.
        send_plain(&sender);
        send_plain(&handle.plain_sender());
        assert_eq!(queue.len(), PLAIN_NOTIFICATION_BUDGET);
        assert_eq!(handle.plain_budget.in_flight(), PLAIN_NOTIFICATION_BUDGET);

        // OSC 99 still gets its queue.
        assert_eq!(
            handle.show(instance(1), None, &notification()),
            Osc99SystemDispatch::Queued
        );
        assert_eq!(handle.close(instance(1)), Osc99SystemDispatch::Queued);
        assert_eq!(queue.len(), PLAIN_NOTIFICATION_BUDGET + 2);
    }

    #[test]
    fn the_budget_is_released_when_the_dispatcher_takes_a_command() {
        let (handle, queue) = stalled_handle();
        let sender = handle.plain_sender();
        for _ in 0..PLAIN_NOTIFICATION_BUDGET {
            send_plain(&sender);
        }
        send_plain(&sender);
        assert_eq!(queue.len(), PLAIN_NOTIFICATION_BUDGET);

        // The dispatcher handles (and so drops) one command: one slot frees.
        drop(queue.try_recv());
        assert_eq!(
            handle.plain_budget.in_flight(),
            PLAIN_NOTIFICATION_BUDGET - 1
        );
        send_plain(&sender);
        assert_eq!(queue.len(), PLAIN_NOTIFICATION_BUDGET);
        assert_eq!(handle.plain_budget.in_flight(), PLAIN_NOTIFICATION_BUDGET);

        // Commands still queued when the channel is dropped release theirs.
        let budget = handle.plain_budget.clone();
        drop((sender, handle, queue));
        assert_eq!(budget.in_flight(), 0);
    }

    #[test]
    fn a_running_dispatcher_releases_the_budget_after_every_notification() -> TestResult {
        let (rig, mut handle, _woken) = started(&["body"])?;

        // One more than the budget, one at a time: each is handled (and its slot
        // released) before the next goes in, so none is dropped.
        let total = PLAIN_NOTIFICATION_BUDGET + 1;
        for n in 0..total {
            show_and_settle(
                &mut handle,
                plain(&format!("n{n}"), "x", NotificationUrgency::Normal),
            )?;
            assert_eq!(handle.plain_budget.in_flight(), 0, "after {n}");
        }

        assert_eq!(rig.recorded().len(), total);
        Ok(())
    }

    #[test]
    fn notifications_dropped_while_the_service_is_unavailable_release_the_budget() -> TestResult {
        let (waker, _woken) = counting_waker();
        let mut handle = start(
            || Err(zbus::Error::Failure("no bus".to_owned())),
            waker,
            Box::new(NoIconLookup),
            test_assets().app_icon_png,
            Duration::from_secs(3600),
        );
        assert_eq!(next_event(&handle)?, Osc99DbusEvent::Unavailable);

        for _ in 0..PLAIN_NOTIFICATION_BUDGET * 2 {
            send_plain(&handle.plain_sender());
            // Handled in order: when the sentinel is answered, so was this.
            assert_no_queued_events(&mut handle)?;
        }

        assert_eq!(handle.plain_budget.in_flight(), 0);
        Ok(())
    }

    #[test]
    fn a_send_to_a_full_queue_is_dropped_without_blocking_and_keeps_no_slot() -> TestResult {
        let (handle, queue) = stalled_handle();
        for n in 0..COMMAND_QUEUE_CAPACITY {
            handle.commands.try_send(DbusCommand::Close {
                instance: instance(u64::try_from(n)?),
            })?;
        }
        assert_eq!(queue.len(), COMMAND_QUEUE_CAPACITY);
        let sender = handle.plain_sender();

        within_deadline(move || {
            for _ in 0..=PLAIN_NOTIFICATION_BUDGET {
                send_plain(&sender);
            }
        })?;

        assert_eq!(queue.len(), COMMAND_QUEUE_CAPACITY, "nothing was added");
        assert_eq!(
            handle.plain_budget.in_flight(),
            0,
            "the refused commands gave their slots back"
        );
        Ok(())
    }

    #[test]
    fn a_send_to_a_gone_dispatcher_returns_and_keeps_no_slot() -> TestResult {
        let (handle, queue) = stalled_handle();
        drop(queue);
        let sender = handle.plain_sender();

        within_deadline(move || {
            for _ in 0..=PLAIN_NOTIFICATION_BUDGET {
                send_plain(&sender);
            }
        })?;

        assert_eq!(handle.plain_budget.in_flight(), 0);
        Ok(())
    }

    // ── Adopting the daemon's identity (149.C17) ────────────────────────

    #[test]
    fn a_plain_notification_adopts_the_reply_sender_as_the_owner_and_refreshes_the_capabilities()
    -> TestResult {
        let rig = bus_rig(
            &["body", "body-markup"],
            NameOwnerAnswer::Broken,
            Some(":1.7"),
        )?;
        let (mut handle, _woken) = spawn_on(&rig, OwnerTracking::Bus);

        // The owner lookup failed, so nobody is asked what the daemon can do.
        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [("GetNameOwner".to_owned(), Activation::Suppressed)]
        );

        // The reply to the first plain `Notify` names the daemon, which is a
        // new owner, so the capabilities are fetched: the same sequence as
        // for an OSC 99 show.
        handle
            .plain_sender()
            .send(plain("Title", "Body", NotificationUrgency::Normal));
        assert_eq!(
            next_event(&handle)?,
            Osc99DbusEvent::Capabilities(Osc99DaemonCapabilities {
                body: Osc99DaemonSupport::Supported,
                actions: Osc99DaemonSupport::Unsupported,
                body_markup: Osc99DaemonSupport::Supported,
                sound: Osc99DaemonSupport::Unsupported,
            })
        );
        assert_no_queued_events(&mut handle)?;
        assert_eq!(
            rig.calls(),
            [
                ("GetNameOwner".to_owned(), Activation::Suppressed),
                ("Notify".to_owned(), Activation::Allowed),
                ("GetCapabilities".to_owned(), Activation::Suppressed),
            ]
        );
        Ok(())
    }
}
