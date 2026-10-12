// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Host-dependent capability facts for the PTY thread.
//!
//! Resolves, from the config and the platform, the capability facts the PTY
//! thread cannot know by itself (Task 130.4). Later facets (Tasks 138, 146)
//! are added here.

use freminal_common::config::{Config, NotificationRouting};
use freminal_common::host_capabilities::{
    HostCapabilities, Osc99ActivationReport, Osc99AutoExpiry, Osc99BodySupport, Osc99ButtonSupport,
    Osc99CloseEvents, Osc99Features, Osc99FocusAction, Osc99IconSupport, Osc99SoundNames,
    Osc99Support, Osc99UrgencySupport,
};

use super::osc99::{Osc99DaemonSupport, Osc99LinuxDaemon, Osc99Platform};

/// Resolve the host-dependent capability facts from the config and platform,
/// assuming the Linux/BSD notification daemon is as capable as kitty assumes
/// until it answers ([`Osc99LinuxDaemon::assumed`]).
///
/// See [`host_capabilities_with_daemon`].
pub(super) const fn host_capabilities(config: &Config) -> HostCapabilities {
    host_capabilities_with_daemon(config, Osc99LinuxDaemon::assumed())
}

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
pub(super) const fn host_capabilities_with_daemon(
    config: &Config,
    daemon: Osc99LinuxDaemon,
) -> HostCapabilities {
    let notifications = &config.notifications;
    if !(notifications.enabled && notifications.osc_99) {
        return HostCapabilities {
            osc99: Osc99Support::Unsupported,
        };
    }
    HostCapabilities {
        osc99: combine(
            notifications.routing_osc99,
            Osc99Platform::current(),
            daemon,
        ),
    }
}

/// The in-window toast leg's capabilities. They do not depend on the platform
/// or the daemon.
///
/// The toast reports activation and close, focuses the window on activation,
/// honours expiry and urgency, and shows a body and buttons. It shows no
/// icons, and understands only the `system` and `silent` sound names.
#[must_use]
pub(super) const fn toast_leg() -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::Reported,
        close_events: Osc99CloseEvents::Reported,
        activation_focus: Osc99FocusAction::Supported,
        auto_expiry: Osc99AutoExpiry::Supported,
        urgency: Osc99UrgencySupport::Supported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Supported,
        buttons: Osc99ButtonSupport::Supported,
        icons: Osc99IconSupport::Unsupported,
    }
}

/// No optional feature: what a leg that cannot be relied on contributes to an
/// intersection.
const fn no_features() -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        close_events: Osc99CloseEvents::NotReported,
        activation_focus: Osc99FocusAction::Unsupported,
        auto_expiry: Osc99AutoExpiry::Unsupported,
        urgency: Osc99UrgencySupport::Unsupported,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Unsupported,
        buttons: Osc99ButtonSupport::Unsupported,
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
/// - **Windows** is the same as macOS, except that it honours urgency.
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
                        Osc99FocusAction::Supported
                    } else {
                        Osc99FocusAction::Unsupported
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
        Osc99Platform::MacOs => Some(untracked_system_leg(Osc99UrgencySupport::Unsupported)),
        Osc99Platform::Windows => Some(untracked_system_leg(Osc99UrgencySupport::Supported)),
    }
}

/// The macOS / Windows system leg, which differ only in urgency.
const fn untracked_system_leg(urgency: Osc99UrgencySupport) -> Osc99Features {
    Osc99Features {
        activation_report: Osc99ActivationReport::NotReported,
        close_events: Osc99CloseEvents::NotReported,
        activation_focus: Osc99FocusAction::Unsupported,
        auto_expiry: Osc99AutoExpiry::Unsupported,
        urgency,
        sound_names: Osc99SoundNames::SystemAndSilent,
        body: Osc99BodySupport::Supported,
        buttons: Osc99ButtonSupport::Unsupported,
        icons: Osc99IconSupport::Supported,
    }
}

