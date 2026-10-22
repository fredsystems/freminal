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
//!
//! The vocabulary lives here; the behaviour is in the submodules:
//!
//! - `pane_state`: where a request came from and the pane state `o=` reads;
//! - `toast_spec`: what a notification looks like as a toast;
//! - `effects`: what the registry asks the GUI to do, and the executor that
//!   does it;
//! - `glue`: the `FreminalGui` side that routes a frame's requests, delivers
//!   events and drains focus requests, and the plain-input functions it is
//!   built from.

mod effects;
mod filter;
mod glue;
mod icon_cache;
mod pane_state;
mod registry;
mod toast_spec;

use std::sync::Weak;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc99_notification::Notification99Data;
use freminal_terminal_emulator::io::InputEvent;

use crate::gui::desktop_notify::Osc99InstanceId;
#[cfg(all(unix, not(target_os = "macos")))]
use crate::gui::desktop_notify::Osc99SystemAction;
use crate::gui::notifications::Osc99Control;
use crate::gui::toast::{ToastEventKind, ToastKey};

pub use effects::{Osc99Effect, Osc99EffectSink, apply_osc99_effects};
pub use filter::{Osc99Filter, Osc99FilterVerdict};
pub(in crate::gui) use glue::Osc99CellAccess;
pub use icon_cache::Osc99IconCache;
pub use pane_state::{
    Osc99Origin, Osc99PaneContext, Osc99PaneState, PaneActivity, PaneKeyboardFocus, PaneVisibility,
    TabActivity, WindowMinimized, ZoomOcclusion,
};
pub use registry::Osc99Registry;
pub use toast_spec::{Osc99ToastSpec, osc99_toast_spec};

/// The [`ToastKey`] an instance's toast is pushed with: the instance's raw
/// value.
#[must_use]
pub const fn instance_toast_key(instance: Osc99InstanceId) -> ToastKey {
    ToastKey(instance.get())
}

/// The instance a toast event's key refers to (the inverse of
/// [`instance_toast_key`]).
#[must_use]
pub const fn instance_from_toast_key(key: ToastKey) -> Osc99InstanceId {
    Osc99InstanceId::new(key.0)
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

#[cfg(all(unix, not(target_os = "macos")))]
impl From<Osc99SystemAction> for Osc99LegActivation {
    /// The registry's vocabulary for the same activation.
    fn from(action: Osc99SystemAction) -> Self {
        match action {
            Osc99SystemAction::Default => Self::Body,
            Osc99SystemAction::Button(n) => Self::Button(n),
        }
    }
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

/// Icon bytes an OSC 99 notification transmitted under a `g=` cache key
/// without any text to show (`WindowManipulation::Osc99CacheIcon`), collected
/// by the per-frame drain so the caller can store them in the session's icon
/// cache, which the per-pane loop cannot reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99IconPrime {
    /// The cache key (`g=`), sanitised.
    pub key: String,
    /// The icon image bytes.
    pub data: Vec<u8>,
}

impl Osc99IconPrime {
    /// Store the icon in `cache` under its key, as the most recently used
    /// entry. The cache applies its own bounds. The prime is consumed: the
    /// cache takes the bytes without copying them.
    pub fn store_in(self, cache: &mut Osc99IconCache) {
        cache.insert(self.key, self.data);
    }
}

/// One OSC 99 request collected by the per-frame drain.
///
/// A frame's requests are kept in ONE ordered list, in the order the panes
/// emitted them, and routed strictly in that order. Keeping display, control
/// and icon-priming requests in separate lists lost the order between them:
/// `p=close;i=x` followed by a new `i=x` display would close the new
/// instance, `p=alive` would answer with instances shown after it, and a
/// priming that preceded a display could be stored after it.
#[derive(Debug, Clone)]
pub enum Osc99Request {
    /// A notification to display. Boxed: it is far larger than the other
    /// variants.
    Display(Box<Osc99DisplayRequest>),
    /// A `p=close` / `p=alive` control.
    Control(Osc99ControlRequest),
    /// Icon bytes for the `g=` cache.
    PrimeIcon(Osc99IconPrime),
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn system_actions_map_to_the_registry_activations() {
        assert_eq!(
            Osc99LegActivation::from(Osc99SystemAction::Default),
            Osc99LegActivation::Body
        );
        assert_eq!(
            Osc99LegActivation::from(Osc99SystemAction::Button(3)),
            Osc99LegActivation::Button(3)
        );
    }
}
