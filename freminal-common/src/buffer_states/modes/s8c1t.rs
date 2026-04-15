// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

/// S8C1T / S7C1T — 8-bit / 7-bit C1 Control Transmission
///
/// - **`ESC SP G`** (S8C1T): Enable 8-bit C1 controls.  The terminal accepts
///   single-byte C1 controls (0x80–0x9F) as equivalents of their 7-bit ESC-based
///   counterparts (e.g. 0x9B = CSI = `ESC [`).
/// - **`ESC SP F`** (S7C1T): Disable 8-bit C1 controls (default).  Only 7-bit
///   escape sequences are recognized; bytes 0x80–0x9F are treated as data.
///
/// This mode is set by `ESC SP G` / `ESC SP F`, not by a CSI private mode
/// sequence, so it does not participate in DECRPM queries.
#[derive(Debug, Eq, PartialEq, Default, Clone, Copy)]
pub enum S8c1t {
    /// Default: only 7-bit escape sequences are recognized.
    #[default]
    SevenBit,
    /// 8-bit C1 controls (0x80–0x9F) are recognized as control introducers.
    EightBit,
}

impl fmt::Display for S8c1t {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SevenBit => write!(f, "7-bit C1 Controls (S7C1T)"),
            Self::EightBit => write!(f, "8-bit C1 Controls (S8C1T)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_seven_bit() {
        assert_eq!(S8c1t::SevenBit.to_string(), "7-bit C1 Controls (S7C1T)");
    }

    #[test]
    fn display_eight_bit() {
        assert_eq!(S8c1t::EightBit.to_string(), "8-bit C1 Controls (S8C1T)");
    }

    #[test]
    fn default_is_seven_bit() {
        assert_eq!(S8c1t::default(), S8c1t::SevenBit);
    }
}
