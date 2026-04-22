// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::themes::ThemePalette;
use conv2::ValueInto;
use std::fmt;
use thiserror::Error;

/// Errors produced when parsing a color from a string representation.
#[derive(Debug, Error, Eq, PartialEq, Clone)]
pub enum ColorParseError {
    /// The input string did not match any recognised named color.
    #[error("invalid color name: {0:?}")]
    InvalidName(String),
}

/// Number of entries in the 256-color palette.
pub const PALETTE_SIZE: usize = 256;

/// A mutable 256-color palette with optional per-index overrides.
///
/// `None` means "use the default color for this index".
/// `Some((r,g,b))` means "use this specific RGB value".
#[derive(Clone, Debug)]
pub struct ColorPalette {
    overrides: Box<[Option<(u8, u8, u8)>; PALETTE_SIZE]>,
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self {
            overrides: Box::new([None; PALETTE_SIZE]),
        }
    }
}

impl PartialEq for ColorPalette {
    fn eq(&self, other: &Self) -> bool {
        self.overrides[..] == other.overrides[..]
    }
}

impl Eq for ColorPalette {}

impl ColorPalette {
    /// Set palette entry at `index` to the given RGB value.
    pub fn set(&mut self, index: u8, r: u8, g: u8, b: u8) {
        self.overrides[usize::from(index)] = Some((r, g, b));
    }

    /// Reset a single palette entry to its default.
    pub fn reset(&mut self, index: u8) {
        self.overrides[usize::from(index)] = None;
    }

    /// Reset all palette entries to their defaults.
    pub fn reset_all(&mut self) {
        *self.overrides = [None; PALETTE_SIZE];
    }

    /// Look up the effective color for `index`, consulting overrides first.
    #[must_use]
    pub fn lookup(&self, index: usize, theme: &ThemePalette) -> TerminalColor {
        if let Some(Some((r, g, b))) = self.overrides.get(index) {
            TerminalColor::Custom(*r, *g, *b)
        } else {
            lookup_default_256_color(index, theme)
        }
    }

    /// Get the current RGB value for a palette index (override or default).
    #[must_use]
    pub fn rgb(&self, index: u8, theme: &ThemePalette) -> (u8, u8, u8) {
        if let Some((r, g, b)) = self.overrides[usize::from(index)] {
            (r, g, b)
        } else {
            default_index_to_rgb(index, theme)
        }
    }
}

/// Look up the default (un-overridden) color for a 256-color index.
///
/// This is the original stateless lookup. Prefer `ColorPalette::lookup()`
/// when a palette with overrides is available.
#[must_use]
pub fn lookup_256_color_by_index(index: usize, theme: &ThemePalette) -> TerminalColor {
    lookup_default_256_color(index, theme)
}

/// Internal default lookup — used by both `lookup_256_color_by_index` and `ColorPalette`.
#[must_use]
fn lookup_default_256_color(index: usize, theme: &ThemePalette) -> TerminalColor {
    // https://stackoverflow.com/questions/69138165/how-to-get-the-rgb-values-of-a-256-color-palette-terminal-color
    match index {
        // Standard ANSI colors (0–15): read from the theme palette
        0..=15 => {
            let (r, g, b) = theme.ansi[index];
            TerminalColor::Custom(r, g, b)
        }
        // Index 16 is the first entry of the 6x6x6 color cube and evaluates to
        // (0, 0, 0) — the same as the theme's ANSI black.  Out-of-range indices
        // (256+) are clamped to black as a safe fallback.
        16 | 256.. => {
            let (r, g, b) = theme.ansi[0];
            TerminalColor::Custom(r, g, b)
        }
        // gray scale
        232..=255 => {
            let value = (2056 + 2570 * (index - 232)) / 256;

            // use conv2 crate to ensure safe casting
            let value: u8 = value.value_into().unwrap_or(0);
            TerminalColor::Custom(value, value, value)
        }
        // programmatic colors (6x6x6 color cube)
        _ => {
            let r = cube_component(index, 36).value_into().unwrap_or(0);
            let g = cube_component(index, 6).value_into().unwrap_or(0);
            let b = cube_component(index, 1).value_into().unwrap_or(0);
            TerminalColor::Custom(r, g, b)
        }
    }
}

