// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Host-dependent capability facts for the PTY thread.
//!
//! Resolves, from the config, the platform and what is known about the
//! Linux/BSD notification daemon, the capability facts the PTY thread cannot
//! know by itself (Tasks 130.4, 138.15), and re-sends them to every pane when
//! they change. Later facets (Task 146) are added here.

#[cfg(all(unix, not(target_os = "macos")))]
use freminal_common::config::NotificationsConfig;
use freminal_common::config::{Config, NotificationRouting};
use freminal_common::host_capabilities::{
    HostCapabilities, KittyFileTransmission, Osc99ActivationReport, Osc99AutoExpiry,
    Osc99BodySupport, Osc99ButtonSupport, Osc99CloseEvents, Osc99Features, Osc99FocusSupport,
    Osc99IconSupport, Osc99SoundNames, Osc99Support, Osc99UrgencySupport,
};

use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use tracing::error;

use super::FreminalGui;
use super::desktop_notify::{Osc99DaemonSupport, Osc99LinuxDaemon, Osc99Platform};
use super::tabs::TabManager;
use super::window::PerWindowState;

/// Resolve the host-dependent capability facts from the config, the platform
/// and what is known about the Linux/BSD notification daemon.
///
/// The PTY thread cannot know these itself (Task 130.4); the GUI computes this
/// value at pane spawn and re-sends it via `InputEvent::HostCapabilitiesChange`
/// whenever it changes.
///
/// OSC 99 is [`Osc99Support::Unsupported`] unless the master switch
/// (`notifications.enabled`) and the per-protocol switch
/// (`notifications.osc_99`) are both on. Otherwise the features follow
/// [`combine`]: each leg the `routing_osc99` setting uses has a capability
/// table, and the routing decides how the legs' tables combine.
pub(super) fn host_capabilities(config: &Config, daemon: Osc99LinuxDaemon) -> HostCapabilities {
    HostCapabilities {
        osc99: osc99_support(config, daemon),
        kitty_file_transmission: kitty_file_transmission(config),
    }
}

/// The OSC 99 facet of [`host_capabilities`].
fn osc99_support(config: &Config, daemon: Osc99LinuxDaemon) -> Osc99Support {
    let notifications = &config.notifications;
    if !(notifications.enabled && notifications.osc_99) {
        return Osc99Support::Unsupported;
    }
    combine(
        notifications.routing_osc99,
        Osc99Platform::current(),
        daemon,
    )
}

/// The kitty file-transmission facet of [`host_capabilities`]:
/// [`KittyFileTransmission::Allowed`] only when `[security]
/// allow_kitty_file_transfer` is on (GHSA-8prj-mhmf-xpjg).
const fn kitty_file_transmission(config: &Config) -> KittyFileTransmission {
    if config.security.allow_kitty_file_transfer {
        KittyFileTransmission::Allowed
    } else {
        KittyFileTransmission::Refused
    }
}

/// Whether `config` can route an OSC 99 notification to the system
/// (desktop-notification) leg: OSC 99 is on (`notifications.enabled` and
/// `notifications.osc_99`) and `routing_osc99` uses the system leg.
///
/// The Linux/BSD notification service is started while this holds, so the
/// daemon's `GetCapabilities` answer is normally in before an application's
/// first `p=?`.
#[cfg(all(unix, not(target_os = "macos")))]
#[must_use]
pub(super) const fn osc99_may_use_system_leg(config: &Config) -> bool {
    let notifications = &config.notifications;
    if !(notifications.enabled && notifications.osc_99) {
        return false;
    }
    routing_has_system_leg(notifications.routing_osc99)
}

/// Whether `routing` ever shows a desktop notification, at some focus state.
#[cfg(all(unix, not(target_os = "macos")))]
const fn routing_has_system_leg(routing: NotificationRouting) -> bool {
    // No wildcard: a new routing variant must be classified here.
    match routing {
        NotificationRouting::System
        | NotificationRouting::Both
        | NotificationRouting::SystemWhenUnfocused => true,
        NotificationRouting::Toast | NotificationRouting::Disabled => false,
    }
}

/// The way a plain notification (OSC 9, OSC 777, OSC 133 `D` command-finished,
/// the Settings "Test Notification" button) reaches the router, which decides
/// whether the `enabled` master switch applies to it.
///
/// The same on every platform, so a call site needs no `cfg`: only Linux/BSD
/// reads it (see `FreminalGui::system_notify_sink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlainNotifyEntry {
    /// `NotificationRouter::route`: nothing is shown unless
    /// `notifications.enabled` is on.
    Routed,
    /// `NotificationRouter::route_test`, the Test Notification button: it
    /// ignores `notifications.enabled`, so the user gets feedback while they
    /// are still configuring routing. Its sample request is an OSC-text one,
    /// so only `routing_osc_text` matters.
    SettingsTest,
}

/// Whether `config` can route a plain notification that arrives through
/// `entry` to the system (desktop-notification) leg.
///
/// The Linux/BSD notification service is started while this holds, so the
/// first OSC 9 / command-finished notification does not wait for a connection
/// that was never opened. The answer errs on the side of starting: a category
/// counts when its routing uses the system leg at any focus state, whatever
/// the per-source `osc_9` / `osc_777` / `on_command_finished` toggles say,
/// because starting the service early is cheap and a missed start would drop
/// notifications.
#[cfg(all(unix, not(target_os = "macos")))]
#[must_use]
pub(super) const fn plain_notifications_may_use_system_leg(
    config: &NotificationsConfig,
    entry: PlainNotifyEntry,
) -> bool {
    match entry {
        PlainNotifyEntry::Routed => {
            config.enabled
                && (routing_has_system_leg(config.routing_osc_text)
                    || routing_has_system_leg(config.routing_command_finished)
                    || routing_has_system_leg(config.routing_error))
        }
        PlainNotifyEntry::SettingsTest => routing_has_system_leg(config.routing_osc_text),
    }
}

/// Whether `config` can route any notification, OSC 99 or plain, to the
/// system (desktop-notification) leg through the normal routed path: the
/// condition for starting the Linux/BSD notification service from
/// `FreminalGui::ensure_osc99_dbus_started`.
///
/// The Settings "Test Notification" button has its own entry
/// ([`PlainNotifyEntry::SettingsTest`]) and starts the service itself.
#[cfg(all(unix, not(target_os = "macos")))]
#[must_use]
pub(super) const fn notification_service_wanted(config: &Config) -> bool {
    osc99_may_use_system_leg(config)
        || plain_notifications_may_use_system_leg(&config.notifications, PlainNotifyEntry::Routed)
}

