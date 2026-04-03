// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Set number of columns (DECCOLM) ?3
///
/// DECSET `?3` sets 132-column mode (`Column132`) and DECRST `?3` sets
/// 80-column mode (`Column80`).
///
/// **Non-standard default:** The `#[default]` here is `Column132` (the
/// *set* state).  The DEC spec and most terminals default to 80 columns,
/// but Freminal does not resize the grid in response to DECCOLM — the
/// terminal is always pixel-resized by the GUI.  `Column132` is kept as
/// default purely so that `Deccolm::new(&SetMode::DecSet)` round-trips
/// correctly through `report()`, which maps `Column132 → ESC[?3;1$y`
/// (mode set).  Applications that query `DECRQM ?3` therefore always
/// receive "set" (1), which is the semantically correct response for a
/// terminal that does not enforce a column limit.
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
