// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// DECNKM — Numeric Keypad Mode (`?66`)
///
/// This is the DECSET/DECRST alias for keypad application mode.
/// `CSI ? 66 h` → application mode (same as `ESC =` / DECKPAM).
/// `CSI ? 66 l` → numeric mode (same as `ESC >` / DECKPNM).
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decnkm {
    #[default]
    /// Numeric (Normal) Mode — DECRST ?66
    Numeric,
    /// Application Mode — DECSET ?66
    Application,
    /// DECRQM query
    Query,
}

impl Decnkm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Application,
            SetMode::DecRst => Self::Numeric,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Decnkm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Application => String::from("\x1b[?66;1$y"),
                Self::Numeric => String::from("\x1b[?66;2$y"),
                Self::Query => String::from("\x1b[?66;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?66;1$y"),
                SetMode::DecRst => String::from("\x1b[?66;2$y"),
                SetMode::DecQuery => String::from("\x1b[?66;0$y"),
            },
        )
    }
}

impl fmt::Display for Decnkm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Application => write!(f, "Keypad Application Mode (DECNKM)"),
            Self::Numeric => write!(f, "Keypad Numeric Mode (DECNKM)"),
            Self::Query => write!(f, "Query Keypad Mode (DECNKM)"),
        }
    }
}
