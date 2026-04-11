// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// DECRQM mode `?2048` — `in-band-resize` query/set/reset via DEC private mode.
///
/// This is the DEC private mode equivalent of the `CSI > 4 ; Pv m` sequence.
/// tmux queries `?2048` via DECRQM to check whether the terminal supports
/// `in-band-resize`.  Responding with mode 1 (set) or mode 2 (reset) tells
/// tmux the feature is recognised.
///
/// - `DECSET ?2048` → enable `in-band-resize` level 1
/// - `DECRST ?2048` → disable `in-band-resize` (level 0)
/// - `DECRQM ?2048` → report current state
#[derive(Debug, Default, Eq, PartialEq, Clone, Copy)]
pub enum InBandResizeMode {
    #[default]
    /// Reset (off) — `in-band-resize` level 0.
    Reset,
    /// Set (on) — `in-band-resize` level 1.
    Set,
    /// Query — report current state.
    Query,
}

impl InBandResizeMode {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Set,
            SetMode::DecRst => Self::Reset,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for InBandResizeMode {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Reset => String::from("\x1b[?2048;2$y"),
                Self::Set => String::from("\x1b[?2048;1$y"),
                Self::Query => String::from("\x1b[?2048;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?2048;1$y"),
                SetMode::DecRst => String::from("\x1b[?2048;2$y"),
                SetMode::DecQuery => String::from("\x1b[?2048;0$y"),
            },
        )
    }
}

impl fmt::Display for InBandResizeMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reset => write!(f, "InBandResize Mode (DEC 2048) Reset"),
            Self::Set => write!(f, "InBandResize Mode (DEC 2048) Set"),
            Self::Query => write!(f, "InBandResize Mode (DEC 2048) Query"),
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
            InBandResizeMode::new(&SetMode::DecSet),
            InBandResizeMode::Set
        );
    }

    #[test]
    fn new_dec_rst_returns_reset() {
        assert_eq!(
            InBandResizeMode::new(&SetMode::DecRst),
            InBandResizeMode::Reset
        );
    }

    #[test]
    fn new_dec_query_returns_query() {
        assert_eq!(
            InBandResizeMode::new(&SetMode::DecQuery),
            InBandResizeMode::Query
        );
    }

    // ── Default ──────────────────────────────────────────────────────

    #[test]
    fn default_is_reset() {
        assert_eq!(InBandResizeMode::default(), InBandResizeMode::Reset);
    }

    // ── ReportMode (self-report, no override) ────────────────────────

    #[test]
    fn report_reset_no_override() {
        assert_eq!(InBandResizeMode::Reset.report(None), "\x1b[?2048;2$y");
    }

    #[test]
    fn report_set_no_override() {
        assert_eq!(InBandResizeMode::Set.report(None), "\x1b[?2048;1$y");
    }

    #[test]
    fn report_query_no_override() {
        assert_eq!(InBandResizeMode::Query.report(None), "\x1b[?2048;0$y");
    }

    // ── ReportMode (with override) ──────────────────────────────────

    #[test]
    fn report_override_dec_set() {
        // Regardless of self, override DecSet → mode 1
        assert_eq!(
            InBandResizeMode::Reset.report(Some(SetMode::DecSet)),
            "\x1b[?2048;1$y"
        );
    }

    #[test]
    fn report_override_dec_rst() {
        // Regardless of self, override DecRst → mode 2
        assert_eq!(
            InBandResizeMode::Reset.report(Some(SetMode::DecRst)),
            "\x1b[?2048;2$y"
        );
    }

    #[test]
    fn report_override_dec_query() {
        // Regardless of self, override DecQuery → mode 0
        assert_eq!(
            InBandResizeMode::Set.report(Some(SetMode::DecQuery)),
            "\x1b[?2048;0$y"
        );
    }

    // ── Display ──────────────────────────────────────────────────────

    #[test]
    fn display_all_variants() {
        // Ensure Display does not panic and produces non-empty strings
        let variants = [
            InBandResizeMode::Reset,
            InBandResizeMode::Set,
            InBandResizeMode::Query,
        ];
        for v in &variants {
            let s = format!("{v}");
            assert!(!s.is_empty(), "Display for {v:?} should not be empty");
            assert!(s.contains("2048"), "Display for {v:?} should mention 2048");
        }
    }
}
