// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{line_wrap::LineWrap, mode::SetMode};

use super::ReportMode;

/// Autowrap Mode (DECAWM) ?7
#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub enum Decawm {
    /// Normal (Reset) Mode
    /// Disables autowrap mode.
    NoAutoWrap,
    /// Alternate (Set) Mode
    /// Enables autowrap mode
    #[default]
    AutoWrap,
    Query,
}

impl Decawm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::AutoWrap,
            SetMode::DecRst => Self::NoAutoWrap,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl From<LineWrap> for Decawm {
    fn from(value: LineWrap) -> Self {
        match value {
            LineWrap::Wrap => Self::AutoWrap,
            LineWrap::NoWrap => Self::NoAutoWrap,
        }
    }
}

impl ReportMode for Decawm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NoAutoWrap => String::from("\x1b[?7;2$y"),
                Self::AutoWrap => String::from("\x1b[?7;1$y"),
                Self::Query => String::from("\x1b[?7;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?7;1$y"),
                SetMode::DecRst => String::from("\x1b[?7;2$y"),
                SetMode::DecQuery => String::from("\x1b[?7;0$y"),
            },
        )
    }
}

impl fmt::Display for Decawm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAutoWrap => write!(f, "Autowrap Mode (DECAWM) Disabled"),
            Self::AutoWrap => write!(f, "Autowrap Mode (DECAWM) Enabled"),
            Self::Query => write!(f, "Autowrap Mode (DECAWM) Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── From<LineWrap> ───────────────────────────────────────────────

    #[test]
    fn from_line_wrap_wrap_is_auto_wrap() {
        assert_eq!(Decawm::from(LineWrap::Wrap), Decawm::AutoWrap);
    }

    #[test]
    fn from_line_wrap_no_wrap_is_no_auto_wrap() {
        assert_eq!(Decawm::from(LineWrap::NoWrap), Decawm::NoAutoWrap);
    }

    // ── ReportMode ───────────────────────────────────────────────────

    #[test]
    fn report_auto_wrap_no_override() {
        assert_eq!(Decawm::AutoWrap.report(None), "\x1b[?7;1$y");
    }

    #[test]
    fn report_no_auto_wrap_no_override() {
        assert_eq!(Decawm::NoAutoWrap.report(None), "\x1b[?7;2$y");
    }

    #[test]
    fn report_query_no_override() {
        assert_eq!(Decawm::Query.report(None), "\x1b[?7;0$y");
    }

    #[test]
    fn report_with_dec_set_override() {
        assert_eq!(
            Decawm::NoAutoWrap.report(Some(SetMode::DecSet)),
            "\x1b[?7;1$y"
        );
    }

    #[test]
    fn report_with_dec_rst_override() {
        assert_eq!(
            Decawm::AutoWrap.report(Some(SetMode::DecRst)),
            "\x1b[?7;2$y"
        );
    }

    #[test]
    fn report_with_dec_query_override() {
        assert_eq!(
            Decawm::AutoWrap.report(Some(SetMode::DecQuery)),
            "\x1b[?7;0$y"
        );
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn display_auto_wrap() {
        assert_eq!(
            Decawm::AutoWrap.to_string(),
            "Autowrap Mode (DECAWM) Enabled"
        );
    }

    #[test]
    fn display_no_auto_wrap() {
        assert_eq!(
            Decawm::NoAutoWrap.to_string(),
            "Autowrap Mode (DECAWM) Disabled"
        );
    }

    #[test]
    fn display_query() {
        assert_eq!(Decawm::Query.to_string(), "Autowrap Mode (DECAWM) Query");
    }
}
