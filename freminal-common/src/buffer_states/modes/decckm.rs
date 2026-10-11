// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::buffer_states::{mode::SetMode, modes::ReportMode};
use core::fmt;

/// Cursor Key Mode (DECCKM) ?1
#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub enum Decckm {
    #[default]
    /// Normal (Reset) Mode
    /// Normal cursor keys in ANSI mode.
    Ansi,
    /// Alternate (Set) Mode
    /// Application cursor keys.
    Application,
    Query,
}

impl Decckm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Application,
            SetMode::DecRst => Self::Ansi,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Decckm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Ansi => String::from("?1;2$y"),
                Self::Application => String::from("?1;1$y"),
                Self::Query => String::from("?1;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("?1;1$y"),
                SetMode::DecRst => String::from("?1;2$y"),
                SetMode::DecQuery => String::from("?1;0$y"),
            },
        )
    }
}

impl fmt::Display for Decckm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ansi => write!(f, "Cursor Key Mode (DECCKM) ANSI"),
            Self::Application => write!(f, "Cursor Key Mode (DECCKM) Application"),
            Self::Query => write!(f, "Cursor Key Mode (DECCKM) Query"),
        }
    }
}
