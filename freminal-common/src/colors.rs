// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use conv2::ValueInto;
use std::fmt;

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
    pub fn lookup(&self, index: usize) -> TerminalColor {
        if let Some(Some((r, g, b))) = self.overrides.get(index) {
            TerminalColor::Custom(*r, *g, *b)
        } else {
            lookup_default_256_color(index)
        }
    }

    /// Get the current RGB value for a palette index (override or default).
    #[must_use]
    pub fn get_rgb(&self, index: u8) -> (u8, u8, u8) {
        if let Some((r, g, b)) = self.overrides[usize::from(index)] {
            (r, g, b)
        } else {
            default_index_to_rgb(index)
        }
    }
}

/// Look up the default (un-overridden) color for a 256-color index.
///
/// This is the original stateless lookup. Prefer `ColorPalette::lookup()`
/// when a palette with overrides is available.
#[must_use]
pub fn lookup_256_color_by_index(index: usize) -> TerminalColor {
    lookup_default_256_color(index)
}

/// Internal default lookup — used by both `lookup_256_color_by_index` and `ColorPalette`.
#[must_use]
fn lookup_default_256_color(index: usize) -> TerminalColor {
    // https://stackoverflow.com/questions/69138165/how-to-get-the-rgb-values-of-a-256-color-palette-terminal-color
    match index {
        // standard colors 0 -15, as well as their bright counterparts 8-15
        // And the other values that map to them further up the color table
        // Standard ANSI colors (0–7)
        0 | 16 | 256 => TerminalColor::Black,
        1 => TerminalColor::Red,
        2 => TerminalColor::Green,
        3 => TerminalColor::Yellow,
        4 => TerminalColor::Blue,
        5 => TerminalColor::Magenta,
        6 => TerminalColor::Cyan,
        7 => TerminalColor::White,

        // Bright ANSI colors (8–15)
        8 => TerminalColor::BrightBlack,
        9 => TerminalColor::BrightRed,
        10 => TerminalColor::BrightGreen,
        11 => TerminalColor::BrightYellow,
        12 => TerminalColor::BrightBlue,
        13 => TerminalColor::BrightMagenta,
        14 => TerminalColor::BrightCyan,
        15 => TerminalColor::BrightWhite,
        // gray scale
        232..=255 => {
            let value = (2056 + 2570 * (index - 232)) / 256;

            // use conv2 crate to ensure safe casting
            let value: u8 = value.value_into().unwrap_or(0);
            TerminalColor::Custom(value, value, value)
        } // // the blacks
        // 0 | 16 | 256.. => (0, 0, 0),
        // // programtic colors
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
/// For named ANSI colors (0-15), we use the Catppuccin Mocha hex values
/// that the GUI renders.  For 16-231 and 232-255 we use the standard
/// xterm cube / greyscale formulas.
#[must_use]
pub fn default_index_to_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        // Catppuccin Mocha base palette 0-7
        0 => (0x45, 0x47, 0x5a), // Black
        1 => (0xf3, 0x8b, 0xa8), // Red
        2 => (0xa6, 0xe3, 0xa1), // Green
        3 => (0xf9, 0xe2, 0xaf), // Yellow
        4 => (0x89, 0xb4, 0xfa), // Blue
        5 => (0xf5, 0xc2, 0xe7), // Magenta
        6 => (0x94, 0xe2, 0xd5), // Cyan
        7 => (0xa6, 0xad, 0xc8), // White
        // Catppuccin Mocha bright palette 8-15
        8 => (0x58, 0x5b, 0x70),  // BrightBlack
        9 => (0xf3, 0x77, 0x99),  // BrightRed
        10 => (0x89, 0xd8, 0x8b), // BrightGreen
        11 => (0xeb, 0xd3, 0x91), // BrightYellow
        12 => (0x74, 0xa8, 0xfc), // BrightBlue
        13 => (0xf2, 0xae, 0xde), // BrightMagenta
        14 => (0x6b, 0xd7, 0xca), // BrightCyan
        15 => (0xba, 0xc2, 0xde), // BrightWhite
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    pub fn resolve_palette_default(self) -> Self {
        match self {
            Self::PaletteIndex(idx) => lookup_default_256_color(usize::from(idx)),
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
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
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
            _ => return Err(anyhow::anyhow!("Invalid color string")),
        };
        Ok(ret)
    }
}
