// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Allow Alternate Screen Switching (`?1046`)
///
/// Controls whether the alternate screen buffer can be entered or left:
/// - **Set** (`CSI ? 1046 h`): Allow — switching to/from the alternate screen
///   is permitted (default).
/// - **Reset** (`CSI ? 1046 l`): Disallow — `?47`, `?1047`, and `?1049`
///   Set/Reset are silently ignored while this mode is reset.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum AllowAltScreen {
    /// Set: Allow alternate screen switching (default).
    #[default]
    Allow,
    /// Reset: Disallow alternate screen switching.
    Disallow,
    Query,
}

impl ReportMode for AllowAltScreen {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Allow => String::from("\x1b[?1046;1$y"),
                Self::Disallow => String::from("\x1b[?1046;2$y"),
                Self::Query => String::from("\x1b[?1046;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1046;1$y"),
                SetMode::DecRst => String::from("\x1b[?1046;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1046;0$y"),
            },
        )
    }
}

impl AllowAltScreen {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Allow,
            SetMode::DecRst => Self::Disallow,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for AllowAltScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allow => write!(f, "Allow Alternate Screen Switching (?1046)"),
            Self::Disallow => write!(f, "Disallow Alternate Screen Switching (?1046)"),
            Self::Query => write!(f, "Query Allow Alternate Screen Switching (?1046)"),
        }
    }
}
