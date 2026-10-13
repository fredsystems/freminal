//! The OSC 99 effects: what the registry asks the GUI to do, the backends that
//! show and close desktop notifications, and the executor that applies a batch
//! of effects (feeding a failed system leg back to the registry).

use std::sync::{Arc, Weak};

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::Notification99Data;
use freminal_terminal_emulator::io::{GuiReply, InputEvent};

use super::{Osc99InstanceId, Osc99Origin, Osc99ToastSpec, Osc99UntrackedBackend};
use crate::gui::notifications::send_reply_weak;
use crate::gui::toast::{ToastContent, ToastPlacement, ToastStack};

/// Something the registry asks the GUI to do.
///
/// The registry is a pure state machine and returns effects; the executor
/// ([`apply_osc99_effects`]) performs them.
#[derive(Debug)]
pub enum Osc99Effect {
    /// Write `reply` to the pane behind `reply_tx`.
    Reply {
        /// A `Weak` handle to the pane's reply sender.
        reply_tx: Weak<Sender<InputEvent>>,
        /// The reply to serialise.
        reply: GuiReply,
    },
    /// Show `instance`'s toast, updating `replaces`' toast in place when
    /// that is set.
    ShowToast {
        /// The instance the toast belongs to.
        instance: Osc99InstanceId,
        /// The instance whose toast this one takes over, if any.
        replaces: Option<Osc99InstanceId>,
        /// What to show.
        spec: Osc99ToastSpec,
    },
    /// Remove `instance`'s toast (a no-op if it is already gone).
    CloseToast(Osc99InstanceId),
    /// Ask the desktop-notification service to close `instance`'s desktop
    /// notification. Its `Closed` event arrives later; a close that cannot be
    /// queued is fed back as one by the caller (see
    /// [`Osc99EffectSink::failed_system_closes`]).
    ///
    /// Only the Linux/BSD service ever produces the leg this closes, but the
    /// registry is platform-independent, so the variant exists everywhere.
    CloseLinuxSystem(Osc99InstanceId),
    /// Show `instance` on the Linux/BSD system leg, updating `replaces`'
    /// notification in place when that is set.
    ShowLinuxSystem {
        /// The instance the desktop notification belongs to.
        instance: Osc99InstanceId,
        /// The instance whose desktop notification this one takes over, if
        /// any.
        replaces: Option<Osc99InstanceId>,
        /// What to show. Shared: it is far larger than every other effect,
        /// and the registry hands the same allocation to every system leg.
        notification: Arc<Notification99Data>,
    },
    /// Show a notification on the untracked system leg (macOS and Windows).
    ///
    /// There is no instance to track, update or close: the notification is
    /// shown and forgotten, and an update of the same `i=` shows another one.
    /// Only macOS and Windows route to that leg, but the registry is
    /// platform-independent, so the variant exists everywhere.
    ShowUntrackedSystem(
        /// What to show. Shared: it is far larger than every other effect.
        Arc<Notification99Data>,
    ),
    /// Bring `origin`'s pane into focus: switch to its tab, unzoom, make it
    /// the active pane and raise its OS window. Emitted when a notification
    /// with `a=focus` is activated.
    FocusPane(Osc99Origin),
}

