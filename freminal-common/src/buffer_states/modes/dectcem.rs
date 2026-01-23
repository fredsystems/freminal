// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Show cursor (DECTCEM) ?25
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Dectcem {
    #[default]
    /// Normal (Set) Mode
    /// Show cursor.
    Show,
    /// Alternate (Reset) Mode
    /// Hide cursor.
    Hide,
    Query,
}

impl ReportMode for Dectcem {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Hide => String::from("\x1b[?25;2$y"),
                Self::Show => String::from("\x1b[?25;1$y"),
                Self::Query => String::from("\x1b[?25;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?25;1$y"),
                SetMode::DecRst => String::from("\x1b[?25;2$y"),
                SetMode::DecQuery => String::from("\x1b[?25;0$y"),
            },
        )
    }
}

impl Dectcem {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Show,
            SetMode::DecRst => Self::Hide,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Dectcem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Show => write!(f, "Show Cursor (DECTCEM)"),
            Self::Hide => write!(f, "Hide Cursor (DECTCEM)"),
            Self::Query => write!(f, "Query Cursor (DECTCEM)"),
        }
    }
}
