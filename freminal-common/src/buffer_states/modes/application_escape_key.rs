// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Application Escape Key Mode `?7727`
///
/// This is a tmux-invented private mode.  When set, pressing the Escape
/// key sends `CSI 27 ; 1 ; 27 ~` (an unambiguous CSI u-style sequence)
/// instead of bare `ESC` (`0x1b`).  This allows tmux (and other
/// multiplexers) to instantly distinguish a user-typed Escape from the
/// start of an escape sequence, eliminating the traditional ~300 ms
/// ambiguity timeout.
///
/// DECRQM query responds with mode 1 (set) or mode 2 (reset) depending
/// on the current state.
#[derive(Debug, Default, Eq, PartialEq, Clone, Copy)]
pub enum ApplicationEscapeKey {
    #[default]
    /// Reset (off) — default.
    Reset,
    /// Set (on) — tmux requested escape-key wrapping.
    Set,
    /// Query.
    Query,
}

impl ApplicationEscapeKey {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Set,
            SetMode::DecRst => Self::Reset,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for ApplicationEscapeKey {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Reset => String::from("\x1b[?7727;2$y"),
                Self::Set => String::from("\x1b[?7727;1$y"),
                Self::Query => String::from("\x1b[?7727;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?7727;1$y"),
                SetMode::DecRst => String::from("\x1b[?7727;2$y"),
                SetMode::DecQuery => String::from("\x1b[?7727;0$y"),
            },
        )
    }
}

impl fmt::Display for ApplicationEscapeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reset => write!(f, "Application Escape Key (?7727) Reset"),
            Self::Set => write!(f, "Application Escape Key (?7727) Set"),
            Self::Query => write!(f, "Application Escape Key (?7727) Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Constructor ───────────────────────────────────────────────────

    #[test]
    fn new_dec_set_returns_set() {
        assert_eq!(
            ApplicationEscapeKey::new(&SetMode::DecSet),
            ApplicationEscapeKey::Set
        );
    }

    #[test]
    fn new_dec_rst_returns_reset() {
        assert_eq!(
            ApplicationEscapeKey::new(&SetMode::DecRst),
            ApplicationEscapeKey::Reset
        );
    }

    #[test]
    fn new_dec_query_returns_query() {
        assert_eq!(
            ApplicationEscapeKey::new(&SetMode::DecQuery),
            ApplicationEscapeKey::Query
        );
    }

    // ── Default ──────────────────────────────────────────────────────

    #[test]
    fn default_is_reset() {
        assert_eq!(ApplicationEscapeKey::default(), ApplicationEscapeKey::Reset);
    }

    // ── ReportMode (self-report, no override) ────────────────────────

    #[test]
    fn report_reset_no_override() {
        assert_eq!(ApplicationEscapeKey::Reset.report(None), "\x1b[?7727;2$y");
    }

    #[test]
    fn report_set_no_override() {
        assert_eq!(ApplicationEscapeKey::Set.report(None), "\x1b[?7727;1$y");
    }

    #[test]
    fn report_query_no_override() {
        assert_eq!(ApplicationEscapeKey::Query.report(None), "\x1b[?7727;0$y");
    }

    // ── ReportMode (with override) ──────────────────────────────────

    #[test]
    fn report_override_dec_set() {
        assert_eq!(
            ApplicationEscapeKey::Reset.report(Some(SetMode::DecSet)),
            "\x1b[?7727;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        assert_eq!(
            ApplicationEscapeKey::Reset.report(Some(SetMode::DecRst)),
            "\x1b[?7727;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        assert_eq!(
            ApplicationEscapeKey::Set.report(Some(SetMode::DecQuery)),
            "\x1b[?7727;0$y"
        );
    }

    // ── Regression: DECRQM report values ────────────────────────────
    // The bug that was fixed: report() used to always return mode 4
    // ("permanently reset") instead of mode 1/2 based on current state.

    #[test]
    fn regression_set_report_is_mode_1_not_mode_4() {
        let report = ApplicationEscapeKey::Set.report(None);
        assert!(
            report.contains(";1$y"),
            "Set.report(None) must contain ';1$y' (mode 1 = set), got: {report}"
        );
        assert!(
            !report.contains(";4$y"),
            "Set.report(None) must NOT contain ';4$y' (mode 4 = permanently reset)"
        );
    }

    #[test]
    fn regression_reset_report_is_mode_2_not_mode_4() {
        let report = ApplicationEscapeKey::Reset.report(None);
        assert!(
            report.contains(";2$y"),
            "Reset.report(None) must contain ';2$y' (mode 2 = reset), got: {report}"
        );
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn display_all_variants() {
        let variants = [
            ApplicationEscapeKey::Reset,
            ApplicationEscapeKey::Set,
            ApplicationEscapeKey::Query,
        ];
        for v in &variants {
            let s = format!("{v}");
            assert!(!s.is_empty(), "Display for {v:?} should not be empty");
            assert!(s.contains("7727"), "Display for {v:?} should mention 7727");
        }
    }
}
