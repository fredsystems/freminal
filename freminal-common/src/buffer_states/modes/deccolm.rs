// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Set number of columns (DECCOLM) ?3
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Deccolm {
    Column80,
    #[default]
    Column132,
    Query,
}

impl ReportMode for Deccolm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Column132 => String::from("\x1b[?3;1$y"),
                Self::Column80 => String::from("\x1b[?3;2$y"),
                Self::Query => String::from("\x1b[?3;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?3;1$y"),
                SetMode::DecRst => String::from("\x1b[?3;2$y"),
                SetMode::DecQuery => String::from("\x1b[?3;0$y"),
            },
        )
    }
}

impl Deccolm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Column132,
            SetMode::DecRst => Self::Column80,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Deccolm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Column80 => write!(f, "80 Column Mode (DECCOLM)"),
            Self::Column132 => write!(f, "132 Column Mode (DECCOLM)"),
            Self::Query => write!(f, "Query Column Mode (DECCOLM)"),
        }
    }
}
