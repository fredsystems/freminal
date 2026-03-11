// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::Color32;
use freminal_common::colors::TerminalColor;

// ---------------------------------------------------------------------------
//  Catppuccin Mocha palette — compile-time constants
// ---------------------------------------------------------------------------

// Foreground / Default text
pub const TEXT: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xf4);
// Background
pub const BASE: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x2e);
// Selection background (referenced in theme comments)
pub const SELECTION_BG: Color32 = Color32::from_rgb(0x35, 0x37, 0x48);
// Cursor color
pub const CURSOR: Color32 = Color32::from_rgb(0xf5, 0xe0, 0xdc);
// Cursor text
pub const CURSOR_TEXT: Color32 = Color32::from_rgb(0x11, 0x11, 0x1b);

// Named palette 0-7
pub const BLACK: Color32 = Color32::from_rgb(0x45, 0x47, 0x5a);
pub const RED: Color32 = Color32::from_rgb(0xf3, 0x8b, 0xa8);
pub const GREEN: Color32 = Color32::from_rgb(0xa6, 0xe3, 0xa1);
pub const YELLOW: Color32 = Color32::from_rgb(0xf9, 0xe2, 0xaf);
pub const BLUE: Color32 = Color32::from_rgb(0x89, 0xb4, 0xfa);
pub const MAGENTA: Color32 = Color32::from_rgb(0xf5, 0xc2, 0xe7);
pub const CYAN: Color32 = Color32::from_rgb(0x94, 0xe2, 0xd5);
pub const WHITE: Color32 = Color32::from_rgb(0xa6, 0xad, 0xc8);

// Bright palette 8-15
pub const BRIGHT_BLACK: Color32 = Color32::from_rgb(0x58, 0x5b, 0x70);
pub const BRIGHT_RED: Color32 = Color32::from_rgb(0xf3, 0x77, 0x99);
pub const BRIGHT_GREEN: Color32 = Color32::from_rgb(0x89, 0xd8, 0x8b);
pub const BRIGHT_YELLOW: Color32 = Color32::from_rgb(0xeb, 0xd3, 0x91);
pub const BRIGHT_BLUE: Color32 = Color32::from_rgb(0x74, 0xa8, 0xfc);
pub const BRIGHT_MAGENTA: Color32 = Color32::from_rgb(0xf2, 0xae, 0xde);
pub const BRIGHT_CYAN: Color32 = Color32::from_rgb(0x6b, 0xd7, 0xca);
pub const BRIGHT_WHITE: Color32 = Color32::from_rgb(0xba, 0xc2, 0xde);

// ---------------------------------------------------------------------------
//  [f32; 4] RGBA versions for GL vertex attributes (premultiplied-compatible)
// ---------------------------------------------------------------------------

/// Convert a [`Color32`] to `[f32; 4]` RGBA in `[0.0, 1.0]` range.
///
/// This is a `const fn` helper so that GL-side color constants can also be
/// computed at compile time.
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

pub const TEXT_F: [f32; 4] = color32_to_f32(TEXT);
pub const BASE_F: [f32; 4] = color32_to_f32(BASE);
pub const SELECTION_BG_F: [f32; 4] = color32_to_f32(SELECTION_BG);
pub const CURSOR_F: [f32; 4] = color32_to_f32(CURSOR);
pub const CURSOR_TEXT_F: [f32; 4] = color32_to_f32(CURSOR_TEXT);

pub const BLACK_F: [f32; 4] = color32_to_f32(BLACK);
pub const RED_F: [f32; 4] = color32_to_f32(RED);
pub const GREEN_F: [f32; 4] = color32_to_f32(GREEN);
pub const YELLOW_F: [f32; 4] = color32_to_f32(YELLOW);
pub const BLUE_F: [f32; 4] = color32_to_f32(BLUE);
pub const MAGENTA_F: [f32; 4] = color32_to_f32(MAGENTA);
pub const CYAN_F: [f32; 4] = color32_to_f32(CYAN);
pub const WHITE_F: [f32; 4] = color32_to_f32(WHITE);

