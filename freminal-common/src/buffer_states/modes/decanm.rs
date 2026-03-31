// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// DECANM — ANSI/VT52 Mode (`?2`)
///
/// - **Set** (`CSI ? 2 h`): ANSI mode (default) — the terminal interprets
///   standard ANSI/VT100+ escape sequences.
/// - **Reset** (`CSI ? 2 l`): VT52 mode — the terminal interprets the
///   reduced VT52 escape set (`ESC A`..`ESC Z`, `ESC Y Pl Pc`, etc.).
///   `ESC <` from VT52 mode returns to ANSI mode.
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decanm {
    /// Set: ANSI mode (default).
    #[default]
    Ansi,
    /// Reset: VT52 compatibility mode.
    Vt52,
    Query,
}

impl ReportMode for Decanm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Ansi => String::from("\x1b[?2;1$y"),
                Self::Vt52 => String::from("\x1b[?2;2$y"),
                Self::Query => String::from("\x1b[?2;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?2;1$y"),
                SetMode::DecRst => String::from("\x1b[?2;2$y"),
                SetMode::DecQuery => String::from("\x1b[?2;0$y"),
            },
        )
    }
}

impl Decanm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Ansi,
            SetMode::DecRst => Self::Vt52,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Decanm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ansi => write!(f, "ANSI Mode (DECANM)"),
            Self::Vt52 => write!(f, "VT52 Mode (DECANM)"),
            Self::Query => write!(f, "Query ANSI/VT52 Mode (DECANM)"),
        }
    }
}
