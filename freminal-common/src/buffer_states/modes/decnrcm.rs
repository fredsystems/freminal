// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// DECNRCM — National Replacement Character Set Mode (`?42`)
///
/// Controls whether national replacement character sets (NRCs) are used for
/// G0–G3 character set designations:
/// - **Set** (`CSI ? 42 h`): NRC mode enabled — character set designations
///   (`ESC ( A`, etc.) map specific ASCII positions to national characters.
/// - **Reset** (`CSI ? 42 l`): NRC mode disabled — standard character sets
///   used (default).
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decnrcm {
    /// Reset: NRC mode disabled, standard character sets used (default).
    #[default]
    NrcDisabled,
    /// Set: NRC mode enabled, national character sets active.
    NrcEnabled,
    Query,
}

impl ReportMode for Decnrcm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NrcEnabled => String::from("\x1b[?42;1$y"),
                Self::NrcDisabled => String::from("\x1b[?42;2$y"),
                Self::Query => String::from("\x1b[?42;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?42;1$y"),
                SetMode::DecRst => String::from("\x1b[?42;2$y"),
                SetMode::DecQuery => String::from("\x1b[?42;0$y"),
            },
        )
    }
}

impl Decnrcm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::NrcEnabled,
            SetMode::DecRst => Self::NrcDisabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Decnrcm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NrcEnabled => write!(f, "DECNRCM NRC Enabled (?42)"),
            Self::NrcDisabled => write!(f, "DECNRCM NRC Disabled (?42)"),
            Self::Query => write!(f, "DECNRCM Query (?42)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decnrcm_new_set() {
        assert_eq!(Decnrcm::new(&SetMode::DecSet), Decnrcm::NrcEnabled);
    }

    #[test]
    fn decnrcm_new_rst() {
        assert_eq!(Decnrcm::new(&SetMode::DecRst), Decnrcm::NrcDisabled);
    }

    #[test]
    fn decnrcm_new_query() {
        assert_eq!(Decnrcm::new(&SetMode::DecQuery), Decnrcm::Query);
    }

    #[test]
    fn decnrcm_default_is_disabled() {
        assert_eq!(Decnrcm::default(), Decnrcm::NrcDisabled);
    }

    #[test]
    fn decnrcm_report_enabled() {
        assert_eq!(Decnrcm::NrcEnabled.report(None), "\x1b[?42;1$y");
    }

    #[test]
    fn decnrcm_report_disabled() {
        assert_eq!(Decnrcm::NrcDisabled.report(None), "\x1b[?42;2$y");
    }

    #[test]
    fn decnrcm_report_query() {
        assert_eq!(Decnrcm::Query.report(None), "\x1b[?42;0$y");
    }

    #[test]
    fn decnrcm_report_override_set() {
        assert_eq!(
            Decnrcm::NrcDisabled.report(Some(SetMode::DecSet)),
            "\x1b[?42;1$y"
        );
    }

    #[test]
    fn decnrcm_report_override_rst() {
        assert_eq!(
            Decnrcm::NrcEnabled.report(Some(SetMode::DecRst)),
            "\x1b[?42;2$y"
        );
    }

    #[test]
    fn decnrcm_display() {
        assert!(!format!("{}", Decnrcm::NrcEnabled).is_empty());
        assert!(!format!("{}", Decnrcm::NrcDisabled).is_empty());
        assert!(!format!("{}", Decnrcm::Query).is_empty());
    }
}
