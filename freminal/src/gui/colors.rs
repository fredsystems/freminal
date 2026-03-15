// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::Color32;
use freminal_common::colors::TerminalColor;
use freminal_common::themes::ThemePalette;

// ---------------------------------------------------------------------------
//  Utility
// ---------------------------------------------------------------------------

/// Convert a [`Color32`] to `[f32; 4]` RGBA in `[0.0, 1.0]` range.
#[must_use]
pub const fn color32_to_f32(color: Color32) -> [f32; 4] {
    let rgba = color.to_array();
    [
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]
}

/// Convert an `(r, g, b)` tuple to a `Color32`.
#[must_use]
const fn rgb_to_color32(rgb: (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(rgb.0, rgb.1, rgb.2)
}

/// Convert an `(r, g, b)` tuple to `[f32; 4]` RGBA with alpha 1.0.
#[must_use]
const fn rgb_to_f32(rgb: (u8, u8, u8)) -> [f32; 4] {
    [
        rgb.0 as f32 / 255.0,
        rgb.1 as f32 / 255.0,
        rgb.2 as f32 / 255.0,
        1.0,
    ]
}

// ---------------------------------------------------------------------------
//  Theme-derived accessors used by renderer.rs
// ---------------------------------------------------------------------------

/// Selection background color as `[f32; 4]` from the active theme.
#[must_use]
pub const fn selection_bg_f(theme: &ThemePalette) -> [f32; 4] {
    rgb_to_f32(theme.selection_bg)
}

/// Selection foreground color as `[f32; 4]` from the active theme.
#[must_use]
pub const fn selection_fg_f(theme: &ThemePalette) -> [f32; 4] {
    rgb_to_f32(theme.selection_fg)
}

/// Cursor color as `[f32; 4]` from the active theme.
#[must_use]
pub const fn cursor_f(theme: &ThemePalette) -> [f32; 4] {
    rgb_to_f32(theme.cursor)
}

// ---------------------------------------------------------------------------
//  Color conversion functions
// ---------------------------------------------------------------------------

/// Map a `TerminalColor` to an egui `Color32`, applying faint dimming if requested.
///
/// Named colors are resolved from the active theme palette.
#[must_use]
pub fn internal_color_to_egui(
    color: TerminalColor,
    make_faint: bool,
    theme: &ThemePalette,
) -> Color32 {
    let color_before_faint = match color {
        TerminalColor::Default
        | TerminalColor::DefaultUnderlineColor
        | TerminalColor::DefaultCursorColor => rgb_to_color32(theme.foreground),

        TerminalColor::DefaultBackground => rgb_to_color32(theme.background),

        // Base palette 0-7
        TerminalColor::Black => rgb_to_color32(theme.ansi[0]),
        TerminalColor::Red => rgb_to_color32(theme.ansi[1]),
        TerminalColor::Green => rgb_to_color32(theme.ansi[2]),
        TerminalColor::Yellow => rgb_to_color32(theme.ansi[3]),
        TerminalColor::Blue => rgb_to_color32(theme.ansi[4]),
        TerminalColor::Magenta => rgb_to_color32(theme.ansi[5]),
        TerminalColor::Cyan => rgb_to_color32(theme.ansi[6]),
        TerminalColor::White => rgb_to_color32(theme.ansi[7]),

        // Bright palette 8-15
        TerminalColor::BrightBlack => rgb_to_color32(theme.ansi[8]),
        TerminalColor::BrightRed => rgb_to_color32(theme.ansi[9]),
        TerminalColor::BrightGreen => rgb_to_color32(theme.ansi[10]),
        TerminalColor::BrightYellow => rgb_to_color32(theme.ansi[11]),
        TerminalColor::BrightBlue => rgb_to_color32(theme.ansi[12]),
        TerminalColor::BrightMagenta => rgb_to_color32(theme.ansi[13]),
        TerminalColor::BrightCyan => rgb_to_color32(theme.ansi[14]),
        TerminalColor::BrightWhite => rgb_to_color32(theme.ansi[15]),

        TerminalColor::Custom(r, g, b) => Color32::from_rgb(r, g, b),

        // PaletteIndex should have been resolved by the handler before reaching
        // the GUI.  If it somehow arrives here, fall back to the default palette.
        TerminalColor::PaletteIndex(_idx) => {
            let resolved = color.resolve_palette_default(theme);
            return internal_color_to_egui(resolved, make_faint, theme);
        }
    };

    if make_faint {
        color_before_faint.gamma_multiply(0.5)
    } else {
        color_before_faint
    }
}

/// Map a `TerminalColor` to an `[f32; 4]` RGBA value for GL vertex attributes.
///
/// Faint dimming is applied by halving the alpha channel.
#[must_use]
pub fn internal_color_to_gl(
    color: TerminalColor,
    make_faint: bool,
    theme: &ThemePalette,
) -> [f32; 4] {
    let base = match color {
        TerminalColor::Default
        | TerminalColor::DefaultUnderlineColor
        | TerminalColor::DefaultCursorColor => rgb_to_f32(theme.foreground),

        TerminalColor::DefaultBackground => rgb_to_f32(theme.background),

        TerminalColor::Black => rgb_to_f32(theme.ansi[0]),
        TerminalColor::Red => rgb_to_f32(theme.ansi[1]),
        TerminalColor::Green => rgb_to_f32(theme.ansi[2]),
        TerminalColor::Yellow => rgb_to_f32(theme.ansi[3]),
        TerminalColor::Blue => rgb_to_f32(theme.ansi[4]),
        TerminalColor::Magenta => rgb_to_f32(theme.ansi[5]),
        TerminalColor::Cyan => rgb_to_f32(theme.ansi[6]),
        TerminalColor::White => rgb_to_f32(theme.ansi[7]),

        TerminalColor::BrightBlack => rgb_to_f32(theme.ansi[8]),
        TerminalColor::BrightRed => rgb_to_f32(theme.ansi[9]),
        TerminalColor::BrightGreen => rgb_to_f32(theme.ansi[10]),
        TerminalColor::BrightYellow => rgb_to_f32(theme.ansi[11]),
        TerminalColor::BrightBlue => rgb_to_f32(theme.ansi[12]),
        TerminalColor::BrightMagenta => rgb_to_f32(theme.ansi[13]),
        TerminalColor::BrightCyan => rgb_to_f32(theme.ansi[14]),
        TerminalColor::BrightWhite => rgb_to_f32(theme.ansi[15]),

        TerminalColor::Custom(r, g, b) => [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ],

        TerminalColor::PaletteIndex(_idx) => {
            let resolved = color.resolve_palette_default(theme);
            return internal_color_to_gl(resolved, make_faint, theme);
        }
    };

    if make_faint {
        [base[0], base[1], base[2], base[3] * 0.5]
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::themes;

    const THEME: &ThemePalette = &themes::CATPPUCCIN_MOCHA;

    /// Helper: `Color32` from a theme (r,g,b) tuple.
    fn c32(rgb: (u8, u8, u8)) -> Color32 {
        Color32::from_rgb(rgb.0, rgb.1, rgb.2)
    }

    /// Helper: `[f32; 4]` from a theme (r,g,b) tuple.
    fn f4(rgb: (u8, u8, u8)) -> [f32; 4] {
        [
            f32::from(rgb.0) / 255.0,
            f32::from(rgb.1) / 255.0,
            f32::from(rgb.2) / 255.0,
            1.0,
        ]
    }

    /// Verify that `internal_color_to_egui` reads theme values correctly.
    #[test]
    fn internal_color_to_egui_reads_theme() {
        assert_eq!(
            internal_color_to_egui(TerminalColor::Default, false, THEME),
            c32(THEME.foreground)
        );
        assert_eq!(
            internal_color_to_egui(TerminalColor::DefaultBackground, false, THEME),
            c32(THEME.background)
        );
        assert_eq!(
            internal_color_to_egui(TerminalColor::Red, false, THEME),
            c32(THEME.ansi[1])
        );
        assert_eq!(
            internal_color_to_egui(TerminalColor::BrightCyan, false, THEME),
            c32(THEME.ansi[14])
        );
    }

    /// Verify that `internal_color_to_gl` reads theme values correctly.
    #[test]
    fn internal_color_to_gl_reads_theme() {
        fn assert_gl_eq(actual: [f32; 4], expected: [f32; 4], label: &str) {
            assert!(
                actual
                    .iter()
                    .zip(expected.iter())
                    .all(|(a, e)| (a - e).abs() < f32::EPSILON),
                "{label}: expected {expected:?}, got {actual:?}"
            );
        }
        assert_gl_eq(
            internal_color_to_gl(TerminalColor::Default, false, THEME),
            f4(THEME.foreground),
            "Default",
        );
        assert_gl_eq(
            internal_color_to_gl(TerminalColor::DefaultBackground, false, THEME),
            f4(THEME.background),
            "DefaultBackground",
        );
        assert_gl_eq(
            internal_color_to_gl(TerminalColor::Red, false, THEME),
            f4(THEME.ansi[1]),
            "Red",
        );
    }

    /// Faint dimming halves the alpha in the GL path.
    #[test]
    fn faint_dims_alpha_gl() {
        let normal = internal_color_to_gl(TerminalColor::Red, false, THEME);
        let faint = internal_color_to_gl(TerminalColor::Red, true, THEME);
        assert!(normal[3].mul_add(-0.5, faint[3]).abs() < f32::EPSILON);
        // RGB channels unchanged.
        assert!((faint[0] - normal[0]).abs() < f32::EPSILON);
        assert!((faint[1] - normal[1]).abs() < f32::EPSILON);
        assert!((faint[2] - normal[2]).abs() < f32::EPSILON);
    }

    /// Custom colors pass through correctly.
    #[test]
    fn custom_color_passthrough() {
        let c = internal_color_to_egui(TerminalColor::Custom(0xAB, 0xCD, 0xEF), false, THEME);
        assert_eq!(c, Color32::from_rgb(0xAB, 0xCD, 0xEF));

        let gl_val = internal_color_to_gl(TerminalColor::Custom(0xAB, 0xCD, 0xEF), false, THEME);
        let tolerance = 1.0 / 255.0 + f32::EPSILON;
        assert!((gl_val[0] - f32::from(0xAB_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[1] - f32::from(0xCD_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[2] - f32::from(0xEF_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[3] - 1.0).abs() < f32::EPSILON);
    }

    /// `PaletteIndex` resolves recursively in both paths.
    #[test]
    fn palette_index_resolves() {
        // Index 1 = Red in the Catppuccin Mocha palette.
        let c = internal_color_to_egui(TerminalColor::PaletteIndex(1), false, THEME);
        assert_eq!(c, c32(THEME.ansi[1]));

        let gl_val = internal_color_to_gl(TerminalColor::PaletteIndex(1), false, THEME);
        let expected = f4(THEME.ansi[1]);
        assert!(
            gl_val
                .iter()
                .zip(expected.iter())
                .all(|(a, e)| (a - e).abs() < f32::EPSILON),
            "PaletteIndex(1) GL should match theme.ansi[1]"
        );
    }

    /// `f32` and `Color32` paths agree for the same theme color.
    #[test]
    fn f32_matches_color32_for_theme() {
        let check = |label: &str, color: TerminalColor| {
            let c32 = internal_color_to_egui(color, false, THEME);
            let gl = internal_color_to_gl(color, false, THEME);
            let rgba = c32.to_array();
            let tolerance = 1.0 / 255.0 + f32::EPSILON;
            assert!(
                (gl[0] - f32::from(rgba[0]) / 255.0).abs() < tolerance,
                "{label} R mismatch"
            );
            assert!(
                (gl[1] - f32::from(rgba[1]) / 255.0).abs() < tolerance,
                "{label} G mismatch"
            );
            assert!(
                (gl[2] - f32::from(rgba[2]) / 255.0).abs() < tolerance,
                "{label} B mismatch"
            );
        };

        check("Default", TerminalColor::Default);
        check("DefaultBackground", TerminalColor::DefaultBackground);
        check("Black", TerminalColor::Black);
        check("Red", TerminalColor::Red);
        check("Green", TerminalColor::Green);
        check("Yellow", TerminalColor::Yellow);
        check("Blue", TerminalColor::Blue);
        check("Magenta", TerminalColor::Magenta);
        check("Cyan", TerminalColor::Cyan);
        check("White", TerminalColor::White);
        check("BrightBlack", TerminalColor::BrightBlack);
        check("BrightRed", TerminalColor::BrightRed);
        check("BrightGreen", TerminalColor::BrightGreen);
        check("BrightYellow", TerminalColor::BrightYellow);
        check("BrightBlue", TerminalColor::BrightBlue);
        check("BrightMagenta", TerminalColor::BrightMagenta);
        check("BrightCyan", TerminalColor::BrightCyan);
        check("BrightWhite", TerminalColor::BrightWhite);
    }

    /// Theme-derived accessors produce correct values.
    #[test]
    fn theme_accessor_fns() {
        let sel_bg = selection_bg_f(THEME);
        let expected = f4(THEME.selection_bg);
        assert!(
            sel_bg
                .iter()
                .zip(expected.iter())
                .all(|(a, e)| (a - e).abs() < f32::EPSILON),
            "selection_bg_f mismatch"
        );

        let sel_fg_color = selection_fg_f(THEME);
        let expected = f4(THEME.selection_fg);
        assert!(
            sel_fg_color
                .iter()
                .zip(expected.iter())
                .all(|(a, e)| (a - e).abs() < f32::EPSILON),
            "selection_fg_f mismatch"
        );

        let cur = cursor_f(THEME);
        let expected = f4(THEME.cursor);
        assert!(
            cur.iter()
                .zip(expected.iter())
                .all(|(a, e)| (a - e).abs() < f32::EPSILON),
            "cursor_f mismatch"
        );
    }
}