#[must_use]
pub const fn cube_component(value: usize, modifier: usize) -> usize {
    let i = ((value - 16) / modifier) % 6;

    if i == 0 { 0 } else { (14135 + 10280 * i) / 256 }
}

/// Map a default palette index to its RGB triple.
///
/// For named ANSI colors (0-15), we use the theme's ansi palette values.
/// For 16-231 and 232-255 we use the standard xterm cube / greyscale formulas.
#[must_use]
pub fn default_index_to_rgb(index: u8, theme: &ThemePalette) -> (u8, u8, u8) {
    match index {
        // ANSI colors 0-15: read from theme
        0..=15 => theme.ansi[usize::from(index)],
        // Greyscale ramp 232-255
        232..=255 => {
            let value = (2056 + 2570 * (usize::from(index) - 232)) / 256;
            let v: u8 = value.value_into().unwrap_or(0);
            (v, v, v)
        }
        // 6x6x6 color cube 16-231
        _ => {
            let idx = usize::from(index);
            let r: u8 = cube_component(idx, 36).value_into().unwrap_or(0);
            let g: u8 = cube_component(idx, 6).value_into().unwrap_or(0);
            let b: u8 = cube_component(idx, 1).value_into().unwrap_or(0);
            (r, g, b)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum TerminalColor {
    Default,
    DefaultBackground,
    DefaultUnderlineColor,
    DefaultCursorColor,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightYellow,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Custom(u8, u8, u8),
    /// A 256-color palette index (0–255).
    ///
    /// Produced by the SGR `38;5;N` / `48;5;N` / `58;5;N` parser paths.
    /// The handler resolves this against the mutable `ColorPalette` before
    /// applying it to cells.  If it somehow reaches the GUI unresolved,
    /// the default palette lookup is used as a fallback.
    PaletteIndex(u8),
}

impl TerminalColor {
    #[must_use]
    pub const fn default_to_regular(self) -> Self {
        match self {
            Self::Default | Self::DefaultUnderlineColor | Self::DefaultCursorColor => Self::White,
            Self::DefaultBackground => Self::Black,
            _ => self,
        }
    }

    /// Resolve a `PaletteIndex` to its default color (stateless).
    ///
    /// Returns `self` unchanged for all other variants.
    #[must_use]
    pub fn resolve_palette_default(self, theme: &ThemePalette) -> Self {
        match self {
            Self::PaletteIndex(idx) => lookup_default_256_color(usize::from(idx), theme),
            other => other,
        }
    }
}

impl fmt::Display for TerminalColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Default => "default",
            Self::Black => "black",
            Self::Red => "red",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Cyan => "cyan",
            Self::White => "white",
            Self::BrightYellow => "bright yellow",
            Self::BrightBlack => "bright black",
            Self::BrightRed => "bright red",
            Self::BrightGreen => "bright green",
            Self::BrightBlue => "bright blue",
            Self::BrightMagenta => "bright magenta",
            Self::BrightCyan => "bright cyan",
            Self::BrightWhite => "bright white",
            Self::DefaultUnderlineColor => "default underline color",
            Self::DefaultBackground => "default background",
            Self::DefaultCursorColor => "default cursor color",
            Self::Custom(r, g, b) => {
                return write!(f, "rgb({r}, {g}, {b})");
            }
            Self::PaletteIndex(idx) => {
                return write!(f, "palette({idx})");
            }
        };

        f.write_str(s)
    }
}

impl std::str::FromStr for TerminalColor {
    type Err = ColorParseError;

