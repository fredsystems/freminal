// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::colors::TerminalColor;
use anyhow::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum SelectGraphicRendition {
    #[default]
    NoOp, // added to allow default construction
    // NOTE: Non-exhaustive list
    Reset,
    Bold,
    Italic,
    Underline,
    Faint,
    ReverseVideo,
    ResetReverseVideo,
    ResetBold,
    NormalIntensity,
    NotUnderlined,
    NotItalic,
    Strikethrough,
    NotStrikethrough,
    Foreground(TerminalColor),
    Background(TerminalColor),
    Unknown(usize),
    UnderlineColor(TerminalColor),
    // We ignore these attributes
    Conceal,
    Revealed,
    PrimaryFont,
    AlternativeFont1,
    AlternativeFont2,
    AlternativeFont3,
    AlternativeFont4,
    AlternativeFont5,
    AlternativeFont6,
    AlternativeFont7,
    AlternativeFont8,
    AlternativeFont9,
    FontFranktur,
    SlowBlink,
    FastBlink,
    NotBlinking,
    ProportionalSpacing,
    DisableProportionalSpacing,
    Framed,
    Encircled,
    Overlined,
    NotFramedOrEncircled,
    NotOverlined,
    IdeogramUnderline,
    IdeogramDoubleUnderline,
    IdeogramOverline,
    IdeogramDoubleOverline,
    IdeogramStress,
    IdeogramAttributes,
    Superscript,
    Subscript,
    NeitherSuperscriptNorSubscript,
}

impl SelectGraphicRendition {
    #[allow(clippy::too_many_lines)]
    pub fn from_usize(val: usize) -> Self {
        match val {
            0 => Self::Reset,
            1 => Self::Bold,
            2 => Self::Faint,
            3 => Self::Italic,
            4 => Self::Underline,
            5 => Self::SlowBlink,
            6 => Self::FastBlink,
            7 => Self::ReverseVideo,
            8 => Self::Conceal,
            9 => Self::Strikethrough,
            10 => Self::PrimaryFont,
            11 => Self::AlternativeFont1,
            12 => Self::AlternativeFont2,
            13 => Self::AlternativeFont3,
            14 => Self::AlternativeFont4,
            15 => Self::AlternativeFont5,
            16 => Self::AlternativeFont6,
            17 => Self::AlternativeFont7,
            18 => Self::AlternativeFont8,
            19 => Self::AlternativeFont9,
            20 => Self::FontFranktur,
            21 => Self::ResetBold,
            22 => Self::NormalIntensity,
            23 => Self::NotItalic,
            24 => Self::NotUnderlined,
            25 => Self::NotBlinking,
            26 => Self::ProportionalSpacing,
            27 => Self::ResetReverseVideo,
            28 => Self::Revealed,
            29 => Self::NotStrikethrough,
            30 => Self::Foreground(TerminalColor::Black),
            31 => Self::Foreground(TerminalColor::Red),
            32 => Self::Foreground(TerminalColor::Green),
            33 => Self::Foreground(TerminalColor::Yellow),
            34 => Self::Foreground(TerminalColor::Blue),
            35 => Self::Foreground(TerminalColor::Magenta),
            36 => Self::Foreground(TerminalColor::Cyan),
            37 => Self::Foreground(TerminalColor::White),
            38 => {
                error!(
                    "This is a custom foreground color. We shouldn't end up here! Setting custom foreground color to default"
                );
                Self::Foreground(TerminalColor::Default)
            }
            39 => Self::Foreground(TerminalColor::Default),
            40 => Self::Background(TerminalColor::Black),
            41 => Self::Background(TerminalColor::Red),
            42 => Self::Background(TerminalColor::Green),
            43 => Self::Background(TerminalColor::Yellow),
            44 => Self::Background(TerminalColor::Blue),
            45 => Self::Background(TerminalColor::Magenta),
            46 => Self::Background(TerminalColor::Cyan),
            47 => Self::Background(TerminalColor::White),
            48 => {
                error!(
                    "This is a custom background color. We shouldn't end up here! Setting custom background color to default"
                );
                Self::Background(TerminalColor::DefaultBackground)
            }
            49 => Self::Background(TerminalColor::DefaultBackground),
            50 => Self::DisableProportionalSpacing,
            51 => Self::Framed,
            52 => Self::Encircled,
            53 => Self::Overlined,
            54 => Self::NotFramedOrEncircled,
            55 => Self::NotOverlined,
            58 => {
                error!(
                    "This is a custom underline color. We shouldn't end up here! Setting custom underline color to default"
                );
                Self::UnderlineColor(TerminalColor::DefaultUnderlineColor)
            }
            59 => Self::UnderlineColor(TerminalColor::DefaultUnderlineColor),
            60 => Self::IdeogramUnderline,
            61 => Self::IdeogramDoubleUnderline,
            62 => Self::IdeogramOverline,
            63 => Self::IdeogramDoubleOverline,
            64 => Self::IdeogramStress,
            65 => Self::IdeogramAttributes,
            73 => Self::Superscript,
            74 => Self::Subscript,
            75 => Self::NeitherSuperscriptNorSubscript,
            90 => Self::Foreground(TerminalColor::BrightBlack),
            91 => Self::Foreground(TerminalColor::BrightRed),
            92 => Self::Foreground(TerminalColor::BrightGreen),
            93 => Self::Foreground(TerminalColor::BrightYellow),
            94 => Self::Foreground(TerminalColor::BrightBlue),
            95 => Self::Foreground(TerminalColor::BrightMagenta),
            96 => Self::Foreground(TerminalColor::BrightCyan),
            97 => Self::Foreground(TerminalColor::BrightWhite),
            100 => Self::Background(TerminalColor::BrightBlack),
            101 => Self::Background(TerminalColor::BrightRed),
            102 => Self::Background(TerminalColor::BrightGreen),
            103 => Self::Background(TerminalColor::BrightYellow),
            104 => Self::Background(TerminalColor::BrightBlue),
            105 => Self::Background(TerminalColor::BrightMagenta),
            106 => Self::Background(TerminalColor::BrightCyan),
            107 => Self::Background(TerminalColor::BrightWhite),
            _ => Self::Unknown(val),
        }
    }

    /// Create a new `SelectGraphicRendition` from a `usize` and three `usize` values representing
    /// the red, green and blue components of a custom color.
    ///
    /// # Errors
    /// Will return an error if any of the `usize` values are greater than `u8::MAX`.
    pub fn from_usize_color(val: usize, r: usize, g: usize, b: usize) -> Result<Self> {
        let r = u8::try_from(r)?;
        let g = u8::try_from(g)?;
        let b = u8::try_from(b)?;

        match val {
            38 => Ok(Self::Foreground(TerminalColor::Custom(r, g, b))),
            48 => Ok(Self::Background(TerminalColor::Custom(r, g, b))),
            58 => Ok(Self::UnderlineColor(TerminalColor::Custom(r, g, b))),
            _ => Ok(Self::Unknown(val)),
        }
    }
}
