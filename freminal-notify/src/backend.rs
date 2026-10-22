// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The interface types between the callers and the tracked system backend:
//! the instance id, the dispatch result and the backend trait, with the no-op
//! and recording implementations.

use freminal_common::buffer_states::osc99_notification::Notification99Data;

/// The caller's identity for one displayed notification.
///
/// Unlike the application's `i=` id (which may be absent, reused, or shared
/// by different panes), an instance id is unique for the process lifetime and
/// is minted by the caller. The toast leg uses the raw value as its toast
/// key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NotificationInstanceId(u64);

impl NotificationInstanceId {
    /// The instance id with the given raw value.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw value of the instance id.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Whether a backend accepted a show or close request.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemDispatch {
    /// The request was queued; its outcome arrives later as a system event
    /// (see `SystemNotifyEvent` on Linux/BSD). Only the Linux/BSD service (and
    /// the test backend) ever queues a request.
    Queued,
    /// The request could not even be queued, so no event will ever follow.
    /// The caller must end the instance's system leg itself.
    Failed,
}

/// How an OSC 99 notification reaches the operating system on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLeg {
    /// Through the [`TrackedBackend`]: freminal observes the notification's
    /// activation and close (Linux/BSD).
    Tracked,
    /// Through the [`UntrackedBackend`](super::UntrackedBackend): the
    /// notification is shown and forgotten (macOS and Windows).
    Untracked,
}

/// The system leg of the platform freminal is running on.
///
/// Defined with `cfg!` rather than `#[cfg]`, so both variants are constructed
/// on every platform.
pub const SYSTEM_LEG: SystemLeg = if cfg!(all(unix, not(target_os = "macos"))) {
    SystemLeg::Tracked
} else {
    SystemLeg::Untracked
};

/// The Linux/BSD desktop-notification service as the caller sees it.
///
/// A trait so the caller and its tests need not reach a session bus, and so
/// every platform compiles the same caller.
pub trait TrackedBackend {
    /// Display `notification` for `instance`, updating `replaces`'
    /// notification in place when that is set.
    fn show(
        &mut self,
        instance: NotificationInstanceId,
        replaces: Option<NotificationInstanceId>,
        notification: &Notification99Data,
    ) -> SystemDispatch;

    /// Close `instance`'s desktop notification. A queued close ends in a
    /// `Closed` event, whether the daemon closed it or it was already gone.
    fn close(&mut self, instance: NotificationInstanceId) -> SystemDispatch;
}

/// The backend on a build or moment with no system-notification service.
///
/// It displays nothing and reports every show and close as
/// [`SystemDispatch::Failed`], so the caller ends the leg instead of
/// waiting for an event that cannot come.
///
/// macOS and Windows never route to the caller's tracked leg, so for them it
/// is never called. On Linux/BSD it stands in while the service's handle is
/// borrowed elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTrackedBackend;

impl TrackedBackend for NoTrackedBackend {
    fn show(
        &mut self,
        instance: NotificationInstanceId,
        _replaces: Option<NotificationInstanceId>,
        _notification: &Notification99Data,
    ) -> SystemDispatch {
        tracing::trace!(
            instance = instance.get(),
            "OSC 99: no system notification service; show failed"
        );
        SystemDispatch::Failed
    }

    fn close(&mut self, instance: NotificationInstanceId) -> SystemDispatch {
        tracing::trace!(
            instance = instance.get(),
            "OSC 99: no system notification service; close failed"
        );
        SystemDispatch::Failed
    }
}

/// One [`TrackedBackend::show`] call, as [`RecordingTrackedBackend`] saw it.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedShow {
    /// The instance shown.
    pub instance: NotificationInstanceId,
    /// The instance it replaced, if any.
    pub replaces: Option<NotificationInstanceId>,
    /// The notification handed to the backend.
    pub notification: Notification99Data,
}

/// How [`RecordingTrackedBackend`] answers a show.
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RecordedShowAnswer {
    /// Report [`SystemDispatch::Queued`].
    #[default]
    Queue,
    /// Report [`SystemDispatch::Failed`], after recording the show.
    Fail,
}

/// A backend that records every show and close, for tests. It reports
/// [`SystemDispatch::Queued`] for both unless `show_answer` is
/// [`RecordedShowAnswer::Fail`] (which fails shows only).
#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Default)]
pub struct RecordingTrackedBackend {
    /// Every show, in call order.
    pub shown: Vec<RecordedShow>,
    /// Every closed instance, in call order.
    pub closed: Vec<NotificationInstanceId>,
    /// How every show is answered.
    pub show_answer: RecordedShowAnswer,
}

#[cfg(any(test, feature = "test-utils"))]
impl TrackedBackend for RecordingTrackedBackend {
    fn show(
        &mut self,
        instance: NotificationInstanceId,
        replaces: Option<NotificationInstanceId>,
        notification: &Notification99Data,
    ) -> SystemDispatch {
        self.shown.push(RecordedShow {
            instance,
            replaces,
            notification: notification.clone(),
        });
        match self.show_answer {
            RecordedShowAnswer::Queue => SystemDispatch::Queued,
            RecordedShowAnswer::Fail => SystemDispatch::Failed,
        }
    }

    fn close(&mut self, instance: NotificationInstanceId) -> SystemDispatch {
        self.closed.push(instance);
        SystemDispatch::Queued
    }
}