/// The capabilities to re-broadcast when the Linux/BSD daemon's state goes
/// from `old` to `new` under `config`: `Some` exactly when the resulting
/// [`HostCapabilities`] differ.
///
/// A daemon change the routing does not read (toast-only routing, OSC 99 off)
/// resolves to the same value and returns `None`.
#[cfg(all(unix, not(target_os = "macos")))]
#[must_use]
pub(super) fn daemon_state_change(
    config: &Config,
    old: Osc99LinuxDaemon,
    new: Osc99LinuxDaemon,
) -> Option<HostCapabilities> {
    let before = host_capabilities(config, old);
    let after = host_capabilities(config, new);
    (before != after).then_some(after)
}

/// Send `capabilities` to every pane of every tab in `windows` and in
/// `in_flight` (`InputEvent::HostCapabilitiesChange`).
///
/// `in_flight` is the window `update()` has taken out of the map for the
/// frame; see [`FreminalGui::broadcast_host_capabilities`]. A tab whose pane
/// tree cannot be walked is logged and skipped.
fn send_host_capabilities<'a>(
    windows: impl IntoIterator<Item = &'a TabManager>,
    in_flight: Option<&'a TabManager>,
    capabilities: HostCapabilities,
) {
    for tabs in windows.into_iter().chain(in_flight) {
        for tab in tabs {
            match tab.pane_tree.iter_panes() {
                Ok(panes) => {
                    for pane in panes {
                        send_or_log!(
                            pane.input_tx,
                            InputEvent::HostCapabilitiesChange(capabilities),
                            "Failed to send HostCapabilitiesChange to PTY thread"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        "iter_panes() failed on tab during host capabilities \
                         broadcast: {e}; skipping this tab"
                    );
                }
            }
        }
    }
}

impl FreminalGui {
    /// Send `capabilities` to every pane of every window, via
    /// `InputEvent::HostCapabilitiesChange`.
    ///
    /// `current` is the window being rendered. `update()` removes that window
    /// from `self.windows` for the duration of the frame, so code running
    /// inside it must pass the window here or its panes would miss the
    /// broadcast. Pass `None` when no window is out of the map (e.g. from the
    /// settings window's frame); passing a window that is also in the map
    /// would send to its panes twice.
    pub(super) fn broadcast_host_capabilities(
        &self,
        capabilities: HostCapabilities,
        current: Option<&PerWindowState>,
    ) {
        send_host_capabilities(
            self.windows.values().map(|win| &win.tabs),
            current.map(|win| &win.tabs),
            capabilities,
        );
    }
}

/// The in-window toast leg's capabilities. They do not depend on the platform
/// or the daemon.
///
/// The toast reports activation and close, focuses the window on activation,
/// honours expiry and urgency, and shows a body and buttons. It draws a glyph
/// for the eight standard icon names (`n=`), but never a transmitted image, so
/// `icons` (which is about a transmitted `p=icon` payload) is `Unsupported`.
/// It plays no sounds and understands only the `system` and `silent` sound
/// names.
#[must_use]
pub(super) const fn toast_leg() -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::Reported,
        close_events: Osc99CloseEvents::Reported,
        activation_focus: Osc99FocusSupport::Supported,
        auto_expiry: Osc99AutoExpiry::Supported,
        urgency: Osc99UrgencySupport::Supported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Supported,
        buttons: Osc99ButtonSupport::Supported,
        icons: Osc99IconSupport::Unsupported,
    }
}

/// The system (desktop-notification) leg's capabilities on `platform`, or
/// `None` when the leg cannot show anything.
///
/// - **Linux/BSD** follows the daemon's advertised capabilities: activation
///   report, focus and buttons need `actions`; the body needs `body`; the
///   standard sound names need `sound`. Close events, expiry, urgency and
///   icons are always available. With no daemon the leg cannot show anything.
/// - **macOS** reports neither activation nor close, and honours neither expiry
///   nor urgency; it shows a body and icons but no buttons.
/// - **Windows** is the same as macOS. Its toast has no urgency levels (the
///   "reminder" scenario needs buttons, which this leg cannot report), so
///   urgency is not advertised (138.C17).
///
/// `daemon` is read only for [`Osc99Platform::LinuxBsd`].
#[must_use]
pub(super) const fn system_leg(
    platform: Osc99Platform,
    daemon: Osc99LinuxDaemon,
) -> Option<Osc99Features> {
    match platform {
        Osc99Platform::LinuxBsd => match daemon {
            Osc99LinuxDaemon::Unavailable => None,
            Osc99LinuxDaemon::Available(caps) => {
                let has_actions = matches!(caps.actions, Osc99DaemonSupport::Supported);
                Some(Osc99Features {
                    activation_report: if has_actions {
                        Osc99ActivationReport::Reported
                    } else {
                        Osc99ActivationReport::NotReported
                    },
                    close_events: Osc99CloseEvents::Reported,
                    activation_focus: if has_actions {
                        Osc99FocusSupport::Supported
                    } else {
                        Osc99FocusSupport::Unsupported
                    },
                    auto_expiry: Osc99AutoExpiry::Supported,
                    urgency: Osc99UrgencySupport::Supported,
                    sound_names: match caps.sound {
                        Osc99DaemonSupport::Supported => Osc99SoundNames::Standard,
                        Osc99DaemonSupport::Unsupported => Osc99SoundNames::SystemAndSilent,
                    },
                    body: match caps.body {
                        Osc99DaemonSupport::Supported => Osc99BodySupport::Supported,
                        Osc99DaemonSupport::Unsupported => Osc99BodySupport::Unsupported,
                    },
                    buttons: if has_actions {
                        Osc99ButtonSupport::Supported
                    } else {
                        Osc99ButtonSupport::Unsupported
                    },
                    icons: Osc99IconSupport::Supported,
                })
            }
        },
        Osc99Platform::MacOs | Osc99Platform::Windows => Some(untracked_system_leg()),
    }
}

/// The macOS / Windows system leg: fire-and-forget, with no reports, expiry,
/// urgency or buttons.
const fn untracked_system_leg() -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        close_events: Osc99CloseEvents::NotReported,
        activation_focus: Osc99FocusSupport::Unsupported,
        auto_expiry: Osc99AutoExpiry::Unsupported,
        urgency: Osc99UrgencySupport::Unsupported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Supported,
        buttons: Osc99ButtonSupport::Unsupported,
        icons: Osc99IconSupport::Supported,
    }
}

/// `yes` if `a` or `b` is `yes`, otherwise `no`. One field of [`union`].
fn either<T: Copy + PartialEq>(a: T, b: T, yes: T, no: T) -> T {
    if a == yes || b == yes { yes } else { no }
}

/// `yes` if both `a` and `b` are `yes`, otherwise `no`. One field of
/// [`intersection`].
fn both<T: Copy + PartialEq>(a: T, b: T, yes: T, no: T) -> T {
    if a == yes && b == yes { yes } else { no }
}

