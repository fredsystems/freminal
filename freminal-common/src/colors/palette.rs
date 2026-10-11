// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The 256-colour palette: the xterm colour cube and greyscale ramp lookups,
//! and [`ColorPalette`], the mutable per-index override table.

use super::TerminalColor;
use crate::themes::ThemePalette;
use conv2::ValueInto;

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
pub(super) fn lookup_default_256_color(index: usize, theme: &ThemePalette) -> TerminalColor {
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
