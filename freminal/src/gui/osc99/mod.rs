//! GUI-side OSC 99 (kitty desktop notification) types.
//!
//! The PTY thread parses and reassembles OSC 99 sequences; the GUI thread
//! decides whether and where to show them. The decision depends on *which
//! pane* the sequence came from and on that pane's state at the time of the
//! frame (focused? visible?), because the protocol's `o=` key is pane-aware
//! (`o=unfocused` / `o=invisible`). This module owns the vocabulary for that:
//! where a request came from ([`Osc99Origin`]), what state the originating
//! pane was in ([`Osc99PaneState`]), and the request structs that carry both
//! from the per-frame drain to the router.

mod capabilities;
#[cfg(all(unix, not(target_os = "macos")))]
mod dbus_service;
mod filter;
mod icon_cache;
#[cfg(all(unix, not(target_os = "macos")))]
mod icon_lookup;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
mod icon_raster;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
mod icon_tempfiles;
#[cfg(all(unix, not(target_os = "macos")))]
mod presentation;
mod registry;
mod untracked;

use std::sync::Weak;
use std::time::Duration;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationUrgency, Osc99Expiry,
};
use freminal_terminal_emulator::io::{GuiReply, InputEvent};
use freminal_windowing::WindowId;

use crate::gui::frame_drain::WindowFocus;
use crate::gui::icons::{ChromeIcon, standard_icon_glyph};
use crate::gui::notifications::{Osc99Control, send_reply_weak};
use crate::gui::panes::PaneId;
use crate::gui::toast::{
    ToastContent, ToastEventKind, ToastKey, ToastKind, ToastLifetime, ToastPlacement, ToastStack,
};

#[cfg(any(all(unix, not(target_os = "macos")), test))]
pub use capabilities::Osc99DaemonCapabilities;
pub use capabilities::{Osc99DaemonSupport, Osc99LinuxDaemon, Osc99Platform};
#[cfg(all(unix, not(target_os = "macos")))]
pub use dbus_service::{Osc99DbusEvent, Osc99DbusHandle, Osc99DbusService, Osc99Wake};
pub use filter::{Osc99Filter, Osc99FilterVerdict};
pub use icon_cache::Osc99IconCache;
pub use registry::Osc99Registry;
#[cfg(test)]
pub use untracked::RecordingUntrackedBackend;
pub use untracked::{NoUntrackedBackend, Osc99UntrackedBackend, platform_untracked_backend};

/// The registry's identity for one displayed notification.
///
/// Unlike the application's `i=` id (which may be absent, reused, or shared
/// by different panes), an instance id is unique for the process lifetime and
/// is minted by [`Osc99Registry::register`]. The toast leg uses the raw value
/// as its [`ToastKey`] (see [`Self::to_toast_key`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Osc99InstanceId(u64);

impl Osc99InstanceId {
    /// The [`ToastKey`] the instance's toast is pushed with.
    #[must_use]
    pub const fn to_toast_key(self) -> ToastKey {
        ToastKey(self.0)
    }

    /// The instance a toast event's key refers to (the inverse of
    /// [`Self::to_toast_key`]).
    #[must_use]
    pub const fn from_toast_key(key: ToastKey) -> Self {
        Self(key.0)
    }
}

/// Where a notification was shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99Leg {
    /// The in-window toast.
    Toast,
    /// A desktop notification whose close and activation freminal observes
    /// (Linux/BSD, via the D-Bus handle).
    LinuxSystem,
    /// A desktop notification whose close and activation freminal cannot
    /// observe (macOS and Windows). It is shown and forgotten: see
    /// [`Osc99UntrackedBackend`].
    UntrackedSystem,
}

/// How a user activated a displayed notification, whichever leg showed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99LegActivation {
    /// The notification body was activated.
    Body,
    /// The 1-based button was pressed.
    Button(u8),
}

/// An event from the Linux/BSD system leg, as the registry consumes it.
///
/// Only the Linux/BSD notification service produces these, so the type exists
/// only there.
#[cfg(all(unix, not(target_os = "macos")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99SystemLegEvent {
    /// The user activated the notification.
    Activated(Osc99LegActivation),
    /// The daemon closed the notification (dismissed, expired, or closed by
    /// call; the reason does not matter to the protocol).
    Closed,
    /// The daemon never displayed the notification.
    ShowFailed,
}

