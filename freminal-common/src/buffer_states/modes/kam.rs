// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Keyboard Action Mode (KAM) — ANSI mode 2.
///
/// `CSI 2 h` locks the keyboard, `CSI 2 l` unlocks it.
///
/// Freminal recognises KAM and does not implement it: the request is logged
/// and nothing changes. Letting any program lock the keyboard is hostile, and
/// kitty and `WezTerm` ignore it too (Ghostty only honours it behind an
/// off-by-default option). Because the mode is not implemented, DECRQM answers
/// `Ps = 0` (not recognised) whatever the variant, in the ANSI form (no `?`).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum Kam {
    /// Keyboard locked (`CSI 2 h`).
    Locked,
    /// Keyboard unlocked (`CSI 2 l`).
    #[default]
    Unlocked,
    /// DECRQM query (`CSI 2 $ p`).
    Query,
}

impl Kam {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Locked,
            SetMode::DecRst => Self::Unlocked,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Kam {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        String::from("2;0$y")
    }
}

impl fmt::Display for Kam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Locked => write!(f, "Keyboard Action Mode (KAM) — Locked"),
            Self::Unlocked => write!(f, "Keyboard Action Mode (KAM) — Unlocked"),
            Self::Query => write!(f, "Keyboard Action Mode (KAM) — Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(Kam::new(&SetMode::DecSet), Kam::Locked);
        assert_eq!(Kam::new(&SetMode::DecRst), Kam::Unlocked);
        assert_eq!(Kam::new(&SetMode::DecQuery), Kam::Query);
    }

    #[test]
    fn default_is_unlocked() {
        assert_eq!(Kam::default(), Kam::Unlocked);
    }

    #[test]
    fn report_is_not_recognised_in_the_ansi_form_for_every_variant_and_override() {
        for kam in [Kam::Locked, Kam::Unlocked, Kam::Query] {
            assert_eq!(kam.report(None), "2;0$y");
            assert_eq!(kam.report(Some(SetMode::DecSet)), "2;0$y");
            assert_eq!(kam.report(Some(SetMode::DecRst)), "2;0$y");
            assert_eq!(kam.report(Some(SetMode::DecQuery)), "2;0$y");
        }
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(
            Kam::Locked.to_string(),
            "Keyboard Action Mode (KAM) — Locked"
        );
        assert_eq!(
            Kam::Unlocked.to_string(),
            "Keyboard Action Mode (KAM) — Unlocked"
        );
        assert_eq!(Kam::Query.to_string(), "Keyboard Action Mode (KAM) — Query");
    }
}
