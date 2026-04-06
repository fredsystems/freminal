// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum FontWeight {
    #[default]
    Normal,
    Bold,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FontDecorations {
    Italic,
    Underline,
    Faint,
    Strikethrough,
}

/// A compact bitfield representing active font decorations (italic, underline,
/// faint, strikethrough).
///
/// Replaces `Vec<FontDecorations>` to eliminate per-cell heap allocation when
/// cloning `FormatTag` values. The four possible decorations map to individual
/// bits; set operations are O(1) bitwise ops.
#[derive(Clone, Copy, Eq, PartialEq, Default, Hash)]
pub struct FontDecorationFlags(u8);

impl FontDecorationFlags {
    const ITALIC: u8 = 0b0001;
    const UNDERLINE: u8 = 0b0010;
    const FAINT: u8 = 0b0100;
    const STRIKETHROUGH: u8 = 0b1000;

    /// An empty decoration set (no decorations active).
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns `true` if no decorations are active.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if the given decoration is active.
    #[must_use]
    pub const fn contains(self, dec: FontDecorations) -> bool {
        self.0 & Self::bit(dec) != 0
    }

    /// Add a decoration to the set (idempotent).
    pub const fn insert(&mut self, dec: FontDecorations) {
        self.0 |= Self::bit(dec);
    }

    /// Remove a decoration from the set (idempotent).
    pub const fn remove(&mut self, dec: FontDecorations) {
        self.0 &= !Self::bit(dec);
    }

    const fn bit(dec: FontDecorations) -> u8 {
        match dec {
            FontDecorations::Italic => Self::ITALIC,
            FontDecorations::Underline => Self::UNDERLINE,
            FontDecorations::Faint => Self::FAINT,
            FontDecorations::Strikethrough => Self::STRIKETHROUGH,
        }
    }

    /// Iterate over all active decorations.
    #[must_use]
    pub const fn iter(self) -> FontDecorationFlagsIter {
        FontDecorationFlagsIter {
            flags: self,
            index: 0,
        }
    }
}

impl fmt::Debug for FontDecorationFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut list = f.debug_set();
        for dec in self.iter() {
            list.entry(&dec);
        }
        list.finish()
    }
}

/// Iterator over the active decorations in a [`FontDecorationFlags`] bitfield.
pub struct FontDecorationFlagsIter {
    flags: FontDecorationFlags,
    index: u8,
}

const ALL_DECORATIONS: [FontDecorations; 4] = [
    FontDecorations::Italic,
    FontDecorations::Underline,
    FontDecorations::Faint,
    FontDecorations::Strikethrough,
];

impl Iterator for FontDecorationFlagsIter {
    type Item = FontDecorations;

    fn next(&mut self) -> Option<Self::Item> {
        while (self.index as usize) < ALL_DECORATIONS.len() {
            let dec = ALL_DECORATIONS[self.index as usize];
            self.index += 1;
            if self.flags.contains(dec) {
                return Some(dec);
            }
        }
        None
    }
}

/// Blink state for text rendered with SGR 5 (slow blink) or SGR 6 (fast blink).
///
/// - `None` — no blink (default).
/// - `Slow` — SGR 5: ~1 Hz (500 ms on, 500 ms off).
/// - `Fast` — SGR 6: ~3 Hz (~167 ms on, ~167 ms off).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum BlinkState {
    #[default]
    None,
    Slow,
    Fast,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_flags_is_empty() {
        let flags = FontDecorationFlags::empty();
        assert!(flags.is_empty());
        assert!(!flags.contains(FontDecorations::Italic));
        assert!(!flags.contains(FontDecorations::Underline));
        assert!(!flags.contains(FontDecorations::Faint));
        assert!(!flags.contains(FontDecorations::Strikethrough));
    }

    #[test]
    fn insert_and_contains() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Italic);
        assert!(flags.contains(FontDecorations::Italic));
        assert!(!flags.contains(FontDecorations::Underline));
        assert!(!flags.is_empty());
    }

    #[test]
    fn insert_is_idempotent() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Faint);
        flags.insert(FontDecorations::Faint);
        assert!(flags.contains(FontDecorations::Faint));
        assert_eq!(flags.iter().count(), 1);
    }

    #[test]
    fn remove_clears_flag() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Underline);
        flags.insert(FontDecorations::Strikethrough);
        flags.remove(FontDecorations::Underline);
        assert!(!flags.contains(FontDecorations::Underline));
        assert!(flags.contains(FontDecorations::Strikethrough));
    }

    #[test]
    fn iter_yields_all_set_flags() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Italic);
        flags.insert(FontDecorations::Faint);
        flags.insert(FontDecorations::Strikethrough);
        let collected: Vec<_> = flags.iter().collect();
        assert_eq!(
            collected,
            vec![
                FontDecorations::Italic,
                FontDecorations::Faint,
                FontDecorations::Strikethrough,
            ]
        );
    }

    #[test]
    fn default_is_empty() {
        let flags = FontDecorationFlags::default();
        assert!(flags.is_empty());
    }

    #[test]
    fn equality_check() {
        let mut a = FontDecorationFlags::empty();
        let mut b = FontDecorationFlags::empty();
        a.insert(FontDecorations::Italic);
        a.insert(FontDecorations::Underline);
        b.insert(FontDecorations::Underline);
        b.insert(FontDecorations::Italic);
        assert_eq!(a, b);
    }
}
