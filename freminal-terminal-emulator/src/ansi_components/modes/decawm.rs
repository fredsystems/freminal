// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use freminal_common::buffer_states::line_wrap::LineWrap;

use crate::ansi_components::mode::SetMode;

use super::ReportMode;

/// Autowrap Mode (DECAWM) ?7
#[derive(Eq, PartialEq, Debug, Default, Clone)]
pub enum Decawm {
    /// Normal (Reset) Mode
    /// Disables autowrap mode.
    NoAutoWrap,
    /// Alternate (Set) Mode
    /// Enables autowrap mode
    #[default]
    AutoWrap,
    Query,
}

impl Decawm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::AutoWrap,
            SetMode::DecRst => Self::NoAutoWrap,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl From<LineWrap> for Decawm {
    fn from(value: LineWrap) -> Self {
        match value {
            LineWrap::Wrap => Self::AutoWrap,
            LineWrap::NoWrap => Self::NoAutoWrap,
        }
    }
}

impl ReportMode for Decawm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NoAutoWrap => String::from("\x1b[?7;2$y"),
                Self::AutoWrap => String::from("\x1b[?7;1$y"),
                Self::Query => String::from("\x1b[?7;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?7;1$y"),
                SetMode::DecRst => String::from("\x1b[?7;2$y"),
                SetMode::DecQuery => String::from("\x1b[?7;0$y"),
            },
        )
    }
}

impl fmt::Display for Decawm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAutoWrap => write!(f, "Autowrap Mode (DECAWM) Disabled"),
            Self::AutoWrap => write!(f, "Autowrap Mode (DECAWM) Enabled"),
            Self::Query => write!(f, "Autowrap Mode (DECAWM) Query"),
        }
    }
}
