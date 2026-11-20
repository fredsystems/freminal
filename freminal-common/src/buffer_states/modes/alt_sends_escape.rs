// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Alt sends escape (`?1039`).
///
/// xterm calls this `altSendsEscape`. Freminal stores the mode and reports it
/// truthfully through DECRQM, and nothing else reads it: the Alt prefix is
/// gated by [`MetaSendsEscape`](super::meta_sends_escape::MetaSendsEscape)
/// (`?1036`) alone, as in Ghostty, which also stores `?1039` without consuming
/// it. The default is [`Self::Reset`].
///
/// DECRQM answers `Ps = 1` (set) or `Ps = 2` (reset).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum AltSendsEscape {
    /// `CSI ? 1039 h`.
    Set,
    /// `CSI ? 1039 l`. The default.
    #[default]
    Reset,
    /// DECRQM query (`CSI ? 1039 $ p`).
    Query,
}

impl AltSendsEscape {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Set,
            SetMode::DecRst => Self::Reset,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for AltSendsEscape {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Set => String::from("?1039;1$y"),
                Self::Reset => String::from("?1039;2$y"),
                Self::Query => String::from("?1039;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("?1039;1$y"),
                SetMode::DecRst => String::from("?1039;2$y"),
                SetMode::DecQuery => String::from("?1039;0$y"),
            },
        )
    }
}

impl fmt::Display for AltSendsEscape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set => write!(f, "Alt Sends Escape (?1039) Set"),
            Self::Reset => write!(f, "Alt Sends Escape (?1039) Reset"),
            Self::Query => write!(f, "Alt Sends Escape (?1039) Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(AltSendsEscape::new(&SetMode::DecSet), AltSendsEscape::Set);
        assert_eq!(AltSendsEscape::new(&SetMode::DecRst), AltSendsEscape::Reset);
        assert_eq!(
            AltSendsEscape::new(&SetMode::DecQuery),
            AltSendsEscape::Query
        );
    }

    #[test]
    fn default_is_reset() {
        assert_eq!(AltSendsEscape::default(), AltSendsEscape::Reset);
    }

    #[test]
    fn report_follows_the_stored_state() {
        assert_eq!(AltSendsEscape::Set.report(None), "?1039;1$y");
        assert_eq!(AltSendsEscape::Reset.report(None), "?1039;2$y");
        assert_eq!(AltSendsEscape::Query.report(None), "?1039;0$y");
    }

    #[test]
    fn report_override_wins_over_the_stored_state() {
        assert_eq!(
            AltSendsEscape::Reset.report(Some(SetMode::DecSet)),
            "?1039;1$y"
        );
        assert_eq!(
            AltSendsEscape::Set.report(Some(SetMode::DecRst)),
            "?1039;2$y"
        );
        assert_eq!(
            AltSendsEscape::Set.report(Some(SetMode::DecQuery)),
            "?1039;0$y"
        );
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            AltSendsEscape::Set.to_string(),
            "Alt Sends Escape (?1039) Set"
        );
        assert_eq!(
            AltSendsEscape::Reset.to_string(),
            "Alt Sends Escape (?1039) Reset"
        );
        assert_eq!(
            AltSendsEscape::Query.to_string(),
            "Alt Sends Escape (?1039) Query"
        );
    }
}
