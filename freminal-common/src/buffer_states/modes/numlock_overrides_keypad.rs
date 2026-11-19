// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// `NumLock` overrides the application keypad (`?1035`).
///
/// In xterm, enabling it makes the `NumLock` state take precedence over the
/// application-keypad mode.
///
/// Freminal recognises `?1035` and does not implement it: the request is
/// logged and nothing changes. winit exposes no `NumLock` state (Task 114.11),
/// so the mode cannot be honoured. Because the mode is not implemented,
/// DECRQM answers `Ps = 0` (not recognised) whatever the variant.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum NumLockOverridesKeypad {
    /// `CSI ? 1035 h`.
    Enabled,
    /// `CSI ? 1035 l`.
    #[default]
    Disabled,
    /// DECRQM query (`CSI ? 1035 $ p`).
    Query,
}

impl NumLockOverridesKeypad {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for NumLockOverridesKeypad {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        String::from("?1035;0$y")
    }
}

impl fmt::Display for NumLockOverridesKeypad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => write!(f, "NumLock Overrides Keypad (?1035) — Enabled"),
            Self::Disabled => write!(f, "NumLock Overrides Keypad (?1035) — Disabled"),
            Self::Query => write!(f, "NumLock Overrides Keypad (?1035) — Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(
            NumLockOverridesKeypad::new(&SetMode::DecSet),
            NumLockOverridesKeypad::Enabled
        );
        assert_eq!(
            NumLockOverridesKeypad::new(&SetMode::DecRst),
            NumLockOverridesKeypad::Disabled
        );
        assert_eq!(
            NumLockOverridesKeypad::new(&SetMode::DecQuery),
            NumLockOverridesKeypad::Query
        );
    }

    #[test]
    fn default_is_disabled() {
        assert_eq!(
            NumLockOverridesKeypad::default(),
            NumLockOverridesKeypad::Disabled
        );
    }

    #[test]
    fn report_is_not_recognised_for_every_variant_and_override() {
        for mode in [
            NumLockOverridesKeypad::Enabled,
            NumLockOverridesKeypad::Disabled,
            NumLockOverridesKeypad::Query,
        ] {
            assert_eq!(mode.report(None), "?1035;0$y");
            assert_eq!(mode.report(Some(SetMode::DecSet)), "?1035;0$y");
            assert_eq!(mode.report(Some(SetMode::DecRst)), "?1035;0$y");
            assert_eq!(mode.report(Some(SetMode::DecQuery)), "?1035;0$y");
        }
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            NumLockOverridesKeypad::Enabled.to_string(),
            "NumLock Overrides Keypad (?1035) — Enabled"
        );
        assert_eq!(
            NumLockOverridesKeypad::Disabled.to_string(),
            "NumLock Overrides Keypad (?1035) — Disabled"
        );
        assert_eq!(
            NumLockOverridesKeypad::Query.to_string(),
            "NumLock Overrides Keypad (?1035) — Query"
        );
    }
}
