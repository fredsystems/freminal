// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! [`ResolvedColors`]: the theme, the OSC 4 palette overrides and the
//! OSC 10/11/17/19 dynamic overrides folded into plain RGB values.

use super::{ColorPalette, DynamicColorOverrides, DynamicColorSlot, PALETTE_SIZE, TerminalColor};
use crate::themes::ThemePalette;

/// Every colour a pane's terminal content needs, fully resolved to RGB.
///
/// The cursor colour (OSC 12) is deliberately **not** resolved here: it stays
/// on the cursor-only render path and is carried separately in the snapshot,
/// so a cursor-colour change never invalidates the content colours.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedColors {
    /// The full 256-colour palette (theme ANSI colours, cube, greyscale, and
    /// any OSC 4 overrides applied).
    pub palette: [(u8, u8, u8); PALETTE_SIZE],
    /// Default foreground colour.
    pub foreground: (u8, u8, u8),
    /// Default background colour.
    pub background: (u8, u8, u8),
    /// Selection background colour.
    pub selection_background: (u8, u8, u8),
    /// Selection foreground colour.
    pub selection_foreground: (u8, u8, u8),
}

impl ResolvedColors {
    /// Resolve `theme`, the OSC 4 `palette` overrides and the `dynamic`
    /// overrides into concrete RGB values.
    ///
    /// For each of foreground, background and the two selection colours, the
    /// dynamic override wins when set; otherwise the theme value is used.
    #[must_use]
    pub fn resolve(
        theme: &ThemePalette,
        palette: &ColorPalette,
        dynamic: &DynamicColorOverrides,
    ) -> Self {
        let mut table = [(0, 0, 0); PALETTE_SIZE];
        for (index, entry) in (0..=u8::MAX).zip(table.iter_mut()) {
            *entry = palette.rgb(index, theme);
        }

        Self {
            palette: table,
            foreground: dynamic
                .get(DynamicColorSlot::Foreground)
                .unwrap_or(theme.foreground),
            background: dynamic
                .get(DynamicColorSlot::Background)
                .unwrap_or(theme.background),
            selection_background: dynamic
                .get(DynamicColorSlot::SelectionBackground)
                .unwrap_or(theme.selection_bg),
            selection_foreground: dynamic
                .get(DynamicColorSlot::SelectionForeground)
                .unwrap_or(theme.selection_fg),
        }
    }

