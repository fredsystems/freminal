// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use crate::{buffer_states::fonts::UnderlineStyle, colors::TerminalColor};
use thiserror::Error;

/// Errors produced when parsing an SGR (Select Graphic Rendition) sequence.
#[derive(Debug, Error, Eq, PartialEq, Clone)]
pub enum SgrParseError {
    /// A direct-color component (R/G/B) exceeded the u8 range.
    #[error("SGR color component {component} = {value} exceeds u8::MAX (255)")]
    ColorComponentOutOfRange {
        /// Which component was out of range ("r", "g", or "b").
        component: &'static str,
        /// The out-of-range value received.
        value: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum SelectGraphicRendition {
    #[default]
    NoOp,
    // NOTE: Non-exhaustive list
    Reset,
    Bold,
    Italic,
    Underline,
    /// SGR 4:N — underline with a specific style (single, double, curly, dotted, dashed).
    UnderlineWithStyle(UnderlineStyle),
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
    // Inherently large: exhaustive numeric-to-SGR mapping (ECMA-48 Table 43). Each arm is a
    // single numeric code. Splitting would require an intermediate lookup table with no gain.
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
    pub fn from_usize_color(
        val: usize,
        r: usize,
        g: usize,
        b: usize,
    ) -> Result<Self, SgrParseError> {
        let r = u8::try_from(r).map_err(|_| SgrParseError::ColorComponentOutOfRange {
            component: "r",
            value: r,
        })?;
        let g = u8::try_from(g).map_err(|_| SgrParseError::ColorComponentOutOfRange {
            component: "g",
            value: g,
        })?;
        let b = u8::try_from(b).map_err(|_| SgrParseError::ColorComponentOutOfRange {
            component: "b",
            value: b,
        })?;

        match val {
            38 => Ok(Self::Foreground(TerminalColor::Custom(r, g, b))),
            48 => Ok(Self::Background(TerminalColor::Custom(r, g, b))),
            58 => Ok(Self::UnderlineColor(TerminalColor::Custom(r, g, b))),
            _ => Ok(Self::Unknown(val)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // --- from_usize coverage for previously-uncovered SGR codes ---

    #[test]
    fn from_usize_font_codes() {
        assert_eq!(
            SelectGraphicRendition::from_usize(10),
            SelectGraphicRendition::PrimaryFont
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(11),
            SelectGraphicRendition::AlternativeFont1
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(12),
            SelectGraphicRendition::AlternativeFont2
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(13),
            SelectGraphicRendition::AlternativeFont3
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(14),
            SelectGraphicRendition::AlternativeFont4
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(15),
            SelectGraphicRendition::AlternativeFont5
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(16),
            SelectGraphicRendition::AlternativeFont6
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(17),
            SelectGraphicRendition::AlternativeFont7
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(18),
            SelectGraphicRendition::AlternativeFont8
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(19),
            SelectGraphicRendition::AlternativeFont9
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(20),
            SelectGraphicRendition::FontFranktur
        );
    }

    #[test]
    fn from_usize_not_blinking_and_proportional_spacing() {
        assert_eq!(
            SelectGraphicRendition::from_usize(25),
            SelectGraphicRendition::NotBlinking
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(26),
            SelectGraphicRendition::ProportionalSpacing
        );
    }

    #[test]
    fn from_usize_38_error_arm_returns_foreground_default() {
        // 38 without colour components triggers the error-log arm → Foreground(Default)
        assert_eq!(
            SelectGraphicRendition::from_usize(38),
            SelectGraphicRendition::Foreground(TerminalColor::Default)
        );
    }

    #[test]
    fn from_usize_48_error_arm_returns_background_default() {
        // 48 without colour components triggers the error-log arm → Background(DefaultBackground)
        assert_eq!(
            SelectGraphicRendition::from_usize(48),
            SelectGraphicRendition::Background(TerminalColor::DefaultBackground)
        );
    }

    #[test]
    fn from_usize_50_to_55() {
        assert_eq!(
            SelectGraphicRendition::from_usize(50),
            SelectGraphicRendition::DisableProportionalSpacing
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(51),
            SelectGraphicRendition::Framed
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(52),
            SelectGraphicRendition::Encircled
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(53),
            SelectGraphicRendition::Overlined
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(54),
            SelectGraphicRendition::NotFramedOrEncircled
        );
        assert_eq!(
            SelectGraphicRendition::from_usize(55),
            SelectGraphicRendition::NotOverlined
        );
    }

    // --- from_usize_color coverage ---

    #[test]
    fn from_usize_color_58_underline_custom() {
        let result = SelectGraphicRendition::from_usize_color(58, 10, 20, 30).unwrap();
        assert_eq!(
            result,
            SelectGraphicRendition::UnderlineColor(TerminalColor::Custom(10, 20, 30))
        );
    }

    #[test]
    fn from_usize_color_unknown_val() {
        let result = SelectGraphicRendition::from_usize_color(99, 0, 0, 0).unwrap();
        assert_eq!(result, SelectGraphicRendition::Unknown(99));
    }

    #[test]
    fn from_usize_color_38_foreground_custom() {
        let result = SelectGraphicRendition::from_usize_color(38, 255, 128, 0).unwrap();
        assert_eq!(
            result,
            SelectGraphicRendition::Foreground(TerminalColor::Custom(255, 128, 0))
        );
    }

    #[test]
    fn from_usize_color_48_background_custom() {
        let result = SelectGraphicRendition::from_usize_color(48, 0, 64, 128).unwrap();
        assert_eq!(
            result,
            SelectGraphicRendition::Background(TerminalColor::Custom(0, 64, 128))
        );
    }
}
