// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use core::fmt;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
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

/// Underline style variants as specified by SGR 4:N subparameters.
///
/// - `None`   — no underline (SGR 4:0 or SGR 24).
/// - `Single` — plain underline (SGR 4 or SGR 4:1).
/// - `Double` — double underline (SGR 4:2).
/// - `Curly`  — wavy/curly underline (SGR 4:3).
/// - `Dotted` — dotted underline (SGR 4:4).
/// - `Dashed` — dashed underline (SGR 4:5).
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

impl UnderlineStyle {
    /// Convert from the SGR 4:N subparameter value.
    ///
    /// Values outside 0–5 map to `None`.
    #[must_use]
    pub const fn from_sgr_param(n: usize) -> Self {
        match n {
            1 => Self::Single,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::None,
        }
    }

    /// Encode as a 3-bit value for storage in [`FontDecorationFlags`].
    const fn to_bits(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Single => 1,
            Self::Double => 2,
            Self::Curly => 3,
            Self::Dotted => 4,
            Self::Dashed => 5,
        }
    }

    /// Decode from a 3-bit value stored in [`FontDecorationFlags`].
    const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Single,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::None,
        }
    }

    /// Returns `true` if this is any active underline style (not `None`).
    #[must_use]
    pub const fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// A compact bitfield representing active font decorations.
///
/// ## Bit layout
///
/// ```text
/// Bit 0:   Italic
/// Bits 1–3: Underline style (3-bit field, 0=none, 1–5=styles)
/// Bit 4:   Faint
/// Bit 5:   Strikethrough
/// ```
///
/// Underline styles are mutually exclusive (only one can be active), so they
/// share a 3-bit field rather than separate bits.
#[derive(Clone, Copy, Eq, PartialEq, Default, Hash)]
pub struct FontDecorationFlags(u8);

impl FontDecorationFlags {
    const ITALIC: u8 = 0b0000_0001;
    const UNDERLINE_MASK: u8 = 0b0000_1110;
    const UNDERLINE_SHIFT: u8 = 1;
    const FAINT: u8 = 0b0001_0000;
    const STRIKETHROUGH: u8 = 0b0010_0000;

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
    ///
    /// For `FontDecorations::Underline`, returns `true` if *any* underline
    /// style is set (single, double, curly, dotted, or dashed).
    #[must_use]
    pub const fn contains(self, dec: FontDecorations) -> bool {
        match dec {
            FontDecorations::Underline => (self.0 & Self::UNDERLINE_MASK) != 0,
            _ => (self.0 & Self::simple_bit(dec)) != 0,
        }
    }

    /// Add a decoration to the set (idempotent).
    ///
    /// For `FontDecorations::Underline`, sets the style to `Single`.
    /// To set a specific underline style, use [`set_underline_style`](Self::set_underline_style).
    pub const fn insert(&mut self, dec: FontDecorations) {
        match dec {
            FontDecorations::Underline => {
                self.set_underline_style(UnderlineStyle::Single);
            }
            _ => {
                self.0 |= Self::simple_bit(dec);
            }
        }
    }

    /// Remove a decoration from the set (idempotent).
    ///
    /// For `FontDecorations::Underline`, clears all underline styles.
    pub const fn remove(&mut self, dec: FontDecorations) {
        match dec {
            FontDecorations::Underline => {
                self.0 &= !Self::UNDERLINE_MASK;
            }
            _ => {
                self.0 &= !Self::simple_bit(dec);
            }
        }
    }

    /// Bit mask for non-underline decorations.
    const fn simple_bit(dec: FontDecorations) -> u8 {
        match dec {
            FontDecorations::Italic => Self::ITALIC,
            FontDecorations::Faint => Self::FAINT,
            FontDecorations::Strikethrough => Self::STRIKETHROUGH,
            // Underline is handled via the 3-bit field; this arm is unreachable
            // from callers that route Underline to the UNDERLINE_MASK path, but
            // we need an exhaustive match.
            FontDecorations::Underline => Self::UNDERLINE_MASK,
        }
    }

    /// Set the underline style.
    ///
    /// `UnderlineStyle::None` clears the underline; any other value activates
    /// the corresponding style.
    pub const fn set_underline_style(&mut self, style: UnderlineStyle) {
        self.0 = (self.0 & !Self::UNDERLINE_MASK) | (style.to_bits() << Self::UNDERLINE_SHIFT);
    }

