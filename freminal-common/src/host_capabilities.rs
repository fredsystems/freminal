// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The registry of host-dependent capability facts.
//!
//! The PTY thread owns the terminal handler and answers protocol queries
//! itself, but some of the answers depend on facts it cannot know by itself:
//! what the GUI is configured to do (`config.toml`) and what the platform
//! supports. Those facts live here, as plain `Copy` values the GUI computes
//! and sends to the PTY thread (seeded at pane spawn, re-sent on config
//! change).
//!
//! Only host-dependent facts belong in this module. Answers that are intrinsic
//! to the handler's own implementation stay with that code.
//!
//! Every capability is a named domain enum rather than a bare `bool`, because
//! these values cross a thread boundary.

/// All host-dependent capability facts, as seen by the PTY thread.
///
/// The [`Default`] is the most conservative answer: nothing is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HostCapabilities {
    /// What the host can do for kitty desktop notifications (OSC 99).
    pub osc99: Osc99Support,
}

/// Whether the host honours OSC 99 (kitty desktop notifications) at all, and
/// if so with which optional features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Osc99Support {
    /// OSC 99 is not honoured: it is disabled by configuration, so no
    /// notification will be produced and the terminal must not advertise it.
    #[default]
    Unsupported,
    /// OSC 99 is honoured, with the given optional features.
    Supported(Osc99Features),
}

/// The optional OSC 99 features a host that honours the protocol can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99Features {
    /// Whether activation (the user clicking a notification) can be reported
    /// back to the application.
    pub activation_report: Osc99ActivationReport,
    /// Whether notification close events can be reported back to the
    /// application.
    pub close_events: Osc99CloseEvents,
}

/// Whether the host can report notification activation (a click) back to the
/// application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99ActivationReport {
    /// Activation is reported.
    Reported,
    /// Activation is never reported.
    NotReported,
}

/// Whether the host can report notification close events back to the
/// application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99CloseEvents {
    /// Close events are reported.
    Reported,
    /// Close events are never reported.
    NotReported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unsupported() {
        assert_eq!(HostCapabilities::default().osc99, Osc99Support::Unsupported);
    }

    #[test]
    fn supported_carries_its_features() {
        let features = Osc99Features {
            activation_report: Osc99ActivationReport::Reported,
            close_events: Osc99CloseEvents::NotReported,
        };
        let caps = HostCapabilities {
            osc99: Osc99Support::Supported(features),
        };
        assert_ne!(caps, HostCapabilities::default());
        assert_eq!(caps.osc99, Osc99Support::Supported(features));
    }
}
