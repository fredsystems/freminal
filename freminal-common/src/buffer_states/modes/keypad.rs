// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

/// Keypad Mode (DECPAM / DECPNM)
///
/// DECPAM (`ESC =`) switches the keypad to application mode.
/// DECPNM (`ESC >`) switches the keypad to numeric (normal) mode.
///
/// Default is `Numeric`.
#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub enum KeypadMode {
    #[default]
    /// Numeric (Normal) Mode — keypad sends digit/operator characters.
    Numeric,
    /// Application Mode — keypad sends escape sequences.
    Application,
}

impl fmt::Display for KeypadMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numeric => write!(f, "Keypad Mode: Numeric (DECPNM)"),
            Self::Application => write!(f, "Keypad Mode: Application (DECPAM)"),
        }
    }
}
