// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Where plain notifications go to be shown on the operating system.
//!
//! OSC 9, OSC 777, OSC 133 `D` command-finished and the Settings "Test
//! Notification" button show a summary and a body and need nothing back. They
//! are a [`PlainNotification`], and a [`PlainSink`] is the one way one reaches
//! the OS, on every platform: Linux and BSD hand it to the D-Bus service, and
//! macOS and Windows hand it to the `freminal-notify` worker.
//!
//! A test can never reach the OS through a sink. The only constructors that
//! do are `PlainSink::through_service`, which needs a running service's
//! sender, and `PlainSink::platform_worker`, which is private to this crate,
//! so no caller (and none of the callers' tests) can reach it; inside the
//! crate only
//! [`DesktopNotifier::plain_sink`](super::DesktopNotifier::plain_sink) calls
//! it. Every other sink drops the notification (`PlainSink::nowhere`) or
//! records it (`PlainSink::recording`).
//!
//! This matters most on macOS: `notify-rust`'s `show()` goes into the
//! Objective-C notification runtime, which throws an uncatchable foreign
//! exception when called from a binary that is not an `.app` bundle, as a
//! `cargo test` binary is. The process aborts, nondeterministically and on
//! the worker thread, during test teardown. Tests must therefore use
//! `PlainSink::nowhere` or `PlainSink::recording`.

#[cfg(any(test, feature = "test-utils"))]
use std::sync::{Arc, Mutex, PoisonError};

use freminal_common::buffer_states::osc99_notification::NotificationUrgency;

#[cfg(all(unix, not(target_os = "macos")))]
use super::linux::PlainNotifySender;
#[cfg(not(all(unix, not(target_os = "macos"))))]
use super::plain_worker;

/// A notification that has a summary, a body and an urgency, and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainNotification {
    /// The notification's title. Raw: the Linux/BSD service escapes it as the
    /// daemon's capabilities call for, and macOS and Windows interpret no
    /// markup.
    pub summary: String,
    /// The notification's text. Raw, like the summary.
    pub body: String,
    /// The urgency hint: `Critical` persists on most Linux/BSD daemons and
    /// pops as a banner. macOS and Windows do not read it: neither platform's
    /// presenter has an urgency.
    pub urgency: NotificationUrgency,
}

/// Where plain (OSC 9 / OSC 777 / command-finished / Settings Test) system
/// notifications go.
///
/// A cloneable value that never blocks the caller. A notification that cannot
/// be shown (no service, a full queue, a failing daemon) is dropped: a desktop
/// notification is best effort, and nobody is waiting for an answer.
#[derive(Debug, Clone)]
pub struct PlainSink {
    route: Route,
}

#[derive(Debug, Clone)]
enum Route {
    /// Nowhere: the notification is dropped with a debug log.
    #[cfg(any(test, feature = "test-utils", all(unix, not(target_os = "macos"))))]
    Nowhere,
    /// The Linux/BSD D-Bus notification service.
    #[cfg(all(unix, not(target_os = "macos")))]
    Service(PlainNotifySender),
    /// The process-wide macOS / Windows worker.
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    Worker,
    /// Test double: every notification is kept for [`PlainRecorder::taken`].
    #[cfg(any(test, feature = "test-utils"))]
    Recorder(Arc<Mutex<Vec<PlainNotification>>>),
}

impl PlainSink {
    /// A sink with nowhere to send a notification: for tests, and for when the
    /// notification service is not available.
    #[cfg(any(test, feature = "test-utils", all(unix, not(target_os = "macos"))))]
    #[must_use]
    pub const fn nowhere() -> Self {
        Self {
            route: Route::Nowhere,
        }
    }

    /// A sink that shows notifications through the running D-Bus service
    /// `sender` belongs to.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[must_use]
    pub const fn through_service(sender: PlainNotifySender) -> Self {
        Self {
            route: Route::Service(sender),
        }
    }

    /// A sink that shows notifications on the process-wide `freminal-notify`
    /// worker (macOS and Windows).
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    #[must_use]
    pub(crate) const fn platform_worker() -> Self {
        Self {
            route: Route::Worker,
        }
    }

