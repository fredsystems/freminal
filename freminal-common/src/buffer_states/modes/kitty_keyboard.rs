// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Kitty Keyboard Protocol (KKP) flag type and constants.
//!
//! KKP is a progressive-enhancement keyboard encoding protocol specified at
//! <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>.  Programs push a
//! bitmask of desired flags onto a per-terminal stack via `CSI > flags u` and
//! pop them on exit via `CSI < number u`.  The terminal adjusts its key event
//! encoding based on the top-of-stack flags.

use core::fmt;

/// Bitmask of active Kitty Keyboard Protocol flags.
///
/// The inner `u32` holds the raw flag bits.  Named constants are provided for
/// each defined flag bit.  The stack model and push/pop/set semantics are
/// implemented by `TerminalHandler`; this type is purely the value.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct KittyKeyboardFlags(pub u32);

impl KittyKeyboardFlags {
    /// No flags set — protocol inactive.
    pub const NONE: Self = Self(0);

    /// Bit 0 (1): Disambiguate escape codes.
    ///
    /// Keys that normally produce C0 control bytes (Ctrl+letter, Escape, etc.)
    /// are sent as explicit `CSI u` sequences instead.
    pub const DISAMBIGUATE_ESCAPE_CODES: Self = Self(1);

    /// Bit 1 (2): Report event types.
    ///
    /// Key repeat and key release events are reported in addition to press.
    pub const REPORT_EVENT_TYPES: Self = Self(2);

    /// Bit 2 (4): Report alternate keys.
    ///
    /// Shifted/alt key codes are reported in an additional field.
    pub const REPORT_ALTERNATE_KEYS: Self = Self(4);

    /// Bit 3 (8): Report all keys as escape codes.
    ///
    /// Every key press — including plain printable ASCII — is sent as a CSI u
    /// or legacy functional escape code.  Enter, Tab, and Backspace switch
    /// from legacy bytes to `CSI u` encoding under this flag.
    pub const REPORT_ALL_KEYS_AS_ESCAPE_CODES: Self = Self(8);

    /// Bit 4 (16): Report associated text.
    ///
    /// The Unicode text produced by the key event is appended to the CSI u
    /// sequence.
    pub const REPORT_ASSOCIATED_TEXT: Self = Self(16);

    /// Maximum stack depth per the Kitty protocol specification.
    pub const MAX_STACK_DEPTH: usize = 256;

    /// Returns the inner `u32` flag value.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns `true` when no flags are set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for KittyKeyboardFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KittyKeyboardFlags({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zero() {
        assert_eq!(KittyKeyboardFlags::default().bits(), 0);
    }

    #[test]
    fn named_constants_have_correct_bit_values() {
        assert_eq!(KittyKeyboardFlags::NONE.bits(), 0);
        assert_eq!(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.bits(), 1);
        assert_eq!(KittyKeyboardFlags::REPORT_EVENT_TYPES.bits(), 2);
        assert_eq!(KittyKeyboardFlags::REPORT_ALTERNATE_KEYS.bits(), 4);
        assert_eq!(
            KittyKeyboardFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES.bits(),
            8
        );
        assert_eq!(KittyKeyboardFlags::REPORT_ASSOCIATED_TEXT.bits(), 16);
    }

    #[test]
    fn is_empty_true_for_none() {
        assert!(KittyKeyboardFlags::NONE.is_empty());
        assert!(KittyKeyboardFlags::default().is_empty());
    }

    #[test]
    fn is_empty_false_for_nonzero() {
        assert!(!KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.is_empty());
        assert!(!KittyKeyboardFlags(31).is_empty());
    }

    #[test]
    fn max_stack_depth_is_256() {
        assert_eq!(KittyKeyboardFlags::MAX_STACK_DEPTH, 256);
    }

    #[test]
    fn display_shows_inner_value() {
        let flags = KittyKeyboardFlags(7);
        assert_eq!(format!("{flags}"), "KittyKeyboardFlags(7)");
    }
}