impl Osc99LegActivation {
    /// The activation a toast event stands for, or `None` for a `Closed`
    /// event (which is not an activation).
    #[must_use]
    pub const fn from_toast_event(kind: ToastEventKind) -> Option<Self> {
        match kind {
            ToastEventKind::Activated => Some(Self::Body),
            ToastEventKind::Button(n) => Some(Self::Button(n)),
            ToastEventKind::Closed => None,
        }
    }
}

/// How long a `Low`-urgency notification's toast stays when the application
/// did not ask for a duration (`w=-1`, or no `w=`): half the `Info` toast
/// default of 6 s. The toast module's per-kind default is private to it, so
/// the value is restated here.
pub const OSC99_LOW_URGENCY_TOAST_LIFETIME: Duration = Duration::from_secs(3);

/// What an OSC 99 notification looks like as a toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99ToastSpec {
    /// The toast severity.
    pub kind: ToastKind,
    /// The notification title.
    pub title: String,
    /// The notification body, if any.
    pub detail: Option<String>,
    /// How long the toast stays.
    pub lifetime: ToastLifetime,
    /// The button labels (`p=buttons`), in order. The toast keeps at most
    /// eight.
    pub buttons: Vec<String>,
    /// The glyph for the first `n=` name that is a standard icon name, names
    /// in order. `None` leaves the toast's kind icon.
    pub icon: Option<ChromeIcon>,
}

/// Build the toast for `data`.
///
/// - `Critical` urgency is an `Error` toast; everything else is `Info`.
/// - `w=` > 0 is an exact duration, `w=0` stays until dismissed, and
///   `w=-1` (or unset) follows urgency: `Critical` stays until dismissed,
///   `Normal` uses the toast default, `Low` uses
///   [`OSC99_LOW_URGENCY_TOAST_LIFETIME`].
/// - The icon is the first standard `n=` name's glyph (see
///   [`standard_icon_glyph`]); toasts render no images, so transmitted icon
///   data never changes the toast.
#[must_use]
pub fn osc99_toast_spec(data: &Notification99Data) -> Osc99ToastSpec {
    let kind = match data.urgency {
        NotificationUrgency::Critical => ToastKind::Error,
        NotificationUrgency::Low | NotificationUrgency::Normal => ToastKind::Info,
    };
    let lifetime = match data.expiry {
        Osc99Expiry::After(ms) => ToastLifetime::Fixed(Duration::from_millis(u64::from(ms.get()))),
        Osc99Expiry::Never => ToastLifetime::UntilDismissed,
        Osc99Expiry::OsDefault => match data.urgency {
            NotificationUrgency::Critical => ToastLifetime::UntilDismissed,
            NotificationUrgency::Normal => ToastLifetime::KindDefault,
            NotificationUrgency::Low => ToastLifetime::Fixed(OSC99_LOW_URGENCY_TOAST_LIFETIME),
        },
    };
    Osc99ToastSpec {
        kind,
        title: data.title.clone(),
        detail: data.body.clone().filter(|body| !body.is_empty()),
        lifetime,
        buttons: data.buttons.clone(),
        icon: data
            .icon_names
            .iter()
            .find_map(|name| standard_icon_glyph(name)),
    }
}

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
        /// What to show. Boxed: it is far larger than every other effect.
        notification: Box<Notification99Data>,
    },
    /// Show a notification on the untracked system leg (macOS and Windows).
    ///
    /// There is no instance to track, update or close: the notification is
    /// shown and forgotten, and an update of the same `i=` shows another one.
    /// Only macOS and Windows route to that leg, but the registry is
    /// platform-independent, so the variant exists everywhere.
    ShowUntrackedSystem(
        /// What to show. Boxed: it is far larger than every other effect.
        Box<Notification99Data>,
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
    /// [`Osc99SystemLegEvent`]. Only the Linux/BSD service (and the test
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
/// macOS and Windows never route to [`Osc99Leg::LinuxSystem`], so for them it
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
    /// queue, in effect order. The caller feeds each back to the registry as a
    /// failed show. Always empty on macOS and Windows, which never produce
    /// that effect (the field exists there only because the executor is
    /// shared).
    pub failed_system_shows: Vec<Osc99InstanceId>,
    /// Instances whose [`Osc99Effect::CloseLinuxSystem`] the backend could not
    /// queue, in effect order. The caller feeds each back to the registry as a
    /// `Closed` event, because no real one will come. Always empty on macOS and
    /// Windows, as for [`Self::failed_system_shows`].
    pub failed_system_closes: Vec<Osc99InstanceId>,
}

