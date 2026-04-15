// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Left/Right Margin Mode (DECLRMM) ?69
///
/// When set, DECSLRM (`CSI Pl ; Pr s`) can be used to set left and right
/// margins.  When reset, `CSI s` reverts to SCOSC (save cursor).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum Declrmm {
    #[default]
    Disabled,
    Enabled,
    Query,
}

impl Declrmm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl ReportMode for Declrmm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Disabled => String::from("\x1b[?69;2$y"),
                Self::Enabled => String::from("\x1b[?69;1$y"),
                Self::Query => String::from("\x1b[?69;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?69;1$y"),
                SetMode::DecRst => String::from("\x1b[?69;2$y"),
                SetMode::DecQuery => String::from("\x1b[?69;0$y"),
            },
        )
    }
}

impl fmt::Display for Declrmm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "Left/Right Margin Mode (DECLRMM) Disabled"),
            Self::Enabled => write!(f, "Left/Right Margin Mode (DECLRMM) Enabled"),
            Self::Query => write!(f, "Left/Right Margin Mode (DECLRMM) Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_enabled() ─────────────────────────────────────────────────

    #[test]
    fn is_enabled_true_for_enabled() {
        assert!(Declrmm::Enabled.is_enabled());
    }

    #[test]
    fn is_enabled_false_for_disabled() {
        assert!(!Declrmm::Disabled.is_enabled());
    }

    #[test]
    fn is_enabled_false_for_query() {
        assert!(!Declrmm::Query.is_enabled());
    }

    // ── ReportMode ───────────────────────────────────────────────────

    #[test]
    fn report_enabled_no_override() {
        assert_eq!(Declrmm::Enabled.report(None), "\x1b[?69;1$y");
    }

    #[test]
    fn report_disabled_no_override() {
        assert_eq!(Declrmm::Disabled.report(None), "\x1b[?69;2$y");
    }

    #[test]
    fn report_query_no_override() {
        assert_eq!(Declrmm::Query.report(None), "\x1b[?69;0$y");
    }

    #[test]
    fn report_with_dec_set_override() {
        assert_eq!(
            Declrmm::Disabled.report(Some(SetMode::DecSet)),
            "\x1b[?69;1$y"
        );
    }

    #[test]
    fn report_with_dec_rst_override() {
        assert_eq!(
            Declrmm::Enabled.report(Some(SetMode::DecRst)),
            "\x1b[?69;2$y"
        );
    }

    #[test]
    fn report_with_dec_query_override() {
        assert_eq!(
            Declrmm::Enabled.report(Some(SetMode::DecQuery)),
            "\x1b[?69;0$y"
        );
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn display_enabled() {
        assert_eq!(
            Declrmm::Enabled.to_string(),
            "Left/Right Margin Mode (DECLRMM) Enabled"
        );
    }

    #[test]
    fn display_disabled() {
        assert_eq!(
            Declrmm::Disabled.to_string(),
            "Left/Right Margin Mode (DECLRMM) Disabled"
        );
    }

    #[test]
    fn display_query() {
        assert_eq!(
            Declrmm::Query.to_string(),
            "Left/Right Margin Mode (DECLRMM) Query"
        );
    }

    // ── default ──────────────────────────────────────────────────────

    #[test]
    fn default_is_disabled() {
        assert_eq!(Declrmm::default(), Declrmm::Disabled);
    }
}
