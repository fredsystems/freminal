// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Bracketed Paste (`RL_BRACKET`) Mode ?2004
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub enum RlBracket {
    #[default]
    /// Normal (Reset) Mode
    /// Bracketed paste mode is disabled
    Disabled,
    /// Alternate (Set) Mode
    /// Bracketed paste mode is enabled and the terminal will send ESC [200~ and ESC [201~ around pasted text
    Enabled,
    Query,
}

impl RlBracket {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for RlBracket {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Disabled => String::from("\x1b[?2004;2$y"),
                Self::Enabled => String::from("\x1b[?2004;1$y"),
                Self::Query => String::from("\x1b[?2004;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?2004;1$y"),
                SetMode::DecRst => String::from("\x1b[?2004;2$y"),
                SetMode::DecQuery => String::from("\x1b[?2004;0$y"),
            },
        )
    }
}

impl fmt::Display for RlBracket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "Bracketed Paste Mode (DEC 2004) Disabled"),
            Self::Enabled => write!(f, "Bracketed Paste Mode (DEC 2004) Enabled"),
            Self::Query => write!(f, "Bracketed Paste Mode (DEC 2004) Query"),
        }
    }
}
