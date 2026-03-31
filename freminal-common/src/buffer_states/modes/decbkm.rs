// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// DECBKM — Backarrow Key Mode (`?67`)
///
/// Controls what the Backspace key sends to the host:
/// - Set (`CSI ? 67 h`): Backspace sends `BS` (0x08).
/// - Reset (`CSI ? 67 l`): Backspace sends `DEL` (0x7F).
///
/// Default is `BackarrowSendsBs` (set), matching Freminal's historical behavior.
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decbkm {
    #[default]
    /// Backarrow sends BS (0x08) — DECSET ?67 (default)
    BackarrowSendsBs,
    /// Backarrow sends DEL (0x7F) — DECRST ?67
    BackarrowSendsDel,
    /// DECRQM query
    Query,
}

impl Decbkm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::BackarrowSendsBs,
            SetMode::DecRst => Self::BackarrowSendsDel,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Decbkm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::BackarrowSendsBs => String::from("\x1b[?67;1$y"),
                Self::BackarrowSendsDel => String::from("\x1b[?67;2$y"),
                Self::Query => String::from("\x1b[?67;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?67;1$y"),
                SetMode::DecRst => String::from("\x1b[?67;2$y"),
                SetMode::DecQuery => String::from("\x1b[?67;0$y"),
            },
        )
    }
}

impl fmt::Display for Decbkm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackarrowSendsBs => write!(f, "Backarrow sends BS (DECBKM set)"),
            Self::BackarrowSendsDel => write!(f, "Backarrow sends DEL (DECBKM reset)"),
            Self::Query => write!(f, "Query Backarrow Key Mode (DECBKM)"),
        }
    }
}
