// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Show cursor (DECTCEM) ?8
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decarm {
    #[default]
    /// Normal (Set) Mode
    /// Repeat key presses.
    RepeatKey,
    /// Alternate (Reset) Mode
    /// Do not repeat keys.
    NoRepeatKey,
    Query,
}

impl ReportMode for Decarm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NoRepeatKey => String::from("\x1b[?8;2$y"),
                Self::RepeatKey => String::from("\x1b[?8;1$y"),
                Self::Query => String::from("\x1b[?8;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?8;1$y"),
                SetMode::DecRst => String::from("\x1b[?8;2$y"),
                SetMode::DecQuery => String::from("\x1b[?8;0$y"),
            },
        )
    }
}

impl Decarm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::RepeatKey,
            SetMode::DecRst => Self::NoRepeatKey,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Decarm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RepeatKey => write!(f, "Repeat Key (DECARM)"),
            Self::NoRepeatKey => write!(f, "No Repeat Key (DECARM)"),
            Self::Query => write!(f, "Query Repeat Key (DECARM)"),
        }
    }
}
