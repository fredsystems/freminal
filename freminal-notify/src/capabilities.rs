// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The facts about the host that decide which OSC 99 features the system
//! (desktop-notification) leg offers (Task 138.17).
//!
//! These types exist on every platform so the capability tables in
//! `host_capabilities` can be built, and tested, everywhere: the Linux/BSD
//! daemon's advertised capabilities ([`DaemonCapabilities`]), whether that
//! daemon is reachable at all ([`LinuxDaemon`]), and which platform's
//! system leg applies ([`NotifyPlatform`]).

/// Whether the notification daemon has a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonSupport {
    /// The daemon advertises the capability (or it is assumed, before the
    /// daemon has answered).
    Supported,
    /// The daemon does not advertise the capability.
    Unsupported,
}

/// The `GetCapabilities` entries the OSC 99 system leg cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonCapabilities {
    /// `body`: the daemon shows a notification body.
    pub body: DaemonSupport,
    /// `actions`: the daemon shows and reports actions (buttons and the
    /// default action).
    pub actions: DaemonSupport,
    /// `body-markup`: the daemon renders markup in the body. Only the Linux/BSD
    /// service reads it, to decide whether to escape the body.
    pub body_markup: DaemonSupport,
    /// `sound`: the daemon plays notification sounds.
    pub sound: DaemonSupport,
}

impl DaemonCapabilities {
    /// What is assumed until the daemon answers `GetCapabilities`: kitty
    /// assumes `body`, `actions` and `body-markup`, and no `sound`.
    pub const ASSUMED: Self = Self {
        body: DaemonSupport::Supported,
        actions: DaemonSupport::Supported,
        body_markup: DaemonSupport::Supported,
        sound: DaemonSupport::Unsupported,
    };
}

impl Default for DaemonCapabilities {
    /// [`Self::ASSUMED`].
    fn default() -> Self {
        Self::ASSUMED
    }
}

/// What is known about the Linux/BSD notification daemon.
///
/// Only meaningful for [`NotifyPlatform::LinuxBsd`]; the macOS and Windows
/// system legs do not depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxDaemon {
    /// A daemon is (assumed) reachable, with these capabilities.
    Available(DaemonCapabilities),
    /// No daemon is reachable, so the Linux/BSD system leg cannot show
    /// anything.
    Unavailable,
}

impl LinuxDaemon {
    /// What is known before the daemon has answered.
    ///
    /// On Linux/BSD that is a daemon with [`DaemonCapabilities::ASSUMED`]
    /// capabilities. Everywhere else there is no Linux/BSD daemon to assume, so
    /// it is [`Self::Unavailable`] (no platform's table other than Linux/BSD
    /// reads it).
    #[must_use]
    pub const fn assumed() -> Self {
        if cfg!(all(unix, not(target_os = "macos"))) {
            Self::Available(DaemonCapabilities::ASSUMED)
        } else {
            Self::Unavailable
        }
    }
}

/// Which platform's system-leg capability row applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyPlatform {
    /// Linux and the BSDs, via the freedesktop notification daemon.
    LinuxBsd,
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
}

impl NotifyPlatform {
    /// The platform this build runs on. Unix other than macOS is
    /// [`Self::LinuxBsd`], matching the D-Bus service's own gate.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(unix) {
            Self::LinuxBsd
        } else {
            Self::Windows
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_are_the_assumed_constant() {
        assert_eq!(DaemonCapabilities::default(), DaemonCapabilities::ASSUMED);
        assert_eq!(DaemonCapabilities::ASSUMED.body, DaemonSupport::Supported);
        assert_eq!(
            DaemonCapabilities::ASSUMED.actions,
            DaemonSupport::Supported
        );
        assert_eq!(
            DaemonCapabilities::ASSUMED.sound,
            DaemonSupport::Unsupported
        );
    }

    #[test]
    fn assumed_daemon_is_available_on_linux_bsd_only() {
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(
                LinuxDaemon::assumed(),
                LinuxDaemon::Available(DaemonCapabilities::ASSUMED)
            );
        } else {
            assert_eq!(LinuxDaemon::assumed(), LinuxDaemon::Unavailable);
        }
    }

    #[test]
    fn current_platform_matches_the_build_target() {
        let expected = if cfg!(target_os = "macos") {
            NotifyPlatform::MacOs
        } else if cfg!(unix) {
            NotifyPlatform::LinuxBsd
        } else {
            NotifyPlatform::Windows
        };
        assert_eq!(NotifyPlatform::current(), expected);
    }
}
