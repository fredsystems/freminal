// Copyright (C) 2024-2025 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use conv2::ValueInto;
use std::fmt;

#[must_use]
pub fn lookup_256_color_by_index(index: usize) -> TerminalColor {
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
