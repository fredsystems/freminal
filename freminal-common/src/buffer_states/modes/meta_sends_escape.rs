// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Meta sends escape (`?1036`).
///
/// xterm calls this `metaSendsEscape`: when the Meta modifier is held, the key
/// is sent as `ESC` followed by its character. Freminal has no Meta key and
/// treats Alt as Meta, so this mode gates the legacy Alt prefix: while
/// [`Self::Set`] (the default, as in Ghostty and kitty), Alt+text sends `ESC`
/// then the text; while [`Self::Reset`], the text is sent alone.
///
/// The mode only affects the legacy keyboard encoding. While any kitty
/// keyboard flag is active the kitty encoding owns Alt+text.
///
/// DECRQM answers `Ps = 1` (set) or `Ps = 2` (reset).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum MetaSendsEscape {
    /// `CSI ? 1036 h` -- Alt+text is prefixed with `ESC`. The default.
    #[default]
    Set,
    /// `CSI ? 1036 l` -- Alt+text is sent without a prefix.
    Reset,
    /// DECRQM query (`CSI ? 1036 $ p`).
    Query,
}

impl MetaSendsEscape {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Set,
            SetMode::DecRst => Self::Reset,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for MetaSendsEscape {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Set => String::from("?1036;1$y"),
                Self::Reset => String::from("?1036;2$y"),
                Self::Query => String::from("?1036;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("?1036;1$y"),
                SetMode::DecRst => String::from("?1036;2$y"),
                SetMode::DecQuery => String::from("?1036;0$y"),
            },
        )
    }
}

impl fmt::Display for MetaSendsEscape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set => write!(f, "Meta Sends Escape (?1036) Set"),
            Self::Reset => write!(f, "Meta Sends Escape (?1036) Reset"),
            Self::Query => write!(f, "Meta Sends Escape (?1036) Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(MetaSendsEscape::new(&SetMode::DecSet), MetaSendsEscape::Set);
        assert_eq!(
            MetaSendsEscape::new(&SetMode::DecRst),
            MetaSendsEscape::Reset
        );
        assert_eq!(
            MetaSendsEscape::new(&SetMode::DecQuery),
            MetaSendsEscape::Query
        );
    }

    #[test]
    fn default_is_set() {
        assert_eq!(MetaSendsEscape::default(), MetaSendsEscape::Set);
    }

    #[test]
    fn report_follows_the_stored_state() {
        assert_eq!(MetaSendsEscape::Set.report(None), "?1036;1$y");
        assert_eq!(MetaSendsEscape::Reset.report(None), "?1036;2$y");
        assert_eq!(MetaSendsEscape::Query.report(None), "?1036;0$y");
    }

    #[test]
    fn report_override_wins_over_the_stored_state() {
        assert_eq!(
            MetaSendsEscape::Reset.report(Some(SetMode::DecSet)),
            "?1036;1$y"
        );
        assert_eq!(
            MetaSendsEscape::Set.report(Some(SetMode::DecRst)),
            "?1036;2$y"
        );
        assert_eq!(
            MetaSendsEscape::Set.report(Some(SetMode::DecQuery)),
            "?1036;0$y"
        );
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            MetaSendsEscape::Set.to_string(),
            "Meta Sends Escape (?1036) Set"
        );
        assert_eq!(
            MetaSendsEscape::Reset.to_string(),
            "Meta Sends Escape (?1036) Reset"
        );
        assert_eq!(
            MetaSendsEscape::Query.to_string(),
            "Meta Sends Escape (?1036) Query"
        );
    }
}