/// The instances whose system-leg requests the backend could not queue, as
/// collected by one run of the executor. No service event will ever follow
/// them, so the caller feeds each back to the registry (a failed show as
/// `ShowFailed`, a failed close as `Closed`).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Osc99FailedSystemLegs {
    /// Instances whose [`Osc99Effect::ShowLinuxSystem`] failed.
    pub shows: Vec<Osc99InstanceId>,
    /// Instances whose [`Osc99Effect::CloseLinuxSystem`] failed.
    pub closes: Vec<Osc99InstanceId>,
}

impl Osc99FailedSystemLegs {
    /// Whether nothing failed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.shows.is_empty() && self.closes.is_empty()
    }
}

/// Execute `effects` in order against `sink`.
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

/// The pane an OSC 99 request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99Origin {
    /// The OS window that owns the pane.
    pub window_id: WindowId,
    /// The pane that emitted the sequence.
    pub pane_id: PaneId,
}

/// Whether a pane has keyboard focus, in the sense the `o=unfocused` key
/// needs: the OS window is focused, the pane's tab is active, and the pane is
/// that tab's active pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKeyboardFocus {
    /// The pane would receive keyboard input right now.
    Focused,
    /// The pane would not receive keyboard input right now.
    Unfocused,
}

/// Whether a pane is on screen, in the sense the `o=invisible` key needs: the
/// OS window is focused and not minimised, the pane's tab is active, and no
/// other pane in that tab is zoomed over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneVisibility {
    /// The pane is drawn on screen.
    Visible,
    /// The pane is not drawn on screen.
    Hidden,
}

/// The state of an OSC 99 request's originating pane at the time it was
/// drained, as consumed by the `o=` occasion check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99PaneState {
    /// Whether the pane has keyboard focus.
    pub focus: PaneKeyboardFocus,
    /// Whether the pane is on screen.
    pub visibility: PaneVisibility,
}

/// Whether the OS window is minimised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMinimized {
    /// The window is minimised.
    Minimized,
    /// The window is not minimised.
    Restored,
}

impl WindowMinimized {
    /// Build from the raw `bool` read off the egui viewport info.
    pub const fn from_bool(minimized: bool) -> Self {
        if minimized {
            Self::Minimized
        } else {
            Self::Restored
        }
    }
}

/// Whether a pane's tab is the window's active tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabActivity {
    /// The tab is the window's active tab.
    Active,
    /// The tab is a background tab.
    Background,
}

/// Whether a pane is its tab's active pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneActivity {
    /// The pane is its tab's active pane.
    Active,
    /// The pane is not its tab's active pane.
    Inactive,
}

/// Whether another pane's zoom hides a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomOcclusion {
    /// The pane's tab has no zoomed pane, or the zoomed pane is this one.
    NotOccluded,
    /// The pane's tab has a zoomed pane that is not this pane.
    Occluded,
}

impl Osc99PaneState {
    /// Derive a pane's state from the window, tab and pane inputs.
    ///
    /// - Focused iff the window is focused, the tab is active and the pane is
    ///   the tab's active pane.
    /// - Visible iff the window is focused and not minimised, the tab is
    ///   active, and no other pane is zoomed.
    #[must_use]
    pub const fn compute(
        window: WindowFocus,
        minimized: WindowMinimized,
        tab: TabActivity,
        pane: PaneActivity,
        zoom: ZoomOcclusion,
    ) -> Self {
        let window_focused = matches!(window, WindowFocus::Focused);
        let tab_active = matches!(tab, TabActivity::Active);

        let focus = if window_focused && tab_active && matches!(pane, PaneActivity::Active) {
            PaneKeyboardFocus::Focused
        } else {
            PaneKeyboardFocus::Unfocused
        };
        let visibility = if window_focused
            && matches!(minimized, WindowMinimized::Restored)
            && tab_active
            && matches!(zoom, ZoomOcclusion::NotOccluded)
        {
            PaneVisibility::Visible
        } else {
            PaneVisibility::Hidden
        };

        Self { focus, visibility }
    }
}

