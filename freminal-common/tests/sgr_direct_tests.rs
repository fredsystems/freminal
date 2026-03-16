// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

#![allow(clippy::unwrap_used)]

use freminal_common::colors::TerminalColor;
use freminal_common::sgr::SelectGraphicRendition as Sgr;

// ---------------------------------------------------------------------------
// from_usize — basic attribute variants
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_reset() {
    assert_eq!(Sgr::from_usize(0), Sgr::Reset);
}

#[test]
fn test_from_usize_bold() {
    assert_eq!(Sgr::from_usize(1), Sgr::Bold);
}

#[test]
fn test_from_usize_faint() {
    assert_eq!(Sgr::from_usize(2), Sgr::Faint);
}

#[test]
fn test_from_usize_italic() {
    assert_eq!(Sgr::from_usize(3), Sgr::Italic);
}

#[test]
fn test_from_usize_underline() {
    assert_eq!(Sgr::from_usize(4), Sgr::Underline);
}

#[test]
fn test_from_usize_reverse_video() {
    assert_eq!(Sgr::from_usize(7), Sgr::ReverseVideo);
}

#[test]
fn test_from_usize_strikethrough() {
    assert_eq!(Sgr::from_usize(9), Sgr::Strikethrough);
}

// ---------------------------------------------------------------------------
// from_usize — standard foreground colors (30–37, 39)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_foreground_colors() {
    assert_eq!(Sgr::from_usize(30), Sgr::Foreground(TerminalColor::Black));
    assert_eq!(Sgr::from_usize(31), Sgr::Foreground(TerminalColor::Red));
    assert_eq!(Sgr::from_usize(32), Sgr::Foreground(TerminalColor::Green));
    assert_eq!(Sgr::from_usize(33), Sgr::Foreground(TerminalColor::Yellow));
    assert_eq!(Sgr::from_usize(34), Sgr::Foreground(TerminalColor::Blue));
    assert_eq!(Sgr::from_usize(35), Sgr::Foreground(TerminalColor::Magenta));
    assert_eq!(Sgr::from_usize(36), Sgr::Foreground(TerminalColor::Cyan));
    assert_eq!(Sgr::from_usize(37), Sgr::Foreground(TerminalColor::White));
}

/// 38 is the "custom foreground" introducer; when reached via `from_usize`
/// (i.e. without colour components) the implementation logs an error and
/// falls back to `Foreground(Default)`.
#[test]
fn test_from_usize_38_fallback() {
    assert_eq!(Sgr::from_usize(38), Sgr::Foreground(TerminalColor::Default));
}

#[test]
fn test_from_usize_39_default_fg() {
    assert_eq!(Sgr::from_usize(39), Sgr::Foreground(TerminalColor::Default));
}

// ---------------------------------------------------------------------------
// from_usize — standard background colors (40–47, 49)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_background_colors() {
    assert_eq!(Sgr::from_usize(40), Sgr::Background(TerminalColor::Black));
    assert_eq!(Sgr::from_usize(41), Sgr::Background(TerminalColor::Red));
    assert_eq!(Sgr::from_usize(42), Sgr::Background(TerminalColor::Green));
    assert_eq!(Sgr::from_usize(43), Sgr::Background(TerminalColor::Yellow));
    assert_eq!(Sgr::from_usize(44), Sgr::Background(TerminalColor::Blue));
    assert_eq!(Sgr::from_usize(45), Sgr::Background(TerminalColor::Magenta));
    assert_eq!(Sgr::from_usize(46), Sgr::Background(TerminalColor::Cyan));
    assert_eq!(Sgr::from_usize(47), Sgr::Background(TerminalColor::White));
}

/// 48 is the "custom background" introducer; the error-log fallback path
/// returns `Background(DefaultBackground)`.
#[test]
fn test_from_usize_48_fallback() {
    assert_eq!(
        Sgr::from_usize(48),
        Sgr::Background(TerminalColor::DefaultBackground)
    );
}

#[test]
fn test_from_usize_49_default_bg() {
    assert_eq!(
        Sgr::from_usize(49),
        Sgr::Background(TerminalColor::DefaultBackground)
    );
}

// ---------------------------------------------------------------------------
// from_usize — underline color (58, 59)
// ---------------------------------------------------------------------------

/// 58 is the "custom underline color" introducer; the error-log fallback
/// returns `UnderlineColor(DefaultUnderlineColor)`.
#[test]
fn test_from_usize_58_fallback() {
    assert_eq!(
        Sgr::from_usize(58),
        Sgr::UnderlineColor(TerminalColor::DefaultUnderlineColor)
    );
}

#[test]
fn test_from_usize_59_default_ul() {
    assert_eq!(
        Sgr::from_usize(59),
        Sgr::UnderlineColor(TerminalColor::DefaultUnderlineColor)
    );
}