/// The features present in **either** `a` or `b`. For sound names, `Standard`
/// if either understands it.
///
/// The struct literal names every field, so a new [`Osc99Features`] field
/// fails to compile here until it is classified.
fn union(a: Osc99Features, b: Osc99Features) -> Osc99Features {
    Osc99Features {
        activation_report: either(
            a.activation_report,
            b.activation_report,
            Osc99ActivationReport::Reported,
            Osc99ActivationReport::NotReported,
        ),
        close_events: either(
            a.close_events,
            b.close_events,
            Osc99CloseEvents::Reported,
            Osc99CloseEvents::NotReported,
        ),
        activation_focus: either(
            a.activation_focus,
            b.activation_focus,
            Osc99FocusSupport::Supported,
            Osc99FocusSupport::Unsupported,
        ),
        auto_expiry: either(
            a.auto_expiry,
            b.auto_expiry,
            Osc99AutoExpiry::Supported,
            Osc99AutoExpiry::Unsupported,
        ),
        urgency: either(
            a.urgency,
            b.urgency,
            Osc99UrgencySupport::Supported,
            Osc99UrgencySupport::Unsupported,
        ),
        sound_names: either(
            a.sound_names,
            b.sound_names,
            Osc99SoundNames::Standard,
            Osc99SoundNames::SystemAndSilent,
        ),
        body: either(
            a.body,
            b.body,
            Osc99BodySupport::Supported,
            Osc99BodySupport::Unsupported,
        ),
        buttons: either(
            a.buttons,
            b.buttons,
            Osc99ButtonSupport::Supported,
            Osc99ButtonSupport::Unsupported,
        ),
        icons: either(
            a.icons,
            b.icons,
            Osc99IconSupport::Supported,
            Osc99IconSupport::Unsupported,
        ),
    }
}

/// The features present in **both** `a` and `b`. For sound names, `Standard`
/// only if both understand it.
///
/// Exhaustive in the same way as [`union`].
fn intersection(a: Osc99Features, b: Osc99Features) -> Osc99Features {
    Osc99Features {
        activation_report: both(
            a.activation_report,
            b.activation_report,
            Osc99ActivationReport::Reported,
            Osc99ActivationReport::NotReported,
        ),
        close_events: both(
            a.close_events,
            b.close_events,
            Osc99CloseEvents::Reported,
            Osc99CloseEvents::NotReported,
        ),
        activation_focus: both(
            a.activation_focus,
            b.activation_focus,
            Osc99FocusSupport::Supported,
            Osc99FocusSupport::Unsupported,
        ),
        auto_expiry: both(
            a.auto_expiry,
            b.auto_expiry,
            Osc99AutoExpiry::Supported,
            Osc99AutoExpiry::Unsupported,
        ),
        urgency: both(
            a.urgency,
            b.urgency,
            Osc99UrgencySupport::Supported,
            Osc99UrgencySupport::Unsupported,
        ),
        sound_names: both(
            a.sound_names,
            b.sound_names,
            Osc99SoundNames::Standard,
            Osc99SoundNames::SystemAndSilent,
        ),
        body: both(
            a.body,
            b.body,
            Osc99BodySupport::Supported,
            Osc99BodySupport::Unsupported,
        ),
        buttons: both(
            a.buttons,
            b.buttons,
            Osc99ButtonSupport::Supported,
            Osc99ButtonSupport::Unsupported,
        ),
        icons: both(
            a.icons,
            b.icons,
            Osc99IconSupport::Supported,
            Osc99IconSupport::Unsupported,
        ),
    }
}