    fn from_str(s: &str) -> Result<Self, ColorParseError> {
        let ret = match s {
            "default" => Self::Default,
            "default_background" => Self::DefaultBackground,
            "default_underline_color" => Self::DefaultUnderlineColor,
            "default_cursor_color" => Self::DefaultCursorColor,
            "black" => Self::Black,
            "red" => Self::Red,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "blue" => Self::Blue,
            "magenta" => Self::Magenta,
            "cyan" => Self::Cyan,
            "white" => Self::White,
            "bright yellow" => Self::BrightYellow,
            "bright black" => Self::BrightBlack,
            "bright red" => Self::BrightRed,
            "bright green" => Self::BrightGreen,
            "bright blue" => Self::BrightBlue,
            "bright magenta" => Self::BrightMagenta,
            "bright cyan" => Self::BrightCyan,
            "bright white" => Self::BrightWhite,
            _ => return Err(ColorParseError::InvalidName(s.to_string())),
        };
        Ok(ret)
    }
}

/// Parse an X11 color spec string to an RGB triple.
///
/// Supported formats:
/// - `rgb:R/G/B` where R, G, B are 1–4 hex digits each (`XParseColor` format)
/// - `#RRGGBB` (6-digit hex)
/// - `#RGB` (3-digit hex, expanded to 6)
///
/// Returns `None` if the string is not a recognised color spec.
#[must_use]
pub fn parse_color_spec(spec: &str) -> Option<(u8, u8, u8)> {
    if let Some(rest) = spec.strip_prefix("rgb:") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() != 3 {
            return None;
        }
        let r = scale_hex_channel(parts[0])?;
        let g = scale_hex_channel(parts[1])?;
        let b = scale_hex_channel(parts[2])?;
        Some((r, g, b))
    } else if let Some(hex) = spec.strip_prefix('#') {
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some((r, g, b))
            }
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                // Expand: 0xA → 0xAA
                Some((r * 17, g * 17, b * 17))
            }
            _ => None,
        }
    } else {
        None
    }
}

/// Scale a 1–4 hex-digit channel value to 8-bit.
///
/// Follows the `XParseColor` convention:
/// - 1 digit:  0xH   → (H << 4) | H  (e.g. `a` → 0xaa)
/// - 2 digits: 0xHH  → HH as-is
/// - 3 digits: 0xHHH → top 8 bits (shift right 4)
/// - 4 digits: 0xHHHH → top 8 bits (shift right 8)
#[must_use]
pub fn scale_hex_channel(s: &str) -> Option<u8> {
    let v = u16::from_str_radix(s, 16).ok()?;
    let scaled = match s.len() {
        1 => (v << 4) | v,
        2 => v,
        3 => v >> 4,
        4 => v >> 8,
        _ => return None,
    };
    u8::try_from(scaled).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_hex_channel_five_digit_returns_none() {
        // Length 5 is not in 1–4 → the `_ => return None` arm
        assert_eq!(scale_hex_channel("fffff"), None);
    }

    #[test]
    fn scale_hex_channel_empty_returns_none() {
        // Empty string: from_str_radix will fail → None via `?`
        assert_eq!(scale_hex_channel(""), None);
    }

    #[test]
    fn scale_hex_channel_one_digit() {
        // 'a' → 0xaa = 170
        assert_eq!(scale_hex_channel("a"), Some(0xaa));
    }

    #[test]
    fn scale_hex_channel_two_digits() {
        assert_eq!(scale_hex_channel("ff"), Some(0xff));
        assert_eq!(scale_hex_channel("80"), Some(0x80));
    }

    #[test]
    fn scale_hex_channel_three_digits() {
        // 0xABC >> 4 = 0xAB = 171
        assert_eq!(scale_hex_channel("ABC"), Some(0xAB));
    }

    #[test]
    fn scale_hex_channel_four_digits() {
        // 0xABCD >> 8 = 0xAB = 171
        assert_eq!(scale_hex_channel("ABCD"), Some(0xAB));
    }
}