    /// A sink that records what it is given instead of showing it, and the
    /// handle to read it back with.
    #[cfg(any(test, feature = "test-utils"))]
    #[must_use]
    pub fn recording() -> (Self, PlainRecorder) {
        let shared = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                route: Route::Recorder(Arc::clone(&shared)),
            },
            PlainRecorder { shared },
        )
    }

    /// Show `notification` on this sink's destination, without blocking.
    pub fn show(&self, notification: PlainNotification) {
        match &self.route {
            #[cfg(any(test, feature = "test-utils", all(unix, not(target_os = "macos"))))]
            Route::Nowhere => {
                tracing::debug!("no desktop notification service to show a notification; dropped");
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Route::Service(sender) => sender.send(notification),
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            Route::Worker => plain_worker::show_plain(notification),
            #[cfg(any(test, feature = "test-utils"))]
            Route::Recorder(shared) => shared
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(notification),
        }
    }
}

/// Where a [`PlainSink`] sends what it is given, for tests that must tell the
/// destinations apart without reaching any of them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// The notification is dropped.
    Nowhere,
    /// The Linux/BSD D-Bus service.
    #[cfg(all(unix, not(target_os = "macos")))]
    Service,
    /// The macOS / Windows worker.
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    Worker,
    /// A [`PlainRecorder`].
    Recorder,
}

#[cfg(test)]
impl PlainSink {
    /// Which destination this sink sends to.
    #[must_use]
    pub fn route_kind(&self) -> RouteKind {
        match &self.route {
            Route::Nowhere => RouteKind::Nowhere,
            #[cfg(all(unix, not(target_os = "macos")))]
            Route::Service(_) => RouteKind::Service,
            #[cfg(not(all(unix, not(target_os = "macos"))))]
            Route::Worker => RouteKind::Worker,
            Route::Recorder(_) => RouteKind::Recorder,
        }
    }
}

/// The read side of [`PlainSink::recording`].
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone)]
pub struct PlainRecorder {
    shared: Arc<Mutex<Vec<PlainNotification>>>,
}

#[cfg(any(test, feature = "test-utils"))]
impl PlainRecorder {
    /// Take every notification recorded since the last call, oldest first.
    #[must_use]
    pub fn taken(&self) -> Vec<PlainNotification> {
        let mut recorded = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(summary: &str, urgency: NotificationUrgency) -> PlainNotification {
        PlainNotification {
            summary: summary.to_owned(),
            body: "body".to_owned(),
            urgency,
        }
    }

    #[test]
    fn a_recording_sink_keeps_what_it_is_given_in_order_and_drains_it() {
        let (sink, recorder) = PlainSink::recording();
        assert_eq!(recorder.taken(), []);

        sink.show(notification("first", NotificationUrgency::Normal));
        sink.show(notification("second", NotificationUrgency::Critical));

        assert_eq!(
            recorder.taken(),
            [
                notification("first", NotificationUrgency::Normal),
                notification("second", NotificationUrgency::Critical),
            ]
        );
        assert_eq!(recorder.taken(), [], "taking drains the recording");
    }

    #[test]
    fn clones_of_a_recording_sink_share_one_recording() {
        let (sink, recorder) = PlainSink::recording();
        let clone = sink.clone();
        sink.show(notification("original", NotificationUrgency::Normal));
        clone.show(notification("cloned", NotificationUrgency::Low));
        assert_eq!(
            recorder.taken(),
            [
                notification("original", NotificationUrgency::Normal),
                notification("cloned", NotificationUrgency::Low),
            ]
        );
    }

    /// A sink with nowhere to go routes nowhere, and showing on it neither
    /// panics nor blocks.
    #[test]
    fn a_sink_with_nowhere_to_go_drops_the_notification() {
        let sink = PlainSink::nowhere();
        assert_eq!(sink.route_kind(), RouteKind::Nowhere);

        sink.show(notification("lost", NotificationUrgency::Normal));
    }
}
