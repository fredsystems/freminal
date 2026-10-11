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
    HostCapabilities, Osc99ActivationReport, Osc99CloseEvents, Osc99Features, Osc99Support,
};

/// Resolve the host-dependent capability facts from the config and platform.
///
/// The PTY thread cannot know these itself (Task 130.4); the GUI computes this
/// value at pane spawn and re-sends it via `InputEvent::HostCapabilitiesChange`
/// whenever it changes.
///
/// OSC 99 is [`Osc99Support::Unsupported`] unless the master switch
/// (`notifications.enabled`), the per-protocol switch (`notifications.osc_99`)
/// and a non-`Disabled` `routing_osc99` are all in place. Otherwise:
///
/// - **Close events** are reportable whenever the routing includes a system
///   (desktop) leg: with the system leg freminal emits either an observed
///   close (Linux/BSD) or an untracked one (macOS/Windows). A toast-only
///   routing has no desktop notification whose close could be reported.
/// - **Activation** is reportable only when the system leg is possible *and*
///   the platform is Linux/BSD: the activation callback exists only on the
///   D-Bus backend of `notify-rust` (see `show_system_osc99`); macOS and
///   Windows have no background-thread activation callback.
pub(super) const fn host_capabilities(config: &Config) -> HostCapabilities {
    let notifications = &config.notifications;
    if !(notifications.enabled && notifications.osc_99) {
        return HostCapabilities {
            osc99: Osc99Support::Unsupported,
        };
    }

    // No wildcard: a new routing variant must be classified here.
    let system_leg_possible = match notifications.routing_osc99 {
        NotificationRouting::Disabled => {
            return HostCapabilities {
                osc99: Osc99Support::Unsupported,
            };
        }
        NotificationRouting::System
        | NotificationRouting::Both
        | NotificationRouting::SystemWhenUnfocused => true,
        NotificationRouting::Toast => false,
    };

    let activation_report = if system_leg_possible && cfg!(all(unix, not(target_os = "macos"))) {
        Osc99ActivationReport::Reported
    } else {
        Osc99ActivationReport::NotReported
    };
    let close_events = if system_leg_possible {
        Osc99CloseEvents::Reported
    } else {
        Osc99CloseEvents::NotReported
    };

    HostCapabilities {
        osc99: Osc99Support::Supported(Osc99Features {
            activation_report,
            close_events,
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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
        for routing in [
            NotificationRouting::Toast,
            NotificationRouting::System,
            NotificationRouting::Both,
            NotificationRouting::SystemWhenUnfocused,
            NotificationRouting::Disabled,
        ] {
            let mut config = config_with(routing);
            config.notifications.enabled = false;
            assert_eq!(
                host_capabilities(&config).osc99,
                Osc99Support::Unsupported,
                "enabled = false with routing {routing:?}"
            );
        }
    }

    #[test]
    fn host_capabilities_unsupported_when_osc_99_off() {
        for routing in [
            NotificationRouting::Toast,
            NotificationRouting::System,
            NotificationRouting::Both,
            NotificationRouting::SystemWhenUnfocused,
            NotificationRouting::Disabled,
        ] {
            let mut config = config_with(routing);
            config.notifications.osc_99 = false;
            assert_eq!(
                host_capabilities(&config).osc99,
                Osc99Support::Unsupported,
                "osc_99 = false with routing {routing:?}"
            );
        }
    }

    #[test]
    fn host_capabilities_routing_truth_table() {
        let linux_bsd = cfg!(all(unix, not(target_os = "macos")));
        let with_system_leg = Osc99Support::Supported(Osc99Features {
            activation_report: if linux_bsd {
                Osc99ActivationReport::Reported
            } else {
                Osc99ActivationReport::NotReported
            },
            close_events: Osc99CloseEvents::Reported,
        });
        let toast_only = Osc99Support::Supported(Osc99Features {
            activation_report: Osc99ActivationReport::NotReported,
            close_events: Osc99CloseEvents::NotReported,
        });

        let cases = [
            (NotificationRouting::Toast, toast_only),
            (NotificationRouting::System, with_system_leg),
            (NotificationRouting::Both, with_system_leg),
            (NotificationRouting::SystemWhenUnfocused, with_system_leg),
            (NotificationRouting::Disabled, Osc99Support::Unsupported),
        ];
        for (routing, expected) in cases {
            assert_eq!(
                host_capabilities(&config_with(routing)).osc99,
                expected,
                "routing {routing:?}"
            );
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
}