/// Whether a backend accepted a show or close request.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99SystemDispatch {
    /// The request was queued; its outcome arrives later as an
    /// [`Osc99SystemLegEvent`](super::Osc99SystemLegEvent). Only the Linux/BSD service (and the test
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
/// macOS and Windows never route to [`Osc99Leg::LinuxSystem`](super::Osc99Leg::LinuxSystem), so for them it
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
            instance = instance.0,
            "OSC 99: no system notification service; show failed"
        );
        Osc99SystemDispatch::Failed
    }

    fn close(&mut self, instance: Osc99InstanceId) -> Osc99SystemDispatch {
        tracing::trace!(
            instance = instance.0,
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

/// The GUI state the executor acts on.
pub struct Osc99EffectSink<'a> {
    /// The shared toast stack.
    pub toasts: &'a mut ToastStack,
    /// The Linux/BSD desktop-notification service.
    pub system: &'a mut dyn Osc99SystemBackend,
    /// The macOS and Windows fire-and-forget desktop notification.
    pub untracked: &'a mut dyn Osc99UntrackedBackend,
    /// Panes to focus, collected in effect order.
    ///
    /// The executor has no window access, so [`Osc99Effect::FocusPane`] is
    /// recorded here and performed by the caller once it can reach the
    /// windows (the next `update()`).
    pub focus_requests: Vec<Osc99Origin>,
    /// Instances whose [`Osc99Effect::ShowLinuxSystem`] the backend could not
    /// queue, in effect order. `glue::apply_and_feed_back` feeds each back to the
    /// registry as a failed show. Always empty on macOS and Windows, which never produce
    /// that effect (the field exists there only because the executor is
    /// shared).
    pub failed_system_shows: Vec<Osc99InstanceId>,
    /// Instances whose [`Osc99Effect::CloseLinuxSystem`] the backend could not
    /// queue, in effect order. `glue::apply_and_feed_back` feeds each back to the
    /// registry as a `Closed` event, because no real one will come. Always empty on macOS and
    /// Windows, as for [`Self::failed_system_shows`].
    pub failed_system_closes: Vec<Osc99InstanceId>,
}

impl<'a> Osc99EffectSink<'a> {
    /// A sink over the given toast stack and backends, with nothing queued and
    /// nothing failed.
    #[must_use]
    pub fn new(
        toasts: &'a mut ToastStack,
        system: &'a mut dyn Osc99SystemBackend,
        untracked: &'a mut dyn Osc99UntrackedBackend,
    ) -> Self {
        Self {
            toasts,
            system,
            untracked,
            focus_requests: Vec::new(),
            failed_system_shows: Vec::new(),
            failed_system_closes: Vec::new(),
        }
    }
}

/// Execute `effects` in order against `sink`.
///
/// A show or close the system backend could not queue is recorded in the
/// sink's `failed_system_*` lists; the caller ends those legs (see
/// `glue::apply_and_feed_back`).
pub fn apply_osc99_effects(effects: Vec<Osc99Effect>, sink: &mut Osc99EffectSink<'_>) {
    for effect in effects {
        match effect {
            Osc99Effect::Reply { reply_tx, reply } => send_reply_weak(&reply_tx, reply),
            Osc99Effect::ShowToast {
                instance,
                replaces,
                spec,
            } => {
                let Osc99ToastSpec {
                    kind,
                    title,
                    detail,
                    lifetime,
                    buttons,
                    icon,
                } = spec;
                let key = instance.to_toast_key();
                let content = ToastContent {
                    kind,
                    title,
                    detail,
                    buttons,
                    icon,
                };
                if let Some(old) = replaces {
                    sink.toasts
                        .replace_keyed(old.to_toast_key(), key, content, lifetime);
                } else {
                    sink.toasts
                        .push_keyed(key, content, lifetime, ToastPlacement::TOP_RIGHT);
                }
            }
            Osc99Effect::CloseToast(instance) => sink.toasts.close_keyed(instance.to_toast_key()),
            Osc99Effect::ShowLinuxSystem {
                instance,
                replaces,
                notification,
            } => {
                if sink.system.show(instance, replaces, &notification)
                    == Osc99SystemDispatch::Failed
                {
                    sink.failed_system_shows.push(instance);
                }
            }
            Osc99Effect::CloseLinuxSystem(instance) => {
                if sink.system.close(instance) == Osc99SystemDispatch::Failed {
                    sink.failed_system_closes.push(instance);
                }
            }
            Osc99Effect::ShowUntrackedSystem(notification) => sink.untracked.show(&notification),
            Osc99Effect::FocusPane(origin) => sink.focus_requests.push(origin),
        }
    }
}
