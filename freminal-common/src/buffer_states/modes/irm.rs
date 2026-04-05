// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Insert/Replace Mode (IRM) — ANSI mode 4.
///
/// When `Insert`, writing a character first shifts existing content one cell
/// to the right (characters at the right margin are lost).  When `Replace`
/// (the default), characters are written in place, overwriting whatever was
/// there.
///
/// Set by `CSI 4 h`, reset by `CSI 4 l`.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum Irm {
    /// Replace mode (default): writing overwrites the cell at the cursor.
    #[default]
    Replace,
    /// Insert mode: writing first inserts a blank cell at the cursor,
    /// shifting existing content one position to the right.
    Insert,
    /// Query: used only during mode-reporting; not stored as runtime state.
    Query,
}

impl Irm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Insert,
            SetMode::DecRst => Self::Replace,
            SetMode::DecQuery => Self::Query,
        }
    }

    /// Returns `true` when insert mode is active.
    #[must_use]
    pub const fn is_insert(&self) -> bool {
        matches!(self, Self::Insert)
    }
}

impl ReportMode for Irm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Insert => String::from("\x1b[4;1$y"),
                Self::Replace => String::from("\x1b[4;2$y"),
                Self::Query => String::from("\x1b[4;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[4;1$y"),
                SetMode::DecRst => String::from("\x1b[4;2$y"),
                SetMode::DecQuery => String::from("\x1b[4;0$y"),
            },
        )
    }
}

impl fmt::Display for Irm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Replace => write!(f, "Insert/Replace Mode (IRM) — Replace (default)"),
            Self::Insert => write!(f, "Insert/Replace Mode (IRM) — Insert"),
            Self::Query => write!(f, "Insert/Replace Mode (IRM) — Query"),
        }
    }
}
