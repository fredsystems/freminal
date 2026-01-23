// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Synchronized Updates Mode ?2031
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub enum Theming {
    #[default]
    /// Normal (Reset) Mode
    Light,
    /// Alternate (Set) Mode
    Dark,
    Query,
}

impl Theming {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Light,
            SetMode::DecRst => Self::Dark,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Theming {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Dark => String::from("\x1b[?2031;2$y"),
                Self::Light => String::from("\x1b[?2031;1$y"),
                Self::Query => String::from("\x1b[?2031;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?2031;1$y"),
                SetMode::DecRst => String::from("\x1b[?2031;2$y"),
                SetMode::DecQuery => String::from("\x1b[?2031;0$y"),
            },
        )
    }
}

impl fmt::Display for Theming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Light => write!(f, "Theming Mode (DEC 2031) Light"),
            Self::Dark => write!(f, "Theming Mode (DEC 2031) Dark"),
            Self::Query => write!(f, "Theming Mode (DEC 2031) Query"),
        }
    }
}
