// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Alternate Screen (`XT_EXTSCRN`) ?1049
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum XtExtscrn {
    /// Primary screen
    /// Clear screen, switch to normal screen buffer, and restore cursor position.
    #[default]
    Primary,
    /// Save cursor position, switch to alternate screen buffer, and clear screen.
    /// Also known as the "alternate screen".
    Alternate,
    Query,
}

impl XtExtscrn {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Alternate,
            SetMode::DecRst => Self::Primary,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for XtExtscrn {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Primary => String::from("\x1b[?1049;2$y"),
                Self::Alternate => String::from("\x1b[?1049;1$y"),
                Self::Query => String::from("\x1b[?1049;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1049;1$y"),
                SetMode::DecRst => String::from("\x1b[?1049;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1049;0$y"),
            },
        )
    }
}

impl fmt::Display for XtExtscrn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary => f.write_str("XT_EXTSCRN (RESET) Primary Screen"),
            Self::Alternate => f.write_str("XT_EXTSCRN (SET) Alternate Screen"),
            Self::Query => f.write_str("XT_EXTSCRN (QUERY)"),
        }
    }
}
