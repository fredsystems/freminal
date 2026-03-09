// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Line Feed (LNM) 20
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Lnm {
    NewLine,
    #[default]
    LineFeed,
    Query,
}

impl ReportMode for Lnm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NewLine => String::from("\x1b[?20;1$y"),
                Self::LineFeed => String::from("\x1b[?20;2$y"),
                Self::Query => String::from("\x1b[?20;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?20;1$y"),
                SetMode::DecRst => String::from("\x1b[?20;2$y"),
                SetMode::DecQuery => String::from("\x1b[?20;0$y"),
            },
        )
    }
}

impl Lnm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::NewLine,
            SetMode::DecRst => Self::LineFeed,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Lnm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewLine => write!(f, "New Line Mode (LNM)"),
            Self::LineFeed => write!(f, "Line Feed Mode (LNM)"),
            Self::Query => write!(f, "Query Line Mode (LNM)"),
        }
    }
}