/// The features present in **either** `a` or `b`. For sound names, `Standard`
/// if either understands it.
const fn union(a: Osc99Features, b: Osc99Features) -> Osc99Features {
    Osc99Features {
        activation_report: if matches!(a.activation_report, Osc99ActivationReport::Reported)
            || matches!(b.activation_report, Osc99ActivationReport::Reported)
        {
            Osc99ActivationReport::Reported
        } else {
            Osc99ActivationReport::NotReported
        },
        close_events: if matches!(a.close_events, Osc99CloseEvents::Reported)
            || matches!(b.close_events, Osc99CloseEvents::Reported)
        {
            Osc99CloseEvents::Reported
        } else {
            Osc99CloseEvents::NotReported
        },
        activation_focus: if matches!(a.activation_focus, Osc99FocusAction::Supported)
            || matches!(b.activation_focus, Osc99FocusAction::Supported)
        {
            Osc99FocusAction::Supported
        } else {
            Osc99FocusAction::Unsupported
        },
        auto_expiry: if matches!(a.auto_expiry, Osc99AutoExpiry::Supported)
            || matches!(b.auto_expiry, Osc99AutoExpiry::Supported)
        {
            Osc99AutoExpiry::Supported
        } else {
            Osc99AutoExpiry::Unsupported
        },
        urgency: if matches!(a.urgency, Osc99UrgencySupport::Supported)
            || matches!(b.urgency, Osc99UrgencySupport::Supported)
        {
            Osc99UrgencySupport::Supported
        } else {
            Osc99UrgencySupport::Unsupported
        },
        sound_names: if matches!(a.sound_names, Osc99SoundNames::Standard)
            || matches!(b.sound_names, Osc99SoundNames::Standard)
        {
            Osc99SoundNames::Standard
        } else {
            Osc99SoundNames::SystemAndSilent
        },
        body: if matches!(a.body, Osc99BodySupport::Supported)
            || matches!(b.body, Osc99BodySupport::Supported)
        {
            Osc99BodySupport::Supported
        } else {
            Osc99BodySupport::Unsupported
        },
        buttons: if matches!(a.buttons, Osc99ButtonSupport::Supported)
            || matches!(b.buttons, Osc99ButtonSupport::Supported)
        {
            Osc99ButtonSupport::Supported
        } else {
            Osc99ButtonSupport::Unsupported
        },
        icons: if matches!(a.icons, Osc99IconSupport::Supported)
            || matches!(b.icons, Osc99IconSupport::Supported)
        {
            Osc99IconSupport::Supported
        } else {
            Osc99IconSupport::Unsupported
        },
    }
}

