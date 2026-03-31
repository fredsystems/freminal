// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::mode::SetMode;

use super::ReportMode;

/// Sixel Display Mode (DECSDM) `?80`
///
/// Controls cursor behavior after a Sixel image is placed:
/// - **Set** (`CSI ? 80 h`): Display Mode — cursor does NOT advance past the
///   image (image overwrites in-place).
/// - **Reset** (`CSI ? 80 l`): Scrolling Mode (default) — cursor advances below
///   the image, scrolling if needed.
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decsdm {
    /// Set: Display Mode — no cursor advancement after Sixel placement.
    DisplayMode,
    #[default]
    /// Reset: Scrolling Mode — cursor advances past the image (default).
    ScrollingMode,
    Query,
}

impl ReportMode for Decsdm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::ScrollingMode => String::from("\x1b[?80;2$y"),
                Self::DisplayMode => String::from("\x1b[?80;1$y"),
                Self::Query => String::from("\x1b[?80;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?80;1$y"),
                SetMode::DecRst => String::from("\x1b[?80;2$y"),
                SetMode::DecQuery => String::from("\x1b[?80;0$y"),
            },
        )
    }
}

impl Decsdm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::DisplayMode,
            SetMode::DecRst => Self::ScrollingMode,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl fmt::Display for Decsdm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayMode => write!(f, "Sixel Display Mode (DECSDM)"),
            Self::ScrollingMode => write!(f, "Sixel Scrolling Mode (DECSDM)"),
            Self::Query => write!(f, "Query Sixel Display Mode (DECSDM)"),
        }
    }
}
