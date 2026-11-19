// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Hilite mouse tracking (`?1001`).
///
/// An X11-era protocol in which the terminal highlights the region between
/// press and release. It is deliberately **not** a [`MouseTrack`] level:
/// setting it neither reports mouse events nor changes the active tracking
/// mode.
///
/// Freminal recognises `?1001` and does not implement it: the request is
/// logged and nothing changes. kitty, Ghostty and `WezTerm` don't implement it,
/// and `WezTerm`'s documentation warns that it can hang the terminal. Because
/// the mode is not implemented, DECRQM answers `Ps = 0` (not recognised)
/// whatever the variant.
///
/// [`MouseTrack`]: crate::buffer_states::modes::mouse::MouseTrack
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum HiliteMouseTracking {
    /// `CSI ? 1001 h`.
    Enabled,
    /// `CSI ? 1001 l`.
    #[default]
    Disabled,
    /// DECRQM query (`CSI ? 1001 $ p`).
    Query,
}

impl HiliteMouseTracking {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for HiliteMouseTracking {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        String::from("?1001;0$y")
    }
}

impl fmt::Display for HiliteMouseTracking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => write!(f, "Hilite Mouse Tracking (?1001) — Enabled"),
            Self::Disabled => write!(f, "Hilite Mouse Tracking (?1001) — Disabled"),
            Self::Query => write!(f, "Hilite Mouse Tracking (?1001) — Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(
            HiliteMouseTracking::new(&SetMode::DecSet),
            HiliteMouseTracking::Enabled
        );
        assert_eq!(
            HiliteMouseTracking::new(&SetMode::DecRst),
            HiliteMouseTracking::Disabled
        );
        assert_eq!(
            HiliteMouseTracking::new(&SetMode::DecQuery),
            HiliteMouseTracking::Query
        );
    }

    #[test]
    fn default_is_disabled() {
        assert_eq!(
            HiliteMouseTracking::default(),
            HiliteMouseTracking::Disabled
        );
    }

    #[test]
    fn report_is_not_recognised_for_every_variant_and_override() {
        for mode in [
            HiliteMouseTracking::Enabled,
            HiliteMouseTracking::Disabled,
            HiliteMouseTracking::Query,
        ] {
            assert_eq!(mode.report(None), "?1001;0$y");
            assert_eq!(mode.report(Some(SetMode::DecSet)), "?1001;0$y");
            assert_eq!(mode.report(Some(SetMode::DecRst)), "?1001;0$y");
            assert_eq!(mode.report(Some(SetMode::DecQuery)), "?1001;0$y");
        }
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            HiliteMouseTracking::Enabled.to_string(),
            "Hilite Mouse Tracking (?1001) — Enabled"
        );
        assert_eq!(
            HiliteMouseTracking::Disabled.to_string(),
            "Hilite Mouse Tracking (?1001) — Disabled"
        );
        assert_eq!(
            HiliteMouseTracking::Query.to_string(),
            "Hilite Mouse Tracking (?1001) — Query"
        );
    }
}