/// The OSC 99 support `routing` yields on `platform` with `daemon`.
///
/// - `Disabled` is [`Osc99Support::Unsupported`]; `Toast` and `System` use their
///   one leg (`System` with no leg is `Unsupported`).
/// - `Both` shows both legs together. A feature one leg suffices for (activation
///   report, focus, urgency, sound names, body, buttons, icons) is advertised if
///   either leg has it. A per-notification guarantee (close events, auto-expiry)
///   is advertised only if both legs have it. With no system leg, only the toast
///   shows.
/// - `SystemWhenUnfocused` shows one leg or the other depending on focus, so
///   every feature needs both. With no system leg the notification routes to
///   the toast leg at every focus state (as `Both` does), so the toast leg's
///   features are advertised.
///
/// Exposed with `platform` as a parameter so every platform's row is testable
/// on any host.
#[must_use]
pub(super) fn combine(
    routing: NotificationRouting,
    platform: Osc99Platform,
    daemon: Osc99LinuxDaemon,
) -> Osc99Support {
    // No wildcard: a new routing variant must be classified here.
    match routing {
        NotificationRouting::Disabled => Osc99Support::Unsupported,
        NotificationRouting::Toast => Osc99Support::Supported(toast_leg()),
        NotificationRouting::System => {
            system_leg(platform, daemon).map_or(Osc99Support::Unsupported, Osc99Support::Supported)
        }
        // With no system leg both routings show the toast alone.
        NotificationRouting::Both => system_leg(platform, daemon).map_or_else(
            || Osc99Support::Supported(toast_leg()),
            |system| {
                let toast = toast_leg();
                let either = union(toast, system);
                let both = intersection(toast, system);
                Osc99Support::Supported(Osc99Features {
                    close_events: both.close_events,
                    auto_expiry: both.auto_expiry,
                    ..either
                })
            },
        ),
        NotificationRouting::SystemWhenUnfocused => system_leg(platform, daemon).map_or_else(
            || Osc99Support::Supported(toast_leg()),
            |system| Osc99Support::Supported(intersection(toast_leg(), system)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No optional feature: what a leg that cannot be relied on contributes to an
    /// intersection.
    fn no_features() -> Osc99Features {
        Osc99Features {
            activation_report: Osc99ActivationReport::NotReported,
            close_events: Osc99CloseEvents::NotReported,
            activation_focus: Osc99FocusSupport::Unsupported,
            auto_expiry: Osc99AutoExpiry::Unsupported,
            urgency: Osc99UrgencySupport::Unsupported,
            sound_names: Osc99SoundNames::SystemAndSilent,
            body: Osc99BodySupport::Unsupported,
            buttons: Osc99ButtonSupport::Unsupported,
            icons: Osc99IconSupport::Unsupported,
        }
    }

    use crate::gui::desktop_notify::Osc99DaemonCapabilities;
    use crate::gui::panes::{Pane, PaneId};
    use crate::gui::pty::WindowCommandBudget;
    use crate::gui::tabs::{Tab, TabId};
    use crate::gui::title_stack::TitleStack;
    use crate::gui::view_state::ViewState;
    use arc_swap::ArcSwap;
    use crossbeam_channel::Receiver;
    use freminal_common::pty_write::PtyWrite;
    use freminal_terminal_emulator::io::SearchCorpus;
    use freminal_terminal_emulator::snapshot::TerminalSnapshot;
    use freminal_terminal_emulator::state::internal::TerminalState;
    use std::sync::{Arc, atomic::AtomicBool};

    const ALL_ROUTINGS: [NotificationRouting; 5] = [
        NotificationRouting::Toast,
        NotificationRouting::System,
        NotificationRouting::Both,
        NotificationRouting::SystemWhenUnfocused,
        NotificationRouting::Disabled,
    ];
    const ALL_PLATFORMS: [Osc99Platform; 3] = [
        Osc99Platform::LinuxBsd,
        Osc99Platform::MacOs,
        Osc99Platform::Windows,
    ];

    /// A daemon advertising `["body", "actions", "body-markup", "sound"]`.
    fn full_daemon() -> Osc99LinuxDaemon {
        Osc99LinuxDaemon::Available(Osc99DaemonCapabilities {
            sound: Osc99DaemonSupport::Supported,
            ..Osc99DaemonCapabilities::ASSUMED
        })
    }

    /// The assumed daemon: `body` and `actions`, no `sound`.
    fn assumed_daemon() -> Osc99LinuxDaemon {
        Osc99LinuxDaemon::Available(Osc99DaemonCapabilities::ASSUMED)
    }

    /// A daemon advertising `body` only: no `actions`, no `sound`.
    fn body_only_daemon() -> Osc99LinuxDaemon {
        Osc99LinuxDaemon::Available(Osc99DaemonCapabilities {
            actions: Osc99DaemonSupport::Unsupported,
            ..Osc99DaemonCapabilities::ASSUMED
        })
    }

    fn all_daemons() -> [Osc99LinuxDaemon; 4] {
        [
            full_daemon(),
            assumed_daemon(),
            body_only_daemon(),
            Osc99LinuxDaemon::Unavailable,
        ]
    }

    /// A config with notifications and OSC 99 enabled and the given routing.
    fn config_with(routing: NotificationRouting) -> Config {
        let mut config = Config::default();
        config.notifications.enabled = true;
        config.notifications.osc_99 = true;
        config.notifications.routing_osc99 = routing;
        config
    }

    #[test]
    fn host_capabilities_unsupported_when_master_switch_off() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                let mut config = config_with(routing);
                config.notifications.enabled = false;
                assert_eq!(
                    host_capabilities(&config, daemon).osc99,
                    Osc99Support::Unsupported,
                    "enabled = false with routing {routing:?}, daemon {daemon:?}"
                );
            }
            let mut config = config_with(routing);
            config.notifications.enabled = false;
            assert_eq!(
                host_capabilities(&config, Osc99LinuxDaemon::assumed()).osc99,
                Osc99Support::Unsupported
            );
        }
    }

    #[test]
    fn host_capabilities_unsupported_when_osc_99_off() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                let mut config = config_with(routing);
                config.notifications.osc_99 = false;
                assert_eq!(
                    host_capabilities(&config, daemon).osc99,
                    Osc99Support::Unsupported,
                    "osc_99 = false with routing {routing:?}, daemon {daemon:?}"
                );
            }
        }
    }

    #[test]
    fn host_capabilities_default_config_is_unsupported() {
        // `notifications.enabled` defaults to false (opt-in).
        assert_eq!(
            host_capabilities(&Config::default(), Osc99LinuxDaemon::assumed()),
            HostCapabilities::default()
        );
    }

    #[test]
    fn kitty_file_transmission_is_refused_by_default() {
        assert_eq!(
            host_capabilities(&Config::default(), Osc99LinuxDaemon::assumed())
                .kitty_file_transmission,
            KittyFileTransmission::Refused
        );
    }

    #[test]
    fn kitty_file_transmission_follows_the_security_toggle_both_ways() {
        for daemon in all_daemons() {
            let mut config = Config::default();
            config.security.allow_kitty_file_transfer = true;
            assert_eq!(
                host_capabilities(&config, daemon).kitty_file_transmission,
                KittyFileTransmission::Allowed,
                "{daemon:?}"
            );
            config.security.allow_kitty_file_transfer = false;
            assert_eq!(
                host_capabilities(&config, daemon).kitty_file_transmission,
                KittyFileTransmission::Refused,
                "{daemon:?}"
            );
        }
    }

    #[test]
    fn kitty_file_transmission_is_independent_of_the_notification_switches() {
        for routing in ALL_ROUTINGS {
            let mut config = config_with(routing);
            config.security.allow_kitty_file_transfer = true;
            assert_eq!(
                host_capabilities(&config, Osc99LinuxDaemon::assumed()).kitty_file_transmission,
                KittyFileTransmission::Allowed
            );
            config.notifications.enabled = false;
            let caps = host_capabilities(&config, Osc99LinuxDaemon::assumed());
            assert_eq!(caps.osc99, Osc99Support::Unsupported);
            assert_eq!(caps.kitty_file_transmission, KittyFileTransmission::Allowed);
        }
    }

    #[test]
    fn toggling_kitty_file_transfer_is_a_host_capabilities_change() {
        // `apply_new_config` rebroadcasts exactly when the resolved value
        // differs, so the toggle must change it.
        let mut on = Config::default();
        on.security.allow_kitty_file_transfer = true;
        let daemon = Osc99LinuxDaemon::assumed();
        assert_ne!(
            host_capabilities(&on, daemon),
            host_capabilities(&Config::default(), daemon)
        );
    }

    #[test]
    fn host_capabilities_follows_combine_on_the_current_platform() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                assert_eq!(
                    host_capabilities(&config_with(routing), daemon).osc99,
                    combine(routing, Osc99Platform::current(), daemon),
                    "routing {routing:?}, daemon {daemon:?}"
                );
            }
            assert_eq!(
                host_capabilities(&config_with(routing), Osc99LinuxDaemon::assumed()).osc99,
                combine(
                    routing,
                    Osc99Platform::current(),
                    Osc99LinuxDaemon::assumed()
                ),
                "routing {routing:?}, assumed daemon"
            );
        }
    }

    #[test]
    fn toast_leg_capabilities() {
        assert_eq!(
            toast_leg(),
            Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusSupport::Supported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Supported,
                icons: Osc99IconSupport::Unsupported,
            }
        );
    }

    #[test]
    fn disabled_routing_is_unsupported_everywhere() {
        for platform in ALL_PLATFORMS {
            for daemon in all_daemons() {
                assert_eq!(
                    combine(NotificationRouting::Disabled, platform, daemon),
                    Osc99Support::Unsupported,
                    "{platform:?}, {daemon:?}"
                );
            }
        }
    }

    #[test]
    fn toast_routing_is_the_toast_leg_on_every_platform_and_daemon() {
        for platform in ALL_PLATFORMS {
            for daemon in all_daemons() {
                assert_eq!(
                    combine(NotificationRouting::Toast, platform, daemon),
                    Osc99Support::Supported(toast_leg()),
                    "{platform:?}, {daemon:?}"
                );
            }
        }
    }

    #[test]
    fn macos_and_windows_ignore_the_linux_daemon() {
        for platform in [Osc99Platform::MacOs, Osc99Platform::Windows] {
            for routing in ALL_ROUTINGS {
                let reference = combine(routing, platform, assumed_daemon());
                for daemon in all_daemons() {
                    assert_eq!(
                        combine(routing, platform, daemon),
                        reference,
                        "{routing:?} on {platform:?} with {daemon:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn both_on_linux_with_a_full_daemon() {
        // `p=?` reply: a=focus,report, u, p with body/icon/buttons, c=1, w=1,
        // standard sound names.
        assert_eq!(
            combine(
                NotificationRouting::Both,
                Osc99Platform::LinuxBsd,
                full_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusSupport::Supported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::Standard,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Supported,
                icons: Osc99IconSupport::Supported,
            })
        );
    }

    #[test]
    fn both_on_linux_without_standard_sounds_keeps_system_and_silent() {
        let Osc99Support::Supported(features) = combine(
            NotificationRouting::Both,
            Osc99Platform::LinuxBsd,
            assumed_daemon(),
        ) else {
            panic!("expected Supported");
        };
        assert_eq!(features.sound_names, Osc99SoundNames::SystemAndSilent);
    }

    #[test]
    fn both_on_macos_and_windows_keeps_per_notification_guarantees_off() {
        // Either leg suffices for report, focus, urgency and buttons (the toast
        // has them) and icons (the system leg has them); close events and
        // expiry need both legs, and the desktop leg has neither.
        for platform in [Osc99Platform::MacOs, Osc99Platform::Windows] {
            assert_eq!(
                combine(NotificationRouting::Both, platform, assumed_daemon()),
                Osc99Support::Supported(Osc99Features {
                    activation_report: Osc99ActivationReport::Reported,
                    close_events: Osc99CloseEvents::NotReported,
                    activation_focus: Osc99FocusSupport::Supported,
                    auto_expiry: Osc99AutoExpiry::Unsupported,
                    urgency: Osc99UrgencySupport::Supported,
                    sound_names: Osc99SoundNames::SystemAndSilent,
                    body: Osc99BodySupport::Supported,
                    buttons: Osc99ButtonSupport::Supported,
                    icons: Osc99IconSupport::Supported,
                }),
                "{platform:?}"
            );
        }
    }

    #[test]
    fn system_when_unfocused_on_macos_is_the_intersection() {
        // No `a`, `u`, `c` or `w`; a body but no buttons; no icons (the toast
        // shows no transmitted image); only the `system` and `silent` sound
        // names.
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::MacOs,
                assumed_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::NotReported,
                activation_focus: Osc99FocusSupport::Unsupported,
                auto_expiry: Osc99AutoExpiry::Unsupported,
                urgency: Osc99UrgencySupport::Unsupported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Unsupported,
                icons: Osc99IconSupport::Unsupported,
            })
        );
    }

    #[test]
    fn system_when_unfocused_on_linux_with_a_full_daemon() {
        // The toast shows no transmitted icon image and has no standard sound
        // names, so neither can be promised even though the daemon has both.
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::LinuxBsd,
                full_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusSupport::Supported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Supported,
                icons: Osc99IconSupport::Unsupported,
            })
        );
    }

    #[test]
    fn system_on_windows() {
        // No `a`, `c`, `w` or `u` (the Windows toast has no urgency levels,
        // 138.C17); a body and icons; no buttons.
        assert_eq!(
            combine(
                NotificationRouting::System,
                Osc99Platform::Windows,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::NotReported,
                activation_focus: Osc99FocusSupport::Unsupported,
                auto_expiry: Osc99AutoExpiry::Unsupported,
                urgency: Osc99UrgencySupport::Unsupported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Unsupported,
                icons: Osc99IconSupport::Supported,
            })
        );
    }

    #[test]
    fn system_on_macos_honours_no_urgency() {
        let Osc99Support::Supported(features) = combine(
            NotificationRouting::System,
            Osc99Platform::MacOs,
            Osc99LinuxDaemon::Unavailable,
        ) else {
            panic!("expected Supported");
        };
        assert_eq!(features.urgency, Osc99UrgencySupport::Unsupported);
        assert_eq!(features.icons, Osc99IconSupport::Supported);
        assert_eq!(features.buttons, Osc99ButtonSupport::Unsupported);
    }

    #[test]
    fn system_on_linux_without_actions_drops_report_focus_and_buttons() {
        assert_eq!(
            combine(
                NotificationRouting::System,
                Osc99Platform::LinuxBsd,
                body_only_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusSupport::Unsupported,
                auto_expiry: Osc99AutoExpiry::Supported,
                urgency: Osc99UrgencySupport::Supported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Supported,
                buttons: Osc99ButtonSupport::Unsupported,
                icons: Osc99IconSupport::Supported,
            })
        );
    }

    #[test]
    fn system_on_linux_without_body_drops_the_body() {
        let daemon = Osc99LinuxDaemon::Available(Osc99DaemonCapabilities {
            body: Osc99DaemonSupport::Unsupported,
            ..Osc99DaemonCapabilities::ASSUMED
        });
        let Some(features) = system_leg(Osc99Platform::LinuxBsd, daemon) else {
            panic!("expected a system leg");
        };
        assert_eq!(features.body, Osc99BodySupport::Unsupported);
        assert_eq!(features.buttons, Osc99ButtonSupport::Supported);
    }

    #[test]
    fn standard_sound_names_need_the_daemon_sound_capability() {
        let sound_names = |daemon| {
            system_leg(Osc99Platform::LinuxBsd, daemon).map(|features| features.sound_names)
        };
        assert_eq!(sound_names(full_daemon()), Some(Osc99SoundNames::Standard));
        assert_eq!(
            sound_names(assumed_daemon()),
            Some(Osc99SoundNames::SystemAndSilent)
        );
    }

    #[test]
    fn linux_without_a_daemon_has_no_system_leg() {
        assert_eq!(
            system_leg(Osc99Platform::LinuxBsd, Osc99LinuxDaemon::Unavailable),
            None
        );
        assert_eq!(
            combine(
                NotificationRouting::System,
                Osc99Platform::LinuxBsd,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Unsupported
        );
    }

    #[test]
    fn both_on_linux_without_a_daemon_is_the_toast_leg() {
        assert_eq!(
            combine(
                NotificationRouting::Both,
                Osc99Platform::LinuxBsd,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Supported(toast_leg())
        );
    }

    #[test]
    fn system_when_unfocused_on_linux_without_a_daemon_is_the_toast_leg() {
        // The routing falls back to the toast at every focus state, as `Both`
        // does, so the toast leg's features are what is advertised.
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::LinuxBsd,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Supported(toast_leg())
        );
    }

    #[test]
    fn union_and_intersection_are_field_by_field() {
        let toast = toast_leg();
        let none = no_features();
        // Union with nothing is identity; intersection with nothing is nothing.
        assert_eq!(union(toast, none), toast);
        assert_eq!(union(none, toast), toast);
        assert_eq!(intersection(toast, none), none);
        assert_eq!(intersection(none, toast), none);
        // Both are idempotent.
        assert_eq!(union(toast, toast), toast);
        assert_eq!(intersection(toast, toast), toast);
    }

    #[test]
    fn sound_names_are_standard_in_a_union_if_either_and_in_an_intersection_only_if_both() {
        let standard = Osc99Features {
            sound_names: Osc99SoundNames::Standard,
            ..toast_leg()
        };
        let plain = toast_leg();
        assert_eq!(
            union(standard, plain).sound_names,
            Osc99SoundNames::Standard
        );
        assert_eq!(
            union(plain, standard).sound_names,
            Osc99SoundNames::Standard
        );
        assert_eq!(
            union(plain, plain).sound_names,
            Osc99SoundNames::SystemAndSilent
        );
        assert_eq!(
            intersection(standard, plain).sound_names,
            Osc99SoundNames::SystemAndSilent
        );
        assert_eq!(
            intersection(standard, standard).sound_names,
            Osc99SoundNames::Standard
        );
    }
    // ── Daemon state changes (Task 138.15) ──────────────────────────────

    /// A daemon reply that differs from [`Osc99DaemonCapabilities::ASSUMED`]
    /// only in lacking `actions`.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn no_actions_caps() -> Osc99DaemonCapabilities {
        Osc99DaemonCapabilities {
            actions: Osc99DaemonSupport::Unsupported,
            ..Osc99DaemonCapabilities::ASSUMED
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn daemon_change_is_invisible_when_osc_99_is_off_or_toast_only() {
        let mut switched_off = config_with(NotificationRouting::System);
        switched_off.notifications.enabled = false;
        let mut osc_99_off = config_with(NotificationRouting::System);
        osc_99_off.notifications.osc_99 = false;
        for config in [
            config_with(NotificationRouting::Toast),
            config_with(NotificationRouting::Disabled),
            switched_off,
            osc_99_off,
        ] {
            for old in all_daemons() {
                for new in all_daemons() {
                    assert_eq!(
                        daemon_state_change(&config, old, new),
                        None,
                        "{:?}: {old:?} -> {new:?}",
                        config.notifications.routing_osc99
                    );
                }
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn an_unchanged_daemon_changes_nothing() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                assert_eq!(
                    daemon_state_change(&config_with(routing), daemon, daemon),
                    None,
                    "{routing:?}, {daemon:?}"
                );
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_capability_reply_matching_the_assumption_changes_nothing() {
        for routing in ALL_ROUTINGS {
            assert_eq!(
                daemon_state_change(&config_with(routing), assumed_daemon(), assumed_daemon()),
                None,
                "{routing:?}"
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn a_reply_differing_only_in_unadvertised_capabilities_changes_nothing() {
        // `body-markup` only decides whether the service escapes the body; it
        // is not a feature the `p=?` reply carries.
        let markup_less = Osc99LinuxDaemon::Available(Osc99DaemonCapabilities {
            body_markup: Osc99DaemonSupport::Unsupported,
            ..Osc99DaemonCapabilities::ASSUMED
        });
        for routing in ALL_ROUTINGS {
            assert_eq!(
                daemon_state_change(&config_with(routing), assumed_daemon(), markup_less),
                None,
                "{routing:?}"
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn system_routing_without_actions_drops_report_focus_and_buttons() {
        let config = config_with(NotificationRouting::System);
        let reply = Osc99LinuxDaemon::Available(no_actions_caps());
        let Some(HostCapabilities {
            osc99: Osc99Support::Supported(features),
            ..
        }) = daemon_state_change(&config, assumed_daemon(), reply)
        else {
            panic!("a reply without actions must change a System-routed pane's capabilities");
        };
        assert_eq!(
            features.activation_report,
            Osc99ActivationReport::NotReported
        );
        assert_eq!(features.activation_focus, Osc99FocusSupport::Unsupported);
        assert_eq!(features.buttons, Osc99ButtonSupport::Unsupported);
        // Everything the daemon still offers stays.
        assert_eq!(features.close_events, Osc99CloseEvents::Reported);
        assert_eq!(features.body, Osc99BodySupport::Supported);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn system_routing_with_the_bus_down_is_unsupported() {
        let config = config_with(NotificationRouting::System);
        assert_eq!(
            daemon_state_change(&config, assumed_daemon(), Osc99LinuxDaemon::Unavailable),
            Some(HostCapabilities {
                osc99: Osc99Support::Unsupported,
                ..HostCapabilities::default()
            })
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn both_routing_with_the_bus_down_is_the_toast_leg_only() {
        let config = config_with(NotificationRouting::Both);
        assert_eq!(
            daemon_state_change(&config, full_daemon(), Osc99LinuxDaemon::Unavailable),
            Some(HostCapabilities {
                osc99: Osc99Support::Supported(toast_leg()),
                ..HostCapabilities::default()
            })
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn system_when_unfocused_with_the_bus_down_is_the_toast_leg() {
        let config = config_with(NotificationRouting::SystemWhenUnfocused);
        // A daemon without `actions` advertises less than the toast leg, so
        // losing it (the toast then shows at every focus state) is a change.
        assert_eq!(
            daemon_state_change(&config, body_only_daemon(), Osc99LinuxDaemon::Unavailable),
            Some(HostCapabilities {
                osc99: Osc99Support::Supported(toast_leg()),
                ..HostCapabilities::default()
            })
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_bus_coming_back_restores_the_capabilities() {
        let config = config_with(NotificationRouting::System);
        assert_eq!(
            daemon_state_change(&config, Osc99LinuxDaemon::Unavailable, assumed_daemon()),
            Some(host_capabilities(&config, assumed_daemon()))
        );
    }

    #[test]
    fn a_daemon_change_never_alters_the_macos_and_windows_rows() {
        // `daemon_state_change` is `host_capabilities` before and after, and
        // `host_capabilities` follows `combine`, which ignores the daemon off
        // Linux/BSD, so no change can ever be reported there.
        for platform in [Osc99Platform::MacOs, Osc99Platform::Windows] {
            for routing in ALL_ROUTINGS {
                for old in all_daemons() {
                    for new in all_daemons() {
                        assert_eq!(
                            combine(routing, platform, old),
                            combine(routing, platform, new),
                            "{routing:?} on {platform:?}: {old:?} -> {new:?}"
                        );
                    }
                }
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_system_leg_is_wanted_only_when_osc_99_is_on_and_routing_uses_it() {
        for routing in ALL_ROUTINGS {
            let expected = matches!(
                routing,
                NotificationRouting::System
                    | NotificationRouting::Both
                    | NotificationRouting::SystemWhenUnfocused
            );
            assert_eq!(
                osc99_may_use_system_leg(&config_with(routing)),
                expected,
                "{routing:?}"
            );
            let mut switched_off = config_with(routing);
            switched_off.notifications.enabled = false;
            assert!(!osc99_may_use_system_leg(&switched_off), "{routing:?}");
            let mut osc_99_off = config_with(routing);
            osc_99_off.notifications.osc_99 = false;
            assert!(!osc99_may_use_system_leg(&osc_99_off), "{routing:?}");
        }
        assert!(!osc99_may_use_system_leg(&Config::default()));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_system_leg_of_a_routing_is_every_variant_but_toast_and_disabled() {
        for routing in ALL_ROUTINGS {
            assert_eq!(
                routing_has_system_leg(routing),
                !matches!(
                    routing,
                    NotificationRouting::Toast | NotificationRouting::Disabled
                ),
                "{routing:?}"
            );
        }
    }

    /// A `[notifications]` config with every routing toast-only, so a test
    /// turns on exactly the categories it is about.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn toast_only_notifications() -> NotificationsConfig {
        NotificationsConfig {
            enabled: true,
            routing_osc_text: NotificationRouting::Toast,
            routing_command_finished: NotificationRouting::Toast,
            routing_error: NotificationRouting::Toast,
            ..NotificationsConfig::default()
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn routed_plain_notifications_need_the_master_switch_and_a_system_routing() {
        assert!(!plain_notifications_may_use_system_leg(
            &toast_only_notifications(),
            PlainNotifyEntry::Routed
        ));
        for routing in ALL_ROUTINGS {
            let expected = routing_has_system_leg(routing);
            for category in 0..3 {
                let mut config = toast_only_notifications();
                match category {
                    0 => config.routing_osc_text = routing,
                    1 => config.routing_command_finished = routing,
                    _ => config.routing_error = routing,
                }
                assert_eq!(
                    plain_notifications_may_use_system_leg(&config, PlainNotifyEntry::Routed),
                    expected,
                    "category {category}: {routing:?}"
                );
                config.enabled = false;
                assert!(
                    !plain_notifications_may_use_system_leg(&config, PlainNotifyEntry::Routed),
                    "the master switch is off: category {category}: {routing:?}"
                );
            }
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_test_button_ignores_the_master_switch_and_reads_only_the_osc_text_routing() {
        for routing in ALL_ROUTINGS {
            for enabled in [true, false] {
                let mut config = toast_only_notifications();
                config.enabled = enabled;
                config.routing_osc_text = routing;
                assert_eq!(
                    plain_notifications_may_use_system_leg(&config, PlainNotifyEntry::SettingsTest),
                    routing_has_system_leg(routing),
                    "{routing:?}, enabled = {enabled}"
                );
            }
            // The other categories do not matter to the sample request.
            let mut config = toast_only_notifications();
            config.routing_command_finished = routing;
            config.routing_error = routing;
            assert!(
                !plain_notifications_may_use_system_leg(&config, PlainNotifyEntry::SettingsTest),
                "{routing:?}"
            );
        }
    }

    /// A config with notifications on and every routing toast-only, OSC 99
    /// included, so a test turns on exactly the legs it is about.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn all_toast_config() -> Config {
        Config {
            notifications: NotificationsConfig {
                osc_99: true,
                routing_osc99: NotificationRouting::Toast,
                ..toast_only_notifications()
            },
            ..Config::default()
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_service_is_wanted_when_any_routed_notification_can_use_the_system_leg() {
        assert!(!notification_service_wanted(&Config::default()));
        assert!(!notification_service_wanted(&all_toast_config()));

        // OSC 99 is off, but the error category can show a system notification.
        let mut osc_99_off = all_toast_config();
        osc_99_off.notifications.osc_99 = false;
        osc_99_off.notifications.routing_osc99 = NotificationRouting::Toast;
        osc_99_off.notifications.routing_error = NotificationRouting::System;
        assert!(notification_service_wanted(&osc_99_off));

        // OSC 99 is toast-only, but OSC 9 / 777 text can show a system one.
        let mut osc_text = all_toast_config();
        osc_text.notifications.routing_osc_text = NotificationRouting::System;
        assert!(notification_service_wanted(&osc_text));

        // Only OSC 99 uses the system leg.
        let mut osc_99_alone = all_toast_config();
        osc_99_alone.notifications.routing_osc99 = NotificationRouting::System;
        assert!(notification_service_wanted(&osc_99_alone));

        // The master switch off silences every one of them.
        for mut config in [osc_99_off, osc_text, osc_99_alone] {
            config.notifications.enabled = false;
            assert!(!notification_service_wanted(&config));
        }
        let everything_system = Config {
            notifications: NotificationsConfig {
                enabled: false,
                osc_99: true,
                routing_osc99: NotificationRouting::System,
                routing_osc_text: NotificationRouting::System,
                routing_command_finished: NotificationRouting::System,
                routing_error: NotificationRouting::System,
                ..NotificationsConfig::default()
            },
            ..Config::default()
        };
        assert!(!notification_service_wanted(&everything_system));
    }

    // ── End to end through a pane's handler (Task 138.15) ───────────────

    /// A pane's terminal state seeded with `capabilities`, and its PTY write
    /// receiver.
    fn pane_state(
        capabilities: HostCapabilities,
    ) -> (TerminalState, crossbeam_channel::Receiver<PtyWrite>) {
        let (tx, rx) = crossbeam_channel::unbounded::<PtyWrite>();
        let mut state = TerminalState::new(tx, None);
        state.handler.set_host_capabilities(capabilities);
        (state, rx)
    }

    /// What the pane writes to the PTY after a `p=?` query.
    fn query(state: &mut TerminalState, rx: &crossbeam_channel::Receiver<PtyWrite>) -> String {
        state.handle_incoming_data(b"\x1b]99;i=q:p=?;\x1b\\");
        let mut bytes = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let PtyWrite::Write(written) = message {
                bytes.extend_from_slice(&written);
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    /// The Linux/BSD capabilities of `routing` with `daemon`, on any host.
    fn linux_capabilities(
        routing: NotificationRouting,
        daemon: Osc99LinuxDaemon,
    ) -> HostCapabilities {
        HostCapabilities {
            osc99: combine(routing, Osc99Platform::LinuxBsd, daemon),
            ..HostCapabilities::default()
        }
    }

    #[test]
    fn a_reply_without_actions_removes_a_and_buttons_from_the_next_query() {
        let routing = NotificationRouting::System;
        let (mut state, rx) = pane_state(linux_capabilities(routing, assumed_daemon()));

        assert_eq!(
            query(&mut state, &rx),
            "\x1b]99;i=q:p=?;a=focus,report:o=always,unfocused,invisible:u=0,1,2:\
             p=title,body,?,close,icon,alive,buttons:c=1:w=1:s=system,silent\x1b\\",
            "before the daemon answers, the assumed capabilities are advertised"
        );

        // The daemon answers without `actions`; the pane gets the recomputed
        // capabilities exactly as `InputEvent::HostCapabilitiesChange` delivers
        // them (the PTY thread calls `set_host_capabilities`).
        state
            .handler
            .set_host_capabilities(linux_capabilities(routing, body_only_daemon()));

        assert_eq!(
            query(&mut state, &rx),
            "\x1b]99;i=q:p=?;o=always,unfocused,invisible:u=0,1,2:\
             p=title,body,?,close,icon,alive:c=1:w=1:s=system,silent\x1b\\"
        );
    }

    #[test]
    fn an_unavailable_bus_makes_a_system_routed_pane_answer_no_query() {
        let routing = NotificationRouting::System;
        let (mut state, rx) = pane_state(linux_capabilities(routing, assumed_daemon()));
        assert!(!query(&mut state, &rx).is_empty(), "sanity: it answered");

        state
            .handler
            .set_host_capabilities(linux_capabilities(routing, Osc99LinuxDaemon::Unavailable));

        assert_eq!(query(&mut state, &rx), "");
    }

    #[test]
    fn an_unavailable_bus_makes_a_both_routed_pane_advertise_the_toast_leg_only() {
        let routing = NotificationRouting::Both;
        let (mut state, rx) = pane_state(linux_capabilities(routing, full_daemon()));
        assert!(
            query(&mut state, &rx).contains("icon"),
            "sanity: the system leg's icons were advertised"
        );

        state
            .handler
            .set_host_capabilities(linux_capabilities(routing, Osc99LinuxDaemon::Unavailable));

        // The toast leg: focus and report, buttons, no icons, `system`/`silent`
        // sounds only.
        assert_eq!(
            query(&mut state, &rx),
            "\x1b]99;i=q:p=?;a=focus,report:o=always,unfocused,invisible:u=0,1,2:\
             p=title,body,?,close,alive,buttons:c=1:w=1:s=system,silent\x1b\\"
        );
    }

    // ── Broadcast to every pane, in-flight window included ──────────────

    /// A tab with one pane whose input receiver is returned.
    fn tab_with_input_rx(id: TabId, pane_id: PaneId) -> (Tab, Receiver<InputEvent>) {
        let arc_swap = Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        let (pty_write_tx, _pty_write_rx) = crossbeam_channel::unbounded();
        let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
        let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
        let (_search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<SearchCorpus>(1);
        let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);
        let (_command_event_tx, command_event_rx) = crossbeam_channel::unbounded();

        let pane = Pane {
            id: pane_id,
            arc_swap,
            reply_tx: Arc::new(input_tx.clone()),
            input_tx,
            pty_write_tx,
            window_cmd_rx,
            clipboard_rx,
            search_buffer_rx,
            pty_dead_rx,
            command_event_rx,
            recent_commands: std::collections::VecDeque::new(),
            history_seed: crate::gui::shell_history::new_seeded_history(),
            shell_program: None,
            shell_histfile_last_seen: None,
            command_texts: std::collections::HashMap::new(),
            title: format!("pane-{pane_id}"),
            bell_active: false,
            pending_copy: false,
            title_stack: TitleStack::new(),
            view_state: ViewState::new(),
            echo_off: Arc::new(AtomicBool::new(false)),
            window_cmd_budget: WindowCommandBudget::default(),
            child_pid: None,
            render_state: crate::gui::terminal::new_render_state(Arc::new(std::sync::Mutex::new(
                crate::gui::renderer::WindowPostRenderer::new(),
            ))),
            render_cache: crate::gui::terminal::PaneRenderCache::new(),
        };

        (Tab::new(id, pane), input_rx)
    }

    /// A window of `count` tabs (one pane each) and the panes' receivers.
    fn window_of(
        ids: &mut crate::gui::panes::PaneIdGenerator,
        count: u64,
    ) -> (TabManager, Vec<Receiver<InputEvent>>) {
        let (first, rx) = tab_with_input_rx(TabId::first(), ids.next_id());
        let mut receivers = vec![rx];
        let mut tabs = TabManager::new(first);
        for n in 1..count {
            let (tab, rx) = tab_with_input_rx(TabId::offset(n), ids.next_id());
            tabs.add_tab(tab);
            receivers.push(rx);
        }
        (tabs, receivers)
    }

    /// The `HostCapabilitiesChange` payloads waiting on `rx`.
    fn received(rx: &Receiver<InputEvent>) -> Vec<HostCapabilities> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                InputEvent::HostCapabilitiesChange(caps) => out.push(caps),
                other => panic!("unexpected event {other:?}"),
            }
        }
        out
    }

    #[test]
    fn broadcast_reaches_every_pane_of_every_window_and_the_in_flight_window() {
        let mut ids = crate::gui::panes::PaneIdGenerator::new(0);
        let (in_map_a, rxs_a) = window_of(&mut ids, 2);
        let (in_map_b, rxs_b) = window_of(&mut ids, 1);
        let (in_flight, rxs_flight) = window_of(&mut ids, 3);
        let capabilities = HostCapabilities {
            osc99: Osc99Support::Supported(toast_leg()),
            ..HostCapabilities::default()
        };

        send_host_capabilities([&in_map_a, &in_map_b], Some(&in_flight), capabilities);

        for rx in rxs_a.iter().chain(&rxs_b).chain(&rxs_flight) {
            assert_eq!(received(rx), vec![capabilities], "exactly one event");
        }
    }

    #[test]
    fn broadcast_without_an_in_flight_window_reaches_only_the_windows_given() {
        let mut ids = crate::gui::panes::PaneIdGenerator::new(0);
        let (in_map, rxs) = window_of(&mut ids, 2);
        let (_untouched, untouched_rxs) = window_of(&mut ids, 1);
        let capabilities = HostCapabilities::default();

        send_host_capabilities([&in_map], None, capabilities);

        for rx in &rxs {
            assert_eq!(received(rx), vec![capabilities]);
        }
        for rx in &untouched_rxs {
            assert_eq!(received(rx), Vec::<HostCapabilities>::new());
        }
    }

    #[test]
    fn broadcast_survives_a_pane_whose_pty_thread_is_gone() {
        let mut ids = crate::gui::panes::PaneIdGenerator::new(0);
        let (dead, dead_rxs) = window_of(&mut ids, 1);
        let (alive, alive_rxs) = window_of(&mut ids, 1);
        drop(dead_rxs);
        let capabilities = HostCapabilities::default();

        send_host_capabilities([&dead, &alive], None, capabilities);

        assert_eq!(received(&alive_rxs[0]), vec![capabilities]);
    }
}
