// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Screen Mode (DECSCNM) ?5
///
/// Controls normal vs reverse video display.
/// When set (`ReverseDisplay`), the screen uses reverse video (dark text on light background).
/// When reset (`NormalDisplay`, default), the screen uses normal video (light text on dark background).
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum Decscnm {
    #[default]
    NormalDisplay,
    ReverseDisplay,
    Query,
}

impl ReportMode for Decscnm {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::NormalDisplay => String::from("?5;2$y"),
                Self::ReverseDisplay => String::from("?5;1$y"),
                Self::Query => String::from("?5;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("?5;1$y"),
                SetMode::DecRst => String::from("?5;2$y"),
                SetMode::DecQuery => String::from("?5;0$y"),
            },
        )
    }
}

impl Decscnm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::ReverseDisplay,
            SetMode::DecRst => Self::NormalDisplay,
            SetMode::DecQuery => Self::Query,
        }
    }

    /// The display mode as transported to the renderer.
    ///
    /// `Query` is a transient request marker that is never stored as the
    /// current mode, so mapping it to [`ScreenInversion::Normal`] exists only
    /// to keep this function total.
    #[must_use]
    pub const fn screen_inversion(&self) -> ScreenInversion {
        match self {
            Self::ReverseDisplay => ScreenInversion::Reversed,
            Self::NormalDisplay | Self::Query => ScreenInversion::Normal,
        }
    }
}

/// Whether the screen is displayed in normal or reverse video (DECSCNM),
/// as transported to the renderer. Unlike [`Decscnm`] it has no `Query` state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum ScreenInversion {
    #[default]
    Normal,
    Reversed,
}

impl fmt::Display for Decscnm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NormalDisplay => write!(f, "Normal Display"),
            Self::ReverseDisplay => write!(f, "Reverse Display"),
            Self::Query => write!(f, "Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_display_maps_to_normal() {
        assert_eq!(
            Decscnm::NormalDisplay.screen_inversion(),
            ScreenInversion::Normal
        );
    }

    #[test]
    fn reverse_display_maps_to_reversed() {
        assert_eq!(
            Decscnm::ReverseDisplay.screen_inversion(),
            ScreenInversion::Reversed
        );
    }

    #[test]
    fn query_maps_to_normal() {
        assert_eq!(Decscnm::Query.screen_inversion(), ScreenInversion::Normal);
    }

    #[test]
    fn default_screen_inversion_is_normal() {
        assert_eq!(ScreenInversion::default(), ScreenInversion::Normal);
    }
}
