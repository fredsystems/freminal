// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Focus reporting mode (`XT_MSE_WIN`)
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum XtMseWin {
    #[default]
    /// Focus reporting is disabled
    Disabled,
    /// Focus reporting is enabled
    Enabled,
    Query,
}

impl XtMseWin {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for XtMseWin {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Disabled => String::from("\x1b[?1004;2$y"),
                Self::Enabled => String::from("\x1b[?1004;1$y"),
                Self::Query => String::from("\x1b[?1004;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1004;1$y"),
                SetMode::DecRst => String::from("\x1b[?1004;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1004;0$y"),
            },
        )
    }
}

impl fmt::Display for XtMseWin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("Focus Reporting Mode (XT_MSE_WIN) Disabled"),
            Self::Enabled => f.write_str("Focus Reporting Mode (XT_MSE_WIN) Enabled"),
            Self::Query => f.write_str("Focus Reporting Mode (XT_MSE_WIN) Query"),
        }
    }
}
