// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Synchronized Updates Mode ?2027
#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub enum GraphemeClustering {
    #[default]
    /// Normal (Reset) Mode
    Unicode,
    /// Alternate (Set) Mode
    Legacy,
    Query,
}

impl GraphemeClustering {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Legacy,
            SetMode::DecRst => Self::Unicode,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for GraphemeClustering {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Unicode | Self::Legacy => String::from("\x1b[?2027;3$y"),
                Self::Query => String::from("\x1b[?2027;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet | SetMode::DecRst => String::from("\x1b[?2027;3$y"),
                SetMode::DecQuery => String::from("\x1b[?2027;0$y"),
            },
        )
    }
}

impl fmt::Display for GraphemeClustering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unicode => write!(f, "Grapheme Clustering Mode (DEC 2027) Unicode"),
            Self::Legacy => write!(f, "Grapheme Clustering Mode (DEC 2027) Legacy"),
            Self::Query => write!(f, "Grapheme Clustering Mode (DEC 2027) Query"),
        }
    }
}