/// Where a pane's OSC 99 requests came from and what state it was in, built
/// once per pane per frame by the drain and handed to
/// `rendering::handle_window_manipulation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99PaneContext {
    /// The pane the requests originate from.
    pub origin: Osc99Origin,
    /// The pane's state this frame.
    pub pane_state: Osc99PaneState,
}

/// An OSC 99 notification to display, collected by the per-frame drain.
#[derive(Debug, Clone)]
pub struct Osc99DisplayRequest {
    /// The reassembled notification.
    pub data: Notification99Data,
    /// A `Weak` handle to the originating pane's reply sender, so a closed
    /// pane's PTY consumer is never kept alive by a pending notification.
    pub reply_tx: Weak<Sender<InputEvent>>,
    /// The pane the notification came from.
    pub origin: Osc99Origin,
    /// The originating pane's state when it was drained.
    pub pane_state: Osc99PaneState,
}

/// An OSC 99 control request (`p=close` / `p=alive`), collected by the
/// per-frame drain.
#[derive(Debug, Clone)]
pub struct Osc99ControlRequest {
    /// The control being requested.
    pub control: Osc99Control,
    /// A `Weak` handle to the originating pane's reply sender.
    pub reply_tx: Weak<Sender<InputEvent>>,
    /// The pane the control came from.
    pub origin: Osc99Origin,
}

#[cfg(test)]
mod tests {
    use super::{
        Osc99PaneState, PaneActivity, PaneKeyboardFocus, PaneVisibility, TabActivity,
        WindowMinimized, ZoomOcclusion,
    };
    use crate::gui::frame_drain::WindowFocus;

    const FOCUSED: WindowFocus = WindowFocus::Focused;
    const UNFOCUSED: WindowFocus = WindowFocus::Unfocused;
    const RESTORED: WindowMinimized = WindowMinimized::Restored;
    const MINIMIZED: WindowMinimized = WindowMinimized::Minimized;

    fn state(
        window: WindowFocus,
        minimized: WindowMinimized,
        tab: TabActivity,
        pane: PaneActivity,
        zoom: ZoomOcclusion,
    ) -> (PaneKeyboardFocus, PaneVisibility) {
        let s = Osc99PaneState::compute(window, minimized, tab, pane, zoom);
        (s.focus, s.visibility)
    }

    #[test]
    fn active_pane_of_active_tab_in_focused_window_is_focused_and_visible() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Focused, PaneVisibility::Visible)
        );
    }

    #[test]
    fn unfocused_window_makes_every_pane_unfocused_and_hidden() {
        assert_eq!(
            state(
                UNFOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn minimized_window_hides_the_pane_but_does_not_change_focus() {
        // Focus is "would receive keyboard input"; minimisation is a
        // visibility concept only, so a focused-but-minimised window (a
        // transient state some platforms report) keeps `Focused`.
        assert_eq!(
            state(
                FOCUSED,
                MINIMIZED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Focused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn background_tab_is_unfocused_and_hidden_even_for_its_active_pane() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Background,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn inactive_pane_in_active_tab_is_unfocused_but_visible() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Inactive,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Visible)
        );
    }

    #[test]
    fn pane_occluded_by_another_panes_zoom_is_hidden() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Inactive,
                ZoomOcclusion::Occluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn exhaustive_inputs_match_the_stated_rules() {
        for window in [FOCUSED, UNFOCUSED] {
            for minimized in [RESTORED, MINIMIZED] {
                for tab in [TabActivity::Active, TabActivity::Background] {
                    for pane in [PaneActivity::Active, PaneActivity::Inactive] {
                        for zoom in [ZoomOcclusion::NotOccluded, ZoomOcclusion::Occluded] {
                            let s = Osc99PaneState::compute(window, minimized, tab, pane, zoom);
                            let expect_focused = window == FOCUSED
                                && tab == TabActivity::Active
                                && pane == PaneActivity::Active;
                            let expect_visible = window == FOCUSED
                                && minimized == RESTORED
                                && tab == TabActivity::Active
                                && zoom == ZoomOcclusion::NotOccluded;
                            assert_eq!(s.focus == PaneKeyboardFocus::Focused, expect_focused);
                            assert_eq!(s.visibility == PaneVisibility::Visible, expect_visible);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn window_minimized_from_bool() {
        assert_eq!(WindowMinimized::from_bool(true), MINIMIZED);
        assert_eq!(WindowMinimized::from_bool(false), RESTORED);
    }
}