    /// Return the current underline style.
    #[must_use]
    pub const fn underline_style(self) -> UnderlineStyle {
        UnderlineStyle::from_bits((self.0 & Self::UNDERLINE_MASK) >> Self::UNDERLINE_SHIFT)
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
        let style = self.underline_style();
        if style.is_active() && style != UnderlineStyle::Single {
            list.entry(&style);
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
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, Hash)]
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

    // --- UnderlineStyle tests ---

    #[test]
    fn insert_underline_defaults_to_single() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Underline);
        assert!(flags.contains(FontDecorations::Underline));
        assert_eq!(flags.underline_style(), UnderlineStyle::Single);
    }

    #[test]
    fn set_underline_style_curly() {
        let mut flags = FontDecorationFlags::empty();
        flags.set_underline_style(UnderlineStyle::Curly);
        assert!(flags.contains(FontDecorations::Underline));
        assert_eq!(flags.underline_style(), UnderlineStyle::Curly);
    }

    #[test]
    fn set_underline_style_none_clears_underline() {
        let mut flags = FontDecorationFlags::empty();
        flags.set_underline_style(UnderlineStyle::Double);
        assert!(flags.contains(FontDecorations::Underline));
        flags.set_underline_style(UnderlineStyle::None);
        assert!(!flags.contains(FontDecorations::Underline));
        assert_eq!(flags.underline_style(), UnderlineStyle::None);
    }

    #[test]
    fn remove_underline_clears_all_styles() {
        let mut flags = FontDecorationFlags::empty();
        flags.set_underline_style(UnderlineStyle::Dashed);
        flags.remove(FontDecorations::Underline);
        assert!(!flags.contains(FontDecorations::Underline));
        assert_eq!(flags.underline_style(), UnderlineStyle::None);
    }

    #[test]
    fn underline_styles_do_not_interfere_with_other_decorations() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Italic);
        flags.insert(FontDecorations::Faint);
        flags.insert(FontDecorations::Strikethrough);
        flags.set_underline_style(UnderlineStyle::Dotted);

        assert!(flags.contains(FontDecorations::Italic));
        assert!(flags.contains(FontDecorations::Faint));
        assert!(flags.contains(FontDecorations::Strikethrough));
        assert_eq!(flags.underline_style(), UnderlineStyle::Dotted);
    }

    #[test]
    fn all_underline_styles_round_trip() {
        let styles = [
            UnderlineStyle::None,
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ];
        for style in styles {
            let mut flags = FontDecorationFlags::empty();
            flags.set_underline_style(style);
            assert_eq!(flags.underline_style(), style);
        }
    }

    #[test]
    fn underline_style_from_sgr_param() {
        assert_eq!(UnderlineStyle::from_sgr_param(0), UnderlineStyle::None);
        assert_eq!(UnderlineStyle::from_sgr_param(1), UnderlineStyle::Single);
        assert_eq!(UnderlineStyle::from_sgr_param(2), UnderlineStyle::Double);
        assert_eq!(UnderlineStyle::from_sgr_param(3), UnderlineStyle::Curly);
        assert_eq!(UnderlineStyle::from_sgr_param(4), UnderlineStyle::Dotted);
        assert_eq!(UnderlineStyle::from_sgr_param(5), UnderlineStyle::Dashed);
        assert_eq!(UnderlineStyle::from_sgr_param(6), UnderlineStyle::None);
        assert_eq!(UnderlineStyle::from_sgr_param(255), UnderlineStyle::None);
    }

    #[test]
    fn switching_underline_style_preserves_other_flags() {
        let mut flags = FontDecorationFlags::empty();
        flags.insert(FontDecorations::Italic);
        flags.insert(FontDecorations::Strikethrough);
        flags.set_underline_style(UnderlineStyle::Single);

        // Switch style
        flags.set_underline_style(UnderlineStyle::Curly);
        assert_eq!(flags.underline_style(), UnderlineStyle::Curly);
        assert!(flags.contains(FontDecorations::Italic));
        assert!(flags.contains(FontDecorations::Strikethrough));
    }
}
