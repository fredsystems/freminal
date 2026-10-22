//! The interface types between the callers and the tracked system backend:
//! the instance id, the dispatch result and the backend trait, with the no-op
//! and recording implementations.

use freminal_common::buffer_states::osc99_notification::Notification99Data;

/// The registry's identity for one displayed notification.
///
/// Unlike the application's `i=` id (which may be absent, reused, or shared
/// by different panes), an instance id is unique for the process lifetime and
/// is minted by the registry's `register`. The toast leg uses the raw value
/// as its toast key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Osc99InstanceId(u64);

impl Osc99InstanceId {
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
pub enum Osc99SystemDispatch {
    /// The request was queued; its outcome arrives later as an
    /// `Osc99SystemLegEvent`. Only the Linux/BSD service (and the test
    /// backend) ever queues a request, so the variant exists only there.
    #[cfg(any(all(unix, not(target_os = "macos")), test))]
    Queued,
    /// The request could not even be queued, so no event will ever follow.
    /// The caller must end the instance's system leg itself.
    Failed,
}

/// The Linux/BSD desktop-notification service as the effect executor sees it.
///
/// A trait so the executor and its tests need not reach a session bus, and so
/// every platform compiles the same executor.
pub trait Osc99SystemBackend {
    /// Display `notification` for `instance`, updating `replaces`'
    /// notification in place when that is set.
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> Osc99SystemDispatch;

    /// Close `instance`'s desktop notification. A queued close ends in a
    /// `Closed` event, whether the daemon closed it or it was already gone.
    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch;
}

/// The backend on a build or moment with no system-notification service: it
/// displays nothing and reports every show and close as
/// [`Osc99SystemDispatch::Failed`], so the registry ends the leg instead of
/// waiting for an event that cannot come.
///
/// macOS and Windows never route to the registry's `Osc99Leg::LinuxSystem`, so for them it
/// is never called. On Linux/BSD it stands in while the service's handle is
/// borrowed elsewhere.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSystemBackend;

impl Osc99SystemBackend for NoSystemBackend {
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        _replaces: Option<Osc99InstanceId>,
        _notification: &Notification99Data,
    ) -> Osc99SystemDispatch {
        tracing::trace!(
            instance = instance.get(),
            "OSC 99: no system notification service; show failed"
        );
        Osc99SystemDispatch::Failed
    }

    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch {
        tracing::trace!(
            instance = instance.get(),
            "OSC 99: no system notification service; close failed"
        );
        Osc99SystemDispatch::Failed
    }
}

/// One [`Osc99SystemBackend::show`] call, as [`RecordingSystemBackend`] saw it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedShow {
    /// The instance shown.
    pub instance: Osc99InstanceId,
    /// The instance it replaced, if any.
    pub replaces: Option<Osc99InstanceId>,
    /// The notification handed to the backend.
    pub notification: Notification99Data,
}

/// A backend that records every show and close, for tests. It reports
/// [`Osc99SystemDispatch::Queued`] for both unless `fail_shows` is set (which
/// fails shows only).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingSystemBackend {
    /// Every show, in call order.
    pub shown: Vec<RecordedShow>,
    /// Every closed instance, in call order.
    pub closed: Vec<Osc99InstanceId>,
    /// Report every show as [`Osc99SystemDispatch::Failed`] (after recording it).
    pub fail_shows: bool,
}

#[cfg(test)]
impl Osc99SystemBackend for RecordingSystemBackend {
    fn show(
        &mut self,
        instance: Osc99InstanceId,
        replaces: Option<Osc99InstanceId>,
        notification: &Notification99Data,
    ) -> Osc99SystemDispatch {
        self.shown.push(RecordedShow {
            instance,
            replaces,
            notification: notification.clone(),
        });
        if self.fail_shows {
            Osc99SystemDispatch::Failed
        } else {
            Osc99SystemDispatch::Queued
        }
    }

    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch {
        self.closed.push(instance);
        Osc99SystemDispatch::Queued
    }
}