    /// The RGB value of `color` under these resolved colours.
    ///
    /// - `Default`, `DefaultUnderlineColor` and `DefaultCursorColor` map to
    ///   the foreground.
    /// - `DefaultBackground` maps to the background.
    /// - The sixteen named colours map to `palette[0..16]`
    ///   (`Black`=0 .. `White`=7, `BrightBlack`=8 .. `BrightWhite`=15).
    /// - `PaletteIndex(n)` maps to `palette[n]`; `Custom` passes through.
    #[must_use]
    pub fn rgb(&self, color: TerminalColor) -> (u8, u8, u8) {
        match color {
            TerminalColor::Default
            | TerminalColor::DefaultUnderlineColor
            | TerminalColor::DefaultCursorColor => self.foreground,

            TerminalColor::DefaultBackground => self.background,

            TerminalColor::Black => self.palette[0],
            TerminalColor::Red => self.palette[1],
            TerminalColor::Green => self.palette[2],
            TerminalColor::Yellow => self.palette[3],
            TerminalColor::Blue => self.palette[4],
            TerminalColor::Magenta => self.palette[5],
            TerminalColor::Cyan => self.palette[6],
            TerminalColor::White => self.palette[7],

            TerminalColor::BrightBlack => self.palette[8],
            TerminalColor::BrightRed => self.palette[9],
            TerminalColor::BrightGreen => self.palette[10],
            TerminalColor::BrightYellow => self.palette[11],
            TerminalColor::BrightBlue => self.palette[12],
            TerminalColor::BrightMagenta => self.palette[13],
            TerminalColor::BrightCyan => self.palette[14],
            TerminalColor::BrightWhite => self.palette[15],

            TerminalColor::Custom(r, g, b) => (r, g, b),

            TerminalColor::PaletteIndex(n) => self.palette[usize::from(n)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::default_palette_rgb;
    use crate::themes::CATPPUCCIN_MOCHA;

    const THEME: &ThemePalette = &CATPPUCCIN_MOCHA;

    fn resolved(palette: &ColorPalette, dynamic: &DynamicColorOverrides) -> ResolvedColors {
        ResolvedColors::resolve(THEME, palette, dynamic)
    }

    fn plain() -> ResolvedColors {
        resolved(&ColorPalette::default(), &DynamicColorOverrides::default())
    }

    #[test]
    fn resolve_without_overrides_matches_theme_and_default_palette() {
        let r = plain();
        assert_eq!(r.foreground, THEME.foreground);
        assert_eq!(r.background, THEME.background);
        assert_eq!(r.selection_background, THEME.selection_bg);
        assert_eq!(r.selection_foreground, THEME.selection_fg);
        for index in 0..=u8::MAX {
            assert_eq!(
                r.palette[usize::from(index)],
                default_palette_rgb(index, THEME),
                "palette[{index}]"
            );
        }
    }

    #[test]
    fn each_dynamic_override_wins_for_its_own_field_only() {
        let base = plain();
        let marker = (1, 2, 3);

        for slot in [
            DynamicColorSlot::Foreground,
            DynamicColorSlot::Background,
            DynamicColorSlot::SelectionBackground,
            DynamicColorSlot::SelectionForeground,
        ] {
            let mut dynamic = DynamicColorOverrides::default();
            dynamic.set(slot, marker);
            let r = resolved(&ColorPalette::default(), &dynamic);

            let expected = ResolvedColors {
                foreground: if slot == DynamicColorSlot::Foreground {
                    marker
                } else {
                    base.foreground
                },
                background: if slot == DynamicColorSlot::Background {
                    marker
                } else {
                    base.background
                },
                selection_background: if slot == DynamicColorSlot::SelectionBackground {
                    marker
                } else {
                    base.selection_background
                },
                selection_foreground: if slot == DynamicColorSlot::SelectionForeground {
                    marker
                } else {
                    base.selection_foreground
                },
                palette: base.palette,
            };
            assert_eq!(r, expected, "override of {slot:?}");
        }
    }

    #[test]
    fn palette_overrides_appear_in_the_table() {
        let mut palette = ColorPalette::default();
        palette.set(1, 10, 20, 30);
        palette.set(200, 40, 50, 60);
        let r = resolved(&palette, &DynamicColorOverrides::default());
        assert_eq!(r.palette[1], (10, 20, 30));
        assert_eq!(r.palette[200], (40, 50, 60));
        assert_eq!(r.palette[2], default_palette_rgb(2, THEME));
    }

    #[test]
    fn cursor_override_changes_nothing() {
        let mut dynamic = DynamicColorOverrides::default();
        dynamic.set(DynamicColorSlot::Cursor, (200, 100, 50));
        assert_eq!(resolved(&ColorPalette::default(), &dynamic), plain());
    }

    #[test]
    fn rgb_default_variants() {
        let r = plain();
        assert_eq!(r.rgb(TerminalColor::Default), THEME.foreground);
        assert_eq!(
            r.rgb(TerminalColor::DefaultUnderlineColor),
            THEME.foreground
        );
        assert_eq!(r.rgb(TerminalColor::DefaultCursorColor), THEME.foreground);
        assert_eq!(r.rgb(TerminalColor::DefaultBackground), THEME.background);
    }

    #[test]
    fn rgb_named_colors_use_the_gui_index_mapping() {
        let r = plain();
        let named = [
            (TerminalColor::Black, 0),
            (TerminalColor::Red, 1),
            (TerminalColor::Green, 2),
            (TerminalColor::Yellow, 3),
            (TerminalColor::Blue, 4),
            (TerminalColor::Magenta, 5),
            (TerminalColor::Cyan, 6),
            (TerminalColor::White, 7),
            (TerminalColor::BrightBlack, 8),
            (TerminalColor::BrightRed, 9),
            (TerminalColor::BrightGreen, 10),
            (TerminalColor::BrightYellow, 11),
            (TerminalColor::BrightBlue, 12),
            (TerminalColor::BrightMagenta, 13),
            (TerminalColor::BrightCyan, 14),
            (TerminalColor::BrightWhite, 15),
        ];
        for (color, index) in named {
            assert_eq!(r.rgb(color), THEME.ansi[index], "{color:?}");
        }
    }

    #[test]
    fn rgb_custom_passes_through() {
        assert_eq!(plain().rgb(TerminalColor::Custom(9, 8, 7)), (9, 8, 7));
    }

    #[test]
    fn rgb_palette_index() {
        let r = plain();
        for index in [0u8, 16, 255] {
            assert_eq!(
                r.rgb(TerminalColor::PaletteIndex(index)),
                default_palette_rgb(index, THEME),
                "index {index}"
            );
        }
    }

    #[test]
    fn named_red_follows_osc4_override() {
        let mut palette = ColorPalette::default();
        palette.set(1, 11, 22, 33);
        let r = resolved(&palette, &DynamicColorOverrides::default());
        assert_eq!(r.rgb(TerminalColor::Red), (11, 22, 33));
    }

    #[test]
    fn default_follows_foreground_override() {
        let mut dynamic = DynamicColorOverrides::default();
        dynamic.set(DynamicColorSlot::Foreground, (5, 6, 7));
        let r = resolved(&ColorPalette::default(), &dynamic);
        assert_eq!(r.rgb(TerminalColor::Default), (5, 6, 7));
        assert_eq!(r.rgb(TerminalColor::DefaultUnderlineColor), (5, 6, 7));
        assert_eq!(r.rgb(TerminalColor::DefaultCursorColor), (5, 6, 7));
    }

    #[test]
    fn size_is_palette_plus_four_colours() {
        // 256 * 3 + 4 * 3: guards against accidental growth.
        assert_eq!(std::mem::size_of::<ResolvedColors>(), 780);
    }
}