// ---------------------------------------------------------------------------
// from_usize — bright foreground colors (90–97)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_bright_foreground() {
    assert_eq!(
        Sgr::from_usize(90),
        Sgr::Foreground(TerminalColor::BrightBlack)
    );
    assert_eq!(
        Sgr::from_usize(91),
        Sgr::Foreground(TerminalColor::BrightRed)
    );
    assert_eq!(
        Sgr::from_usize(92),
        Sgr::Foreground(TerminalColor::BrightGreen)
    );
    assert_eq!(
        Sgr::from_usize(93),
        Sgr::Foreground(TerminalColor::BrightYellow)
    );
    assert_eq!(
        Sgr::from_usize(94),
        Sgr::Foreground(TerminalColor::BrightBlue)
    );
    assert_eq!(
        Sgr::from_usize(95),
        Sgr::Foreground(TerminalColor::BrightMagenta)
    );
    assert_eq!(
        Sgr::from_usize(96),
        Sgr::Foreground(TerminalColor::BrightCyan)
    );
    assert_eq!(
        Sgr::from_usize(97),
        Sgr::Foreground(TerminalColor::BrightWhite)
    );
}

// ---------------------------------------------------------------------------
// from_usize — bright background colors (100–107)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_bright_background() {
    assert_eq!(
        Sgr::from_usize(100),
        Sgr::Background(TerminalColor::BrightBlack)
    );
    assert_eq!(
        Sgr::from_usize(101),
        Sgr::Background(TerminalColor::BrightRed)
    );
    assert_eq!(
        Sgr::from_usize(102),
        Sgr::Background(TerminalColor::BrightGreen)
    );
    assert_eq!(
        Sgr::from_usize(103),
        Sgr::Background(TerminalColor::BrightYellow)
    );
    assert_eq!(
        Sgr::from_usize(104),
        Sgr::Background(TerminalColor::BrightBlue)
    );
    assert_eq!(
        Sgr::from_usize(105),
        Sgr::Background(TerminalColor::BrightMagenta)
    );
    assert_eq!(
        Sgr::from_usize(106),
        Sgr::Background(TerminalColor::BrightCyan)
    );
    assert_eq!(
        Sgr::from_usize(107),
        Sgr::Background(TerminalColor::BrightWhite)
    );
}

// ---------------------------------------------------------------------------
// from_usize — Unknown fallback
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_unknown() {
    assert_eq!(Sgr::from_usize(110), Sgr::Unknown(110));
}

#[test]
fn test_from_usize_unknown_200() {
    assert_eq!(Sgr::from_usize(200), Sgr::Unknown(200));
}

// ---------------------------------------------------------------------------
// from_usize — ideogram range (60–65)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_ideogram_range() {
    assert_eq!(Sgr::from_usize(60), Sgr::IdeogramUnderline);
    assert_eq!(Sgr::from_usize(61), Sgr::IdeogramDoubleUnderline);
    assert_eq!(Sgr::from_usize(62), Sgr::IdeogramOverline);
    assert_eq!(Sgr::from_usize(63), Sgr::IdeogramDoubleOverline);
    assert_eq!(Sgr::from_usize(64), Sgr::IdeogramStress);
    assert_eq!(Sgr::from_usize(65), Sgr::IdeogramAttributes);
}

// ---------------------------------------------------------------------------
// from_usize — superscript / subscript (73–75)
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_superscript() {
    assert_eq!(Sgr::from_usize(73), Sgr::Superscript);
    assert_eq!(Sgr::from_usize(74), Sgr::Subscript);
    assert_eq!(Sgr::from_usize(75), Sgr::NeitherSuperscriptNorSubscript);
}

// ---------------------------------------------------------------------------
// from_usize_color — custom RGB variants
// ---------------------------------------------------------------------------

#[test]
fn test_from_usize_color_foreground() {
    assert_eq!(
        Sgr::from_usize_color(38, 128, 64, 0).unwrap(),
        Sgr::Foreground(TerminalColor::Custom(128, 64, 0))
    );
}

#[test]
fn test_from_usize_color_background() {
    assert_eq!(
        Sgr::from_usize_color(48, 0, 0, 255).unwrap(),
        Sgr::Background(TerminalColor::Custom(0, 0, 255))
    );
}

#[test]
fn test_from_usize_color_underline() {
    assert_eq!(
        Sgr::from_usize_color(58, 10, 20, 30).unwrap(),
        Sgr::UnderlineColor(TerminalColor::Custom(10, 20, 30))
    );
}

#[test]
fn test_from_usize_color_unknown_val() {
    assert_eq!(
        Sgr::from_usize_color(99, 0, 0, 0).unwrap(),
        Sgr::Unknown(99)
    );
}

/// Any r/g/b component > 255 cannot be narrowed to `u8`, so `from_usize_color`
/// must return an `Err`.
#[test]
fn test_from_usize_color_overflow() {
    assert!(Sgr::from_usize_color(38, 256, 0, 0).is_err());
}
