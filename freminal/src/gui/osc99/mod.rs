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

mod registry;

use std::sync::Weak;
use std::time::Duration;

use crossbeam_channel::Sender;
use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;
use freminal_common::buffer_states::osc99_notification::Osc99Expiry;
use freminal_common::buffer_states::window_manipulation::Notification99Data;
use freminal_terminal_emulator::io::{GuiReply, InputEvent};
use freminal_windowing::WindowId;

use crate::gui::frame_drain::WindowFocus;
use crate::gui::notifications::{Osc99Control, send_reply_weak};
use crate::gui::panes::PaneId;
use crate::gui::toast::{
    ToastContent, ToastKey, ToastKind, ToastLifetime, ToastPlacement, ToastStack,
};

pub use registry::Osc99Registry;

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
    /// observe (macOS and Windows).
    UntrackedSystem,
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
}

/// Build the toast for `data`.
///
/// - `Critical` urgency is an `Error` toast; everything else is `Info`.
/// - `w=` > 0 is an exact duration, `w=0` stays until dismissed, and
///   `w=-1` (or unset) follows urgency: `Critical` stays until dismissed,
///   `Normal` uses the toast default, `Low` uses
///   [`OSC99_LOW_URGENCY_TOAST_LIFETIME`].
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
}

/// The GUI state the executor acts on. Later subtasks add the system legs.
pub struct Osc99EffectSink<'a> {
    /// The shared toast stack.
    pub toasts: &'a mut ToastStack,
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
                } = spec;
                let key = instance.to_toast_key();
                let content = ToastContent {
                    kind,
                    title,
                    detail,
                    buttons,
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
