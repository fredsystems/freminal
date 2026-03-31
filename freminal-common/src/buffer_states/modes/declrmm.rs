// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Left/Right Margin Mode (DECLRMM) ?69
///
/// When set, DECSLRM (`CSI Pl ; Pr s`) can be used to set left and right
/// margins.  When reset, `CSI s` reverts to SCOSC (save cursor).
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Declrmm {
    #[default]
    Disabled,
    Enabled,
    Query,
}

impl Declrmm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl ReportMode for Declrmm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Disabled => String::from("\x1b[?69;2$y"),
                Self::Enabled => String::from("\x1b[?69;1$y"),
                Self::Query => String::from("\x1b[?69;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?69;1$y"),
                SetMode::DecRst => String::from("\x1b[?69;2$y"),
                SetMode::DecQuery => String::from("\x1b[?69;0$y"),
            },
        )
    }
}

impl fmt::Display for Declrmm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "Left/Right Margin Mode (DECLRMM) Disabled"),
            Self::Enabled => write!(f, "Left/Right Margin Mode (DECLRMM) Enabled"),
            Self::Query => write!(f, "Left/Right Margin Mode (DECLRMM) Query"),
        }
    }
}
