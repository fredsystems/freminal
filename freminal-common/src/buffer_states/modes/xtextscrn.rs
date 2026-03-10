// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

use crate::buffer_states::{mode::SetMode, modes::ReportMode};

/// Alternate Screen (`XT_EXTSCRN`) ?1049
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum XtExtscrn {
    /// Primary screen
    /// Clear screen, switch to normal screen buffer, and restore cursor position.
    #[default]
    Primary,
    /// Save cursor position, switch to alternate screen buffer, and clear screen.
    /// Also known as the "alternate screen".
    Alternate,
    Query,
}

impl XtExtscrn {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Alternate,
            SetMode::DecRst => Self::Primary,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for XtExtscrn {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        override_mode.map_or_else(
            || match self {
                Self::Primary => String::from("\x1b[?1049;2$y"),
                Self::Alternate => String::from("\x1b[?1049;1$y"),
                Self::Query => String::from("\x1b[?1049;0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => String::from("\x1b[?1049;1$y"),
                SetMode::DecRst => String::from("\x1b[?1049;2$y"),
                SetMode::DecQuery => String::from("\x1b[?1049;0$y"),
            },
        )
    }
}

impl fmt::Display for XtExtscrn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary => f.write_str("XT_EXTSCRN (RESET) Primary Screen"),
            Self::Alternate => f.write_str("XT_EXTSCRN (SET) Alternate Screen"),
            Self::Query => f.write_str("XT_EXTSCRN (QUERY)"),
        }
    }
}

/// Legacy alternate screen (?47 / ?1047) — switch buffer without explicit
/// cursor save/restore.
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum AltScreen47 {
    #[default]
    Primary,
    Alternate,
    Query,
}

impl AltScreen47 {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Alternate,
            SetMode::DecRst => Self::Primary,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for AltScreen47 {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        let param = "47";
        override_mode.map_or_else(
            || match self {
                Self::Primary => format!("\x1b[?{param};2$y"),
                Self::Alternate => format!("\x1b[?{param};1$y"),
                Self::Query => format!("\x1b[?{param};0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => format!("\x1b[?{param};1$y"),
                SetMode::DecRst => format!("\x1b[?{param};2$y"),
                SetMode::DecQuery => format!("\x1b[?{param};0$y"),
            },
        )
    }
}

impl fmt::Display for AltScreen47 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary => f.write_str("AltScreen47 (RESET) Primary Screen"),
            Self::Alternate => f.write_str("AltScreen47 (SET) Alternate Screen"),
            Self::Query => f.write_str("AltScreen47 (QUERY)"),
        }
    }
}

/// Save/Restore Cursor Only (?1048) — DECSC/DECRC without screen switch.
#[derive(Debug, Eq, PartialEq, Default, Clone)]
pub enum SaveCursor1048 {
    #[default]
    Restore,
    Save,
    Query,
}

impl SaveCursor1048 {
    #[must_use]
    pub const fn new(mode: &SetMode) -> Self {
        match mode {
            SetMode::DecSet => Self::Save,
            SetMode::DecRst => Self::Restore,
            SetMode::DecQuery => Self::Query,
        }
    }
}

impl ReportMode for SaveCursor1048 {
    fn report(&self, override_mode: Option<SetMode>) -> String {
        let param = "1048";
        override_mode.map_or_else(
            || match self {
                Self::Restore => format!("\x1b[?{param};2$y"),
                Self::Save => format!("\x1b[?{param};1$y"),
                Self::Query => format!("\x1b[?{param};0$y"),
            },
            |override_mode| match override_mode {
                SetMode::DecSet => format!("\x1b[?{param};1$y"),
                SetMode::DecRst => format!("\x1b[?{param};2$y"),
                SetMode::DecQuery => format!("\x1b[?{param};0$y"),
            },
        )
    }
}

impl fmt::Display for SaveCursor1048 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restore => f.write_str("SaveCursor1048 (RESET) Restore Cursor"),
            Self::Save => f.write_str("SaveCursor1048 (SET) Save Cursor"),
            Self::Query => f.write_str("SaveCursor1048 (QUERY)"),
        }
    }
}
