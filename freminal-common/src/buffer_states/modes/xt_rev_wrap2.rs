// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Extended Reverse Wraparound Mode — `?1045`
///
/// When set, the cursor can wrap backwards past row 0 of the visible
/// screen into the scrollback buffer.  Requires `?45` (reverse
/// wraparound) to also be set for any reverse wrapping to occur.
///
/// Default: reset (disabled).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum XtRevWrap2 {
    /// Mode is set — allow reverse-wrap into scrollback.
    Enabled,
    #[default]
    /// Mode is reset — do not allow reverse-wrap into scrollback.
    Disabled,
    /// DECRQM query.
    Query,
}

impl ReportMode for XtRevWrap2 {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Disabled => String::from("\x1b[?1045;2$y"),
                Self::Enabled => String::from("\x1b[?1045;1$y"),
                Self::Query => String::from("\x1b[?1045;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1045;1$y"),
                SetMode::DecRst => String::from("\x1b[?1045;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1045;0$y"),
            },
        )
    }
}

impl XtRevWrap2 {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for XtRevWrap2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => write!(f, "Extended Reverse Wrap Enabled"),
            Self::Disabled => write!(f, "Extended Reverse Wrap Disabled"),
            Self::Query => write!(f, "Query Extended Reverse Wrap"),
        }
    }
}
