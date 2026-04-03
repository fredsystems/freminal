// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Reverse Wrap Around (xterm private mode ?45)
///
/// Controls whether the cursor wraps backwards from column 0 to the end of the previous line.
/// When set, moving left past column 0 wraps the cursor to the last column of the previous line.
/// When reset (default), the cursor stops at column 0 and does not reverse-wrap.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum ReverseWrapAround {
    #[default]
    /// Set Mode
    /// Reverse wrap is enabled; cursor wraps backwards from column 0 to the previous line.
    WrapAround,
    /// Reset Mode
    /// Reverse wrap is disabled; cursor stops at column 0 and does not wrap backwards.
    DontWrap,
    Query,
}

impl ReportMode for ReverseWrapAround {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::DontWrap => String::from("\x1b[?45;2$y"),
                Self::WrapAround => String::from("\x1b[?45;1$y"),
                Self::Query => String::from("\x1b[?45;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?45;1$y"),
                SetMode::DecRst => String::from("\x1b[?45;2$y"),
                SetMode::DecQuery => String::from("\x1b[?45;0$y"),
            },
        )
    }
}

impl ReverseWrapAround {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::WrapAround,
            SetMode::DecRst => Self::DontWrap,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for ReverseWrapAround {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrapAround => write!(f, "Wrap Around"),
            Self::DontWrap => write!(f, "No Wrap Around"),
            Self::Query => write!(f, "Query Wrap Around"),
        }
    }
}
