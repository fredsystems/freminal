// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Send/Receive Mode (SRM) — ANSI mode 12.
///
/// Controls whether the terminal echoes keyboard input locally.
///
/// Freminal recognises SRM and does not implement it: the request is logged
/// and nothing changes. On a modern tty the kernel already echoes, so local
/// echo would double every character; kitty and `WezTerm` ignore the mode and
/// Ghostty stores it with no consumer. Because the mode is not implemented,
/// DECRQM answers `Ps = 0` (not recognised) whatever the variant, in the ANSI
/// form (no `?`).
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum Srm {
    /// `CSI 12 h`.
    Set,
    /// `CSI 12 l`.
    #[default]
    Reset,
    /// DECRQM query (`CSI 12 $ p`).
    Query,
}

impl Srm {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Set,
            SetMode::DecRst => Self::Reset,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for Srm {
    fn report(&self, _override_mode: Option<SetMode>) -> String {
        String::from("12;0$y")
    }
}

impl fmt::Display for Srm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set => write!(f, "Send/Receive Mode (SRM) — Set"),
            Self::Reset => write!(f, "Send/Receive Mode (SRM) — Reset"),
            Self::Query => write!(f, "Send/Receive Mode (SRM) — Query"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_maps_each_set_mode() {
        assert_eq!(Srm::new(&SetMode::DecSet), Srm::Set);
        assert_eq!(Srm::new(&SetMode::DecRst), Srm::Reset);
        assert_eq!(Srm::new(&SetMode::DecQuery), Srm::Query);
    }

    #[test]
    fn default_is_reset() {
        assert_eq!(Srm::default(), Srm::Reset);
    }

    #[test]
    fn report_is_not_recognised_in_the_ansi_form_for_every_variant_and_override() {
        for srm in [Srm::Set, Srm::Reset, Srm::Query] {
            assert_eq!(srm.report(None), "12;0$y");
            assert_eq!(srm.report(Some(SetMode::DecSet)), "12;0$y");
            assert_eq!(srm.report(Some(SetMode::DecRst)), "12;0$y");
            assert_eq!(srm.report(Some(SetMode::DecQuery)), "12;0$y");
        }
    }

    #[test]
    fn display_names_each_variant() {
        assert_eq!(Srm::Set.to_string(), "Send/Receive Mode (SRM) — Set");
        assert_eq!(Srm::Reset.to_string(), "Send/Receive Mode (SRM) — Reset");
        assert_eq!(Srm::Query.to_string(), "Send/Receive Mode (SRM) — Query");
    }
}
