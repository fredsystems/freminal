// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::buffer_states::{mode::SetMode, modes::ReportMode};
use core::fmt;
/// Show cursor (DECOM) ?6
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decom {
    #[default]
    NormalCursor,
    OriginMode,
    Query,
}

impl ReportMode for Decom {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NormalCursor => String::from("\x1b[?6;2$y"),
                Self::OriginMode => String::from("\x1b[?6;1$y"),
                Self::Query => String::from("\x1b[?6;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?6;1$y"),
                SetMode::DecRst => String::from("\x1b[?6;2$y"),
                SetMode::DecQuery => String::from("\x1b[?6;0$y"),
            },
        )
    }
}

impl Decom {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::OriginMode,
            SetMode::DecRst => Self::NormalCursor,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Decom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NormalCursor => write!(f, "Normal Cursor"),
            Self::OriginMode => write!(f, "Origin Mode"),
            Self::Query => write!(f, "Query"),
        }
    }
}