pub const BRIGHT_BLACK_F: [f32; 4] = color32_to_f32(BRIGHT_BLACK);
pub const BRIGHT_RED_F: [f32; 4] = color32_to_f32(BRIGHT_RED);
pub const BRIGHT_GREEN_F: [f32; 4] = color32_to_f32(BRIGHT_GREEN);
pub const BRIGHT_YELLOW_F: [f32; 4] = color32_to_f32(BRIGHT_YELLOW);
pub const BRIGHT_BLUE_F: [f32; 4] = color32_to_f32(BRIGHT_BLUE);
pub const BRIGHT_MAGENTA_F: [f32; 4] = color32_to_f32(BRIGHT_MAGENTA);
pub const BRIGHT_CYAN_F: [f32; 4] = color32_to_f32(BRIGHT_CYAN);
pub const BRIGHT_WHITE_F: [f32; 4] = color32_to_f32(BRIGHT_WHITE);

// ---------------------------------------------------------------------------
//  Color conversion functions
// ---------------------------------------------------------------------------

/// Map a `TerminalColor` to an egui `Color32`, applying faint dimming if requested.
///
/// All named colors use compile-time constants — zero heap allocations.
#[must_use]
pub fn internal_color_to_egui(color: TerminalColor, make_faint: bool) -> Color32 {
    let color_before_faint = match color {
        TerminalColor::Default
        | TerminalColor::DefaultUnderlineColor
        | TerminalColor::DefaultCursorColor => TEXT,

        TerminalColor::DefaultBackground => BASE,

        // Base palette 0-7
        TerminalColor::Black => BLACK,
        TerminalColor::Red => RED,
        TerminalColor::Green => GREEN,
        TerminalColor::Yellow => YELLOW,
        TerminalColor::Blue => BLUE,
        TerminalColor::Magenta => MAGENTA,
        TerminalColor::Cyan => CYAN,
        TerminalColor::White => WHITE,

        // Bright palette 8-15
        TerminalColor::BrightBlack => BRIGHT_BLACK,
        TerminalColor::BrightRed => BRIGHT_RED,
        TerminalColor::BrightGreen => BRIGHT_GREEN,
        TerminalColor::BrightYellow => BRIGHT_YELLOW,
        TerminalColor::BrightBlue => BRIGHT_BLUE,
        TerminalColor::BrightMagenta => BRIGHT_MAGENTA,
        TerminalColor::BrightCyan => BRIGHT_CYAN,
        TerminalColor::BrightWhite => BRIGHT_WHITE,

        TerminalColor::Custom(r, g, b) => Color32::from_rgb(r, g, b),

        // PaletteIndex should have been resolved by the handler before reaching
        // the GUI.  If it somehow arrives here, fall back to the default palette.
        TerminalColor::PaletteIndex(_idx) => {
            let resolved = color.resolve_palette_default();
            return internal_color_to_egui(resolved, make_faint);
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
pub fn internal_color_to_gl(color: TerminalColor, make_faint: bool) -> [f32; 4] {
    let base = match color {
        TerminalColor::Default
        | TerminalColor::DefaultUnderlineColor
        | TerminalColor::DefaultCursorColor => TEXT_F,

        TerminalColor::DefaultBackground => BASE_F,

        TerminalColor::Black => BLACK_F,
        TerminalColor::Red => RED_F,
        TerminalColor::Green => GREEN_F,
        TerminalColor::Yellow => YELLOW_F,
        TerminalColor::Blue => BLUE_F,
        TerminalColor::Magenta => MAGENTA_F,
        TerminalColor::Cyan => CYAN_F,
        TerminalColor::White => WHITE_F,

        TerminalColor::BrightBlack => BRIGHT_BLACK_F,
        TerminalColor::BrightRed => BRIGHT_RED_F,
        TerminalColor::BrightGreen => BRIGHT_GREEN_F,
        TerminalColor::BrightYellow => BRIGHT_YELLOW_F,
        TerminalColor::BrightBlue => BRIGHT_BLUE_F,
        TerminalColor::BrightMagenta => BRIGHT_MAGENTA_F,
        TerminalColor::BrightCyan => BRIGHT_CYAN_F,
        TerminalColor::BrightWhite => BRIGHT_WHITE_F,

        TerminalColor::Custom(r, g, b) => [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            1.0,
        ],

        TerminalColor::PaletteIndex(_idx) => {
            let resolved = color.resolve_palette_default();
            return internal_color_to_gl(resolved, make_faint);
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

    /// Verify that each named `Color32` constant matches the original hex values.
    #[test]
    fn named_colors_match_hex_values() {
        // Text / foreground: #cdd6f4
        assert_eq!(TEXT, Color32::from_rgb(0xcd, 0xd6, 0xf4));
        // Background: #1e1e2e
        assert_eq!(BASE, Color32::from_rgb(0x1e, 0x1e, 0x2e));
        // Palette 0-7
        assert_eq!(BLACK, Color32::from_rgb(0x45, 0x47, 0x5a));
        assert_eq!(RED, Color32::from_rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(GREEN, Color32::from_rgb(0xa6, 0xe3, 0xa1));
        assert_eq!(YELLOW, Color32::from_rgb(0xf9, 0xe2, 0xaf));
        assert_eq!(BLUE, Color32::from_rgb(0x89, 0xb4, 0xfa));
        assert_eq!(MAGENTA, Color32::from_rgb(0xf5, 0xc2, 0xe7));
        assert_eq!(CYAN, Color32::from_rgb(0x94, 0xe2, 0xd5));
        assert_eq!(WHITE, Color32::from_rgb(0xa6, 0xad, 0xc8));
        // Bright 8-15
        assert_eq!(BRIGHT_BLACK, Color32::from_rgb(0x58, 0x5b, 0x70));
        assert_eq!(BRIGHT_RED, Color32::from_rgb(0xf3, 0x77, 0x99));
        assert_eq!(BRIGHT_GREEN, Color32::from_rgb(0x89, 0xd8, 0x8b));
        assert_eq!(BRIGHT_YELLOW, Color32::from_rgb(0xeb, 0xd3, 0x91));
        assert_eq!(BRIGHT_BLUE, Color32::from_rgb(0x74, 0xa8, 0xfc));
        assert_eq!(BRIGHT_MAGENTA, Color32::from_rgb(0xf2, 0xae, 0xde));
        assert_eq!(BRIGHT_CYAN, Color32::from_rgb(0x6b, 0xd7, 0xca));
        assert_eq!(BRIGHT_WHITE, Color32::from_rgb(0xba, 0xc2, 0xde));
    }

    /// Verify that `Color32` and `[f32; 4]` representations are consistent.
    #[test]
    fn f32_matches_color32() {
        fn check(name: &str, c32: Color32, gl_color: [f32; 4]) {
            let rgba = c32.to_array();
            let tolerance = 1.0 / 255.0 + f32::EPSILON;
            assert!(
                (gl_color[0] - f32::from(rgba[0]) / 255.0).abs() < tolerance,
                "{name} R mismatch"
            );
            assert!(
                (gl_color[1] - f32::from(rgba[1]) / 255.0).abs() < tolerance,
                "{name} G mismatch"
            );
            assert!(
                (gl_color[2] - f32::from(rgba[2]) / 255.0).abs() < tolerance,
                "{name} B mismatch"
            );
            assert!(
                (gl_color[3] - f32::from(rgba[3]) / 255.0).abs() < tolerance,
                "{name} A mismatch"
            );
        }

        check("TEXT", TEXT, TEXT_F);
        check("BASE", BASE, BASE_F);
        check("BLACK", BLACK, BLACK_F);
        check("RED", RED, RED_F);
        check("GREEN", GREEN, GREEN_F);
        check("YELLOW", YELLOW, YELLOW_F);
        check("BLUE", BLUE, BLUE_F);
        check("MAGENTA", MAGENTA, MAGENTA_F);
        check("CYAN", CYAN, CYAN_F);
        check("WHITE", WHITE, WHITE_F);
        check("BRIGHT_BLACK", BRIGHT_BLACK, BRIGHT_BLACK_F);
        check("BRIGHT_RED", BRIGHT_RED, BRIGHT_RED_F);
        check("BRIGHT_GREEN", BRIGHT_GREEN, BRIGHT_GREEN_F);
        check("BRIGHT_YELLOW", BRIGHT_YELLOW, BRIGHT_YELLOW_F);
        check("BRIGHT_BLUE", BRIGHT_BLUE, BRIGHT_BLUE_F);
        check("BRIGHT_MAGENTA", BRIGHT_MAGENTA, BRIGHT_MAGENTA_F);
        check("BRIGHT_CYAN", BRIGHT_CYAN, BRIGHT_CYAN_F);
        check("BRIGHT_WHITE", BRIGHT_WHITE, BRIGHT_WHITE_F);
    }

    /// Verify that `internal_color_to_egui` returns the const values (no heap allocation).
    #[test]
    fn internal_color_to_egui_uses_consts() {
        assert_eq!(internal_color_to_egui(TerminalColor::Default, false), TEXT);
        assert_eq!(
            internal_color_to_egui(TerminalColor::DefaultBackground, false),
            BASE
        );
        assert_eq!(internal_color_to_egui(TerminalColor::Red, false), RED);
        assert_eq!(
            internal_color_to_egui(TerminalColor::BrightCyan, false),
            BRIGHT_CYAN
        );
    }

    /// Verify that `internal_color_to_gl` produces the const values.
    #[test]
    fn internal_color_to_gl_uses_consts() {
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
            internal_color_to_gl(TerminalColor::Default, false),
            TEXT_F,
            "Default",
        );
        assert_gl_eq(
            internal_color_to_gl(TerminalColor::DefaultBackground, false),
            BASE_F,
            "DefaultBackground",
        );
        assert_gl_eq(
            internal_color_to_gl(TerminalColor::Red, false),
            RED_F,
            "Red",
        );
    }

    /// Faint dimming halves the alpha in the GL path.
    #[test]
    fn faint_dims_alpha_gl() {
        let normal = internal_color_to_gl(TerminalColor::Red, false);
        let faint = internal_color_to_gl(TerminalColor::Red, true);
        assert!(normal[3].mul_add(-0.5, faint[3]).abs() < f32::EPSILON);
        // RGB channels unchanged.
        assert!((faint[0] - normal[0]).abs() < f32::EPSILON);
        assert!((faint[1] - normal[1]).abs() < f32::EPSILON);
        assert!((faint[2] - normal[2]).abs() < f32::EPSILON);
    }

    /// Custom colors pass through correctly.
    #[test]
    fn custom_color_passthrough() {
        let c = internal_color_to_egui(TerminalColor::Custom(0xAB, 0xCD, 0xEF), false);
        assert_eq!(c, Color32::from_rgb(0xAB, 0xCD, 0xEF));

        let gl_val = internal_color_to_gl(TerminalColor::Custom(0xAB, 0xCD, 0xEF), false);
        let tolerance = 1.0 / 255.0 + f32::EPSILON;
        assert!((gl_val[0] - f32::from(0xAB_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[1] - f32::from(0xCD_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[2] - f32::from(0xEF_u8) / 255.0).abs() < tolerance);
        assert!((gl_val[3] - 1.0).abs() < f32::EPSILON);
    }

    /// `PaletteIndex` resolves recursively in both paths.
    #[test]
    fn palette_index_resolves() {
        // Index 1 = Red in the standard palette.
        let c = internal_color_to_egui(TerminalColor::PaletteIndex(1), false);
        // PaletteIndex(1) resolves to TerminalColor::Red → our RED constant.
        assert_eq!(c, RED);

        let gl_val = internal_color_to_gl(TerminalColor::PaletteIndex(1), false);
        assert!(
            gl_val
                .iter()
                .zip(RED_F.iter())
                .all(|(a, e)| (a - e).abs() < f32::EPSILON),
            "PaletteIndex(1) GL should match RED_F"
        );
    }
}
