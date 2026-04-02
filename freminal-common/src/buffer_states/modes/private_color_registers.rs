// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Private Color Registers for Sixel (`?1070`)
///
/// Controls whether each Sixel graphic gets its own private color register set
/// or whether all graphics share a single persistent palette:
/// - **Set** (`CSI ? 1070 h`): Private — each image starts with the default
///   VT340 16-color palette (current behavior, default).
/// - **Reset** (`CSI ? 1070 l`): Shared — palette changes in one image persist
///   into subsequent images.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum PrivateColorRegisters {
    /// Set: Each Sixel image uses its own private palette (default).
    #[default]
    Private,
    /// Reset: All Sixel images share a single persistent palette.
    Shared,
    Query,
}

impl ReportMode for PrivateColorRegisters {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Private => String::from("\x1b[?1070;1$y"),
                Self::Shared => String::from("\x1b[?1070;2$y"),
                Self::Query => String::from("\x1b[?1070;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1070;1$y"),
                SetMode::DecRst => String::from("\x1b[?1070;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1070;0$y"),
            },
        )
    }
}

impl PrivateColorRegisters {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Private,
            SetMode::DecRst => Self::Shared,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for PrivateColorRegisters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Private => write!(f, "Private Sixel Color Registers (?1070)"),
            Self::Shared => write!(f, "Shared Sixel Color Registers (?1070)"),
            Self::Query => write!(f, "Query Sixel Color Registers (?1070)"),
        }
    }
}
