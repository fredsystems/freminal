// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The 256-colour palette: the single default table ([`default_palette_rgb`]),
//! and [`ColorPalette`], the mutable per-index override table.

use crate::themes::ThemePalette;

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

    /// Get the current RGB value for a palette index (override or default).
    #[must_use]
    pub fn rgb(&self, index: u8, theme: &ThemePalette) -> (u8, u8, u8) {
        if let Some((r, g, b)) = self.overrides[usize::from(index)] {
            (r, g, b)
        } else {
            default_palette_rgb(index, theme)
        }
    }
}

/// The six channel levels of the xterm 6x6x6 colour cube.
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// First index of the 6x6x6 colour cube.
const CUBE_START: u8 = 16;

/// First index of the 24-step greyscale ramp.
const GREYSCALE_START: u8 = 232;

/// The one default 256-colour table.
///
/// - 0-15: read from `theme.ansi`.
/// - 16-231: the xterm 6x6x6 cube (levels `0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff`).
///   Index 16 is `(0, 0, 0)` regardless of the theme, matching xterm
///   (`256colres.h`) and kitty.
/// - 232-255: greyscale `8 + 10 * (index - 232)`.
///
/// Every default lookup ([`ColorPalette::rgb`],
/// `TerminalColor::resolve_palette_default`) goes through this function.
#[must_use]
pub fn default_palette_rgb(index: u8, theme: &ThemePalette) -> (u8, u8, u8) {
    match index {
        0..=15 => theme.ansi[usize::from(index)],
        GREYSCALE_START..=255 => {
            // 8 + 10 * 23 = 238, so this cannot overflow `u8`.
            let v = 8 + 10 * (index - GREYSCALE_START);
            (v, v, v)
        }
        _ => {
            let n = index - CUBE_START;
            let step = |component: u8| CUBE_STEPS.get(usize::from(component)).copied().unwrap_or(0);
            (step(n / 36), step((n / 6) % 6), step(n % 6))
        }
    }
}
