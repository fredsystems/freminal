// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal colour types and resolution.
//!
//! - [`TerminalColor`] (this module): the colour a cell asks for.
//! - `palette`: the default 256-colour table and the OSC 4 override table.
//! - `dynamic`: the OSC 10/11/12/17/19 dynamic-colour slots and overrides.
//! - `resolved`: [`ResolvedColors`], everything folded into plain RGB.
//! - `spec`: parsing of xterm colour specifications.
//! - `x11_names` / `wide_gamut`: named-colour and wide-gamut spec support.

mod dynamic;
mod palette;
mod resolved;
mod spec;
mod wide_gamut;
mod x11_names;

pub use dynamic::{DynamicColorOverrides, DynamicColorSlot};
pub use palette::{ColorPalette, PALETTE_SIZE, default_palette_rgb};
pub use resolved::ResolvedColors;
pub use spec::{ColorSpecError, parse_color_spec};

use crate::themes::ThemePalette;
use std::fmt;
use thiserror::Error;

/// Errors produced when parsing a color from a string representation.
#[derive(Debug, Error, Eq, PartialEq, Clone)]
pub enum ColorParseError {
    /// The input string did not match any recognised named color.
    #[error("invalid color name: {0:?}")]
    InvalidName(String),
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
    /// Cells keep the index; the handler does not resolve it.  The renderer
    /// resolves it at render time through the snapshot's `ResolvedColors`,
    /// so OSC 4 palette overrides and theme changes recolour existing text.
    PaletteIndex(u8),
}

impl TerminalColor {
    /// Resolve a `PaletteIndex` to its default color (stateless).
    ///
    /// Returns `self` unchanged for all other variants.
    #[must_use]
    pub fn resolve_palette_default(self, theme: &ThemePalette) -> Self {
        match self {
            Self::PaletteIndex(idx) => {
                let (r, g, b) = default_palette_rgb(idx, theme);
                Self::Custom(r, g, b)
            }
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
