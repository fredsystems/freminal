// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Meta sets the eighth bit (`?1034`).
///
/// In xterm, enabling it makes the Meta key set bit 7 of the character it
/// sends (the `eightBitInput` resource).
///
/// Freminal recognises `?1034` and does not implement it: the request is
/// logged and nothing changes. kitty, Ghostty and `WezTerm` don't implement it,
/// and it conflicts with UTF-8 input. Because the mode is not implemented,
/// DECRQM answers `Ps = 0` (not recognised) whatever the variant.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum MetaSendsEighthBit {
    /// `CSI ? 1034 h`.
    Enabled,
    /// `CSI ? 1034 l`.
    #[default]
    Disabled,
    /// DECRQM query (`CSI ? 1034 $ p`).
    Query,
}

impl MetaSendsEighthBit {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Enabled,
            SetMode::DecRst => Self::Disabled,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for MetaSendsEighthBit {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        String::from("?1034;0$y")
    }
}

impl fmt::Display for MetaSendsEighthBit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled => write!(f, "Meta Sends Eighth Bit (?1034) — Enabled"),
            Self::Disabled => write!(f, "Meta Sends Eighth Bit (?1034) — Disabled"),
            Self::Query => write!(f, "Meta Sends Eighth Bit (?1034) — Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(
            MetaSendsEighthBit::new(&SetMode::DecSet),
            MetaSendsEighthBit::Enabled
        );
        assert_eq!(
            MetaSendsEighthBit::new(&SetMode::DecRst),
            MetaSendsEighthBit::Disabled
        );
        assert_eq!(
            MetaSendsEighthBit::new(&SetMode::DecQuery),
            MetaSendsEighthBit::Query
        );
    }

    #[test]
    fn default_is_disabled() {
        assert_eq!(MetaSendsEighthBit::default(), MetaSendsEighthBit::Disabled);
    }

    #[test]
    fn report_is_not_recognised_for_every_variant_and_override() {
        for mode in [
            MetaSendsEighthBit::Enabled,
            MetaSendsEighthBit::Disabled,
            MetaSendsEighthBit::Query,
        ] {
            assert_eq!(mode.report(None), "?1034;0$y");
            assert_eq!(mode.report(Some(SetMode::DecSet)), "?1034;0$y");
            assert_eq!(mode.report(Some(SetMode::DecRst)), "?1034;0$y");
            assert_eq!(mode.report(Some(SetMode::DecQuery)), "?1034;0$y");
        }
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            MetaSendsEighthBit::Enabled.to_string(),
            "Meta Sends Eighth Bit (?1034) — Enabled"
        );
        assert_eq!(
            MetaSendsEighthBit::Disabled.to_string(),
            "Meta Sends Eighth Bit (?1034) — Disabled"
        );
        assert_eq!(
            MetaSendsEighthBit::Query.to_string(),
            "Meta Sends Eighth Bit (?1034) — Query"
        );
    }
}
