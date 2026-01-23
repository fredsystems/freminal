// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Show cursor (DECTCEM) ?40
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum AllowColumnModeSwitch {
    #[default]
    /// Normal (Set) Mode
    /// Allow switching from 80 to 132 columns.
    AllowColumnModeSwitch,
    /// Alternate (Reset) Mode
    /// Do not allow switching from 80 to 132 columns.
    NoAllowColumnModeSwitch,
    Query,
}

impl ReportMode for AllowColumnModeSwitch {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NoAllowColumnModeSwitch => String::from("\x1b[?40;2$y"),
                Self::AllowColumnModeSwitch => String::from("\x1b[?40;1$y"),
                Self::Query => String::from("\x1b[?40;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?40;1$y"),
                SetMode::DecRst => String::from("\x1b[?40;2$y"),
                SetMode::DecQuery => String::from("\x1b[?40;0$y"),
            },
        )
    }
}

impl AllowColumnModeSwitch {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::AllowColumnModeSwitch,
            SetMode::DecRst => Self::NoAllowColumnModeSwitch,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for AllowColumnModeSwitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAllowColumnModeSwitch => write!(f, "NoAllowColumnModeSwitch"),
            Self::AllowColumnModeSwitch => write!(f, "AllowColumnModeSwitch"),
            Self::Query => write!(f, "Query"),
        }
    }
}
