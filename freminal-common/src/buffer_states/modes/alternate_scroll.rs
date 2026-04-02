// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Alternate Scroll Mode (`?1007`)
///
/// When set **and** the alternate screen is active, mouse scroll-wheel
/// events are translated into arrow-key sequences (`Up`/`Down`) sent to
/// the PTY.  When reset, scroll events on the alternate screen are
/// ignored (unless mouse tracking is active, in which case they are
/// reported as mouse events).
///
/// Primary-screen scroll behaviour is unaffected by this mode.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum AlternateScroll {
    #[default]
    /// Disabled — DECRST `?1007` (default)
    Disabled,
    /// Enabled — DECSET `?1007`
    Enabled,
    /// DECRQM query
    Query,
}

impl AlternateScroll {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for AlternateScroll {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Enabled => String::from("\x1b[?1007;1$y"),
                Self::Disabled => String::from("\x1b[?1007;2$y"),
                Self::Query => String::from("\x1b[?1007;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1007;1$y"),
                SetMode::DecRst => String::from("\x1b[?1007;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1007;0$y"),
            },
        )
    }
}

impl fmt::Display for AlternateScroll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => write!(f, "Alternate Scroll Enabled (?1007)"),
            Self::Disabled => write!(f, "Alternate Scroll Disabled (?1007)"),
            Self::Query => write!(f, "Query Alternate Scroll Mode (?1007)"),
        }
    }
}