/// The features present in **both** `a` and `b`. For sound names, `Standard`
/// only if both understand it.
const fn intersection(a: Osc99Features, b: Osc99Features) -> Osc99Features {
    Osc99Features {
        activation_report: if matches!(a.activation_report, Osc99ActivationReport::Reported)
            && matches!(b.activation_report, Osc99ActivationReport::Reported)
        {
            Osc99ActivationReport::Reported
        } else {
            Osc99ActivationReport::NotReported
        },
        close_events: if matches!(a.close_events, Osc99CloseEvents::Reported)
            && matches!(b.close_events, Osc99CloseEvents::Reported)
        {
            Osc99CloseEvents::Reported
        } else {
            Osc99CloseEvents::NotReported
        },
        activation_focus: if matches!(a.activation_focus, Osc99FocusAction::Supported)
            && matches!(b.activation_focus, Osc99FocusAction::Supported)
        {
            Osc99FocusAction::Supported
        } else {
            Osc99FocusAction::Unsupported
        },
        auto_expiry: if matches!(a.auto_expiry, Osc99AutoExpiry::Supported)
            && matches!(b.auto_expiry, Osc99AutoExpiry::Supported)
        {
            Osc99AutoExpiry::Supported
        } else {
            Osc99AutoExpiry::Unsupported
        },
        urgency: if matches!(a.urgency, Osc99UrgencySupport::Supported)
            && matches!(b.urgency, Osc99UrgencySupport::Supported)
        {
            Osc99UrgencySupport::Supported
        } else {
            Osc99UrgencySupport::Unsupported
        },
        sound_names: if matches!(a.sound_names, Osc99SoundNames::Standard)
            && matches!(b.sound_names, Osc99SoundNames::Standard)
        {
            Osc99SoundNames::Standard
        } else {
            Osc99SoundNames::SystemAndSilent
        },
        body: if matches!(a.body, Osc99BodySupport::Supported)
            && matches!(b.body, Osc99BodySupport::Supported)
        {
            Osc99BodySupport::Supported
        } else {
            Osc99BodySupport::Unsupported
        },
        buttons: if matches!(a.buttons, Osc99ButtonSupport::Supported)
            && matches!(b.buttons, Osc99ButtonSupport::Supported)
        {
            Osc99ButtonSupport::Supported
        } else {
            Osc99ButtonSupport::Unsupported
        },
        icons: if matches!(a.icons, Osc99IconSupport::Supported)
            && matches!(b.icons, Osc99IconSupport::Supported)
        {
            Osc99IconSupport::Supported
        } else {
            Osc99IconSupport::Unsupported
        },
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
///   every feature needs both. With no system leg the toast still shows while
///   focused but the failing system leg shows while unfocused, so no optional
///   feature is guaranteed.
///
/// Exposed with `platform` as a parameter so every platform's row is testable
/// on any host.
#[must_use]
pub(super) const fn combine(
    routing: NotificationRouting,
    platform: Osc99Platform,
    daemon: Osc99LinuxDaemon,
) -> Osc99Support {
    // No wildcard: a new routing variant must be classified here.
    match routing {
        NotificationRouting::Disabled => Osc99Support::Unsupported,
        NotificationRouting::Toast => Osc99Support::Supported(toast_leg()),
        NotificationRouting::System => match system_leg(platform, daemon) {
            Some(system) => Osc99Support::Supported(system),
            None => Osc99Support::Unsupported,
        },
        NotificationRouting::Both => match system_leg(platform, daemon) {
            Some(system) => {
                let toast = toast_leg();
                let either = union(toast, system);
                let both = intersection(toast, system);
                Osc99Support::Supported(Osc99Features {
                    close_events: both.close_events,
                    auto_expiry: both.auto_expiry,
                    ..either
                })
            }
            None => Osc99Support::Supported(toast_leg()),
        },
        NotificationRouting::SystemWhenUnfocused => match system_leg(platform, daemon) {
            Some(system) => Osc99Support::Supported(intersection(toast_leg(), system)),
            None => Osc99Support::Supported(no_features()),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::osc99::Osc99DaemonCapabilities;

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
                    host_capabilities_with_daemon(&config, daemon).osc99,
                    Osc99Support::Unsupported,
                    "enabled = false with routing {routing:?}, daemon {daemon:?}"
                );
            }
            let mut config = config_with(routing);
            config.notifications.enabled = false;
            assert_eq!(host_capabilities(&config).osc99, Osc99Support::Unsupported);
        }
    }

    #[test]
    fn host_capabilities_unsupported_when_osc_99_off() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                let mut config = config_with(routing);
                config.notifications.osc_99 = false;
                assert_eq!(
                    host_capabilities_with_daemon(&config, daemon).osc99,
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
            host_capabilities(&Config::default()),
            HostCapabilities::default()
        );
    }

    #[test]
    fn host_capabilities_follows_combine_on_the_current_platform() {
        for routing in ALL_ROUTINGS {
            for daemon in all_daemons() {
                assert_eq!(
                    host_capabilities_with_daemon(&config_with(routing), daemon).osc99,
                    combine(routing, Osc99Platform::current(), daemon),
                    "routing {routing:?}, daemon {daemon:?}"
                );
            }
            assert_eq!(
                host_capabilities(&config_with(routing)).osc99,
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
                activation_focus: Osc99FocusAction::Supported,
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
                activation_focus: Osc99FocusAction::Supported,
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
                    activation_focus: Osc99FocusAction::Supported,
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
        // shows none); only the `system` and `silent` sound names.
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::MacOs,
                assumed_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::NotReported,
                activation_focus: Osc99FocusAction::Unsupported,
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
        // The toast has no icons and no standard sound names, so neither can
        // be promised even though the daemon has both.
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::LinuxBsd,
                full_daemon()
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::Reported,
                close_events: Osc99CloseEvents::Reported,
                activation_focus: Osc99FocusAction::Supported,
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
        // No `a`, `c` or `w`; urgency, a body and icons; no buttons.
        assert_eq!(
            combine(
                NotificationRouting::System,
                Osc99Platform::Windows,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::NotReported,
                activation_focus: Osc99FocusAction::Unsupported,
                auto_expiry: Osc99AutoExpiry::Unsupported,
                urgency: Osc99UrgencySupport::Supported,
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
                activation_focus: Osc99FocusAction::Unsupported,
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
    fn system_when_unfocused_on_linux_without_a_daemon_promises_nothing() {
        assert_eq!(
            combine(
                NotificationRouting::SystemWhenUnfocused,
                Osc99Platform::LinuxBsd,
                Osc99LinuxDaemon::Unavailable
            ),
            Osc99Support::Supported(Osc99Features {
                activation_report: Osc99ActivationReport::NotReported,
                close_events: Osc99CloseEvents::NotReported,
                activation_focus: Osc99FocusAction::Unsupported,
                auto_expiry: Osc99AutoExpiry::Unsupported,
                urgency: Osc99UrgencySupport::Unsupported,
                sound_names: Osc99SoundNames::SystemAndSilent,
                body: Osc99BodySupport::Unsupported,
                buttons: Osc99ButtonSupport::Unsupported,
                icons: Osc99IconSupport::Unsupported,
            })
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
}
