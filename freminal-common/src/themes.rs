// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use conv2::ConvUtil;

use crate::buffer_states::command_block::CommandStatus;

/// A complete terminal color palette.
///
/// Contains the 16 ANSI colors (normal + bright), special-purpose colors
/// (foreground, background, cursor, selection), and metadata.
///
/// Each embedded theme is a `const` instance of this struct with `'static`
/// lifetime -- zero-cost references, no heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemePalette {
    /// Human-readable display name for the theme (e.g. "Catppuccin Mocha").
    pub name: &'static str,

    /// Machine-readable slug for TOML config (e.g. "catppuccin-mocha").
    pub slug: &'static str,

    /// Default foreground color (text).
    pub foreground: (u8, u8, u8),

    /// Default background color.
    pub background: (u8, u8, u8),

    /// Cursor fill color.
    pub cursor: (u8, u8, u8),

    /// Text color when drawn under the cursor.
    pub cursor_text: (u8, u8, u8),

    /// Selection highlight background.
    pub selection_bg: (u8, u8, u8),

    /// Selection highlight foreground.
    pub selection_fg: (u8, u8, u8),

    /// The 16 ANSI colors: indices 0-7 (normal) and 8-15 (bright).
    ///
    /// Layout: `[black, red, green, yellow, blue, magenta, cyan, white,
    ///          bright_black, bright_red, ..., bright_white]`
    pub ansi: [(u8, u8, u8); 16],

    /// Command-block gutter color for a successful command (OSC 133 exit
    /// code 0).  `None` falls back to the normal-green ANSI color
    /// (`ansi[2]`).  See [`ThemePalette::gutter_color_for`].
    pub gutter_success: Option<(u8, u8, u8)>,

    /// Command-block gutter color for a failed command (OSC 133 non-zero
    /// exit code).  `None` falls back to the normal-red ANSI color
    /// (`ansi[1]`).  See [`ThemePalette::gutter_color_for`].
    pub gutter_failure: Option<(u8, u8, u8)>,

    /// Command-block gutter color for a running command (OSC 133 A
    /// received, no D yet).  `None` falls back to the normal-yellow ANSI
    /// color (`ansi[3]`).  See [`ThemePalette::gutter_color_for`].
    pub gutter_running: Option<(u8, u8, u8)>,

    // --- Chrome (UI) roles ------------------------------------------------
    //
    // These describe the *non-terminal* UI surfaces (menus, dialogs, toasts,
    // tab bar, buttons, inputs) rather than terminal text.  A `ThemePalette`
    // is primarily a terminal palette, so these are `Option`: vetted upstream
    // themes (e.g. Catppuccin's Surface/Overlay ladder) transcribe authored
    // values here, and raw terminal palettes leave them `None` to be resolved
    // by [`ThemePalette::chrome_role`] (best-fit from existing palette colors,
    // then a contrast-guaranteed fallback).  See the `ChromeRole` enum.
    /// Chrome panel/window background surface. `None` resolves to
    /// `background`.
    pub chrome_surface: Option<(u8, u8, u8)>,

    /// Chrome fill for resting interactive widgets (buttons, inputs,
    /// inactive tabs). `None` is best-fit/derived.
    pub chrome_surface_variant: Option<(u8, u8, u8)>,

    /// Chrome fill for hovered widgets. `None` is best-fit/derived.
    pub chrome_surface_hover: Option<(u8, u8, u8)>,

    /// Chrome fill for pressed / selected / active-tab widgets. `None` is
    /// best-fit/derived (often `selection_bg`).
    pub chrome_surface_active: Option<(u8, u8, u8)>,

    /// Chrome border / separator color. Must read against `chrome_surface`.
    /// `None` resolves to a palette color that contrasts the surface.
    pub chrome_border: Option<(u8, u8, u8)>,

    /// Primary chrome text color. `None` resolves to `foreground`.
    pub chrome_text: Option<(u8, u8, u8)>,

    /// Secondary / muted / disabled chrome text. `None` is best-fit/derived
    /// toward the surface.
    pub chrome_text_muted: Option<(u8, u8, u8)>,
}

/// A named chrome (UI) color role resolved from a [`ThemePalette`].
///
/// Roles are resolved by [`ThemePalette::chrome_role`] using a three-step
/// priority ladder: authored value → best-fit from existing palette colors →
/// contrast-guaranteed derivation. This lets vetted themes look exactly as
/// their authors intended while raw terminal palettes still produce a usable,
/// always-contrasting chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeRole {
    /// Panel/window background surface.
    Surface,
    /// Resting interactive-widget fill (buttons, inputs, inactive tabs).
    SurfaceVariant,
    /// Hovered-widget fill.
    SurfaceHover,
    /// Pressed / selected / active-tab fill.
    SurfaceActive,
    /// Border / separator color.
    Border,
    /// Primary chrome text.
    Text,
    /// Secondary / muted chrome text.
    TextMuted,
}

impl ThemePalette {
    /// Resolve the gutter color for a command-block [`CommandStatus`].
    ///
    /// Returns the theme's configured override when present, otherwise a
    /// sensible default derived from the palette's normal ANSI colors:
    ///
    /// - [`CommandStatus::Success`] -> `gutter_success` or green (`ansi[2]`).
    /// - [`CommandStatus::Failure`] -> `gutter_failure` or red (`ansi[1]`).
    /// - [`CommandStatus::Running`] -> `gutter_running` or yellow (`ansi[3]`).
    /// - [`CommandStatus::Unknown`] -> the normal-white ANSI color
    ///   (`ansi[7]`); there is no dedicated override for the unknown
    ///   state because it is not a status a user typically wants to
    ///   recolor.
    #[must_use]
    pub const fn gutter_color_for(&self, status: CommandStatus) -> (u8, u8, u8) {
        match status {
            CommandStatus::Success => match self.gutter_success {
                Some(c) => c,
                None => self.ansi[2],
            },
            CommandStatus::Failure(_) => match self.gutter_failure {
                Some(c) => c,
                None => self.ansi[1],
            },
            CommandStatus::Running => match self.gutter_running {
                Some(c) => c,
                None => self.ansi[3],
            },
            CommandStatus::Unknown => self.ansi[7],
        }
    }

    /// Resolve a chrome (UI) color [`ChromeRole`] to a concrete RGB triple.
    ///
    /// Priority ladder (per the v0.10.0 chrome-styling design decision):
    ///
    /// 1. **Authored** — the theme's explicit `chrome_*` field, if `Some`.
    ///    Vetted themes transcribe these from upstream (112.3d).
    /// 2. **Best-fit** — reuse a color the palette already defines that suits
    ///    the role (e.g. `selection_bg` for the active fill, `background` for
    ///    the surface). No invented hues.
    /// 3. **Contrast-guaranteed fallback** — derive from `background` /
    ///    `foreground` so the result always contrasts the surface it sits on
    ///    (e.g. a border that never vanishes into the background).
    ///
    /// The returned color is never an invisible/non-contrasting choice: the
    /// fallback guarantees a minimum separation from the relevant surface.
    #[must_use]
    pub fn chrome_role(&self, role: ChromeRole) -> (u8, u8, u8) {
        let dark = is_dark(self.background);
        match role {
            // Surface: authored, else the terminal background.
            ChromeRole::Surface => self.chrome_surface.unwrap_or(self.background),

            // Variant fill (buttons/inputs/inactive tabs): authored, else a
            // surface nudged toward the foreground so it reads as a distinct
            // raised/inset fill against the panel.
            ChromeRole::SurfaceVariant => self
                .chrome_surface_variant
                .unwrap_or_else(|| blend(self.background, self.foreground, 0.10)),

            // Hover fill: authored, else a slightly stronger lift than variant.
            ChromeRole::SurfaceHover => self
                .chrome_surface_hover
                .unwrap_or_else(|| blend(self.background, self.foreground, 0.18)),

            // Active/selected fill: authored, else the palette's own selection
            // background (a color the theme already vetted for highlights).
            ChromeRole::SurfaceActive => self.chrome_surface_active.unwrap_or(self.selection_bg),

            // Border: authored, else a color blended between surface and
            // foreground far enough to stay visible against the surface on
            // any theme (the fix for "invisible" ANSI-black borders).
            ChromeRole::Border => self
                .chrome_border
                .unwrap_or_else(|| blend(self.background, self.foreground, 0.30)),

            // Primary text: authored, else the terminal foreground.
            ChromeRole::Text => self.chrome_text.unwrap_or(self.foreground),

            // Muted text: authored, else foreground pulled toward the surface
            // (dimmed) but kept readable.
            ChromeRole::TextMuted => self.chrome_text_muted.unwrap_or_else(|| {
                let muted = blend(self.foreground, self.background, 0.40);
                // Guarantee the muted text still separates from the surface.
                ensure_contrast(muted, self.background, dark)
            }),
        }
    }

    /// Pick whichever of [`ChromeRole::Text`] / a given surface color reads
    /// with higher contrast on `surface` — used to choose legible text on an
    /// arbitrary chrome fill (e.g. the active-tab fill).
    ///
    /// Returns the palette's chrome text if it contrasts the surface well
    /// enough; otherwise returns the opposite of the surface
    /// (`background`/`foreground` whichever contrasts more).
    #[must_use]
    pub fn chrome_text_on(&self, surface: (u8, u8, u8)) -> (u8, u8, u8) {
        let text = self.chrome_role(ChromeRole::Text);
        let candidate_a = text;
        let candidate_b = if is_dark(surface) {
            self.foreground
        } else {
            self.background
        };
        if contrast_ratio(candidate_a, surface) >= contrast_ratio(candidate_b, surface) {
            candidate_a
        } else {
            candidate_b
        }
    }
}

// ---------------------------------------------------------------------------
//  Chrome-role color math (pure RGB; no egui)
// ---------------------------------------------------------------------------

/// Perceptual (gamma-space, Rec. 601) luminance of an RGB color in
/// `[0.0, 1.0]`. Used only for the dark/light decision and quick comparisons.
fn luma(rgb: (u8, u8, u8)) -> f32 {
    let r = f32::from(rgb.0) / 255.0;
    let g = f32::from(rgb.1) / 255.0;
    let b = f32::from(rgb.2) / 255.0;
    0.299_f32.mul_add(r, 0.587_f32.mul_add(g, 0.114 * b))
}

/// Whether a color reads as "dark" (luminance below the midpoint).
fn is_dark(rgb: (u8, u8, u8)) -> bool {
    luma(rgb) < 0.5
}

/// Linear interpolation between two colors in gamma space
/// (`t = 0.0` → `a`, `t = 1.0` → `b`).
fn blend(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 {
        let xf = f32::from(x);
        let yf = f32::from(y);
        let v = (yf - xf).mul_add(t, xf).round().clamp(0.0, 255.0);
        v.approx_as::<u8>().unwrap_or(x)
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Convert an sRGB channel byte to linear light (IEC 61966-2-1).
fn srgb_to_linear(c: u8) -> f32 {
    let cs = f32::from(c) / 255.0;
    if cs <= 0.040_45 {
        cs / 12.92
    } else {
        ((cs + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of an sRGB color (linear-light weighted).
fn relative_luminance(rgb: (u8, u8, u8)) -> f32 {
    let r = srgb_to_linear(rgb.0);
    let g = srgb_to_linear(rgb.1);
    let b = srgb_to_linear(rgb.2);
    0.2126_f32.mul_add(r, 0.7152_f32.mul_add(g, 0.0722 * b))
}

/// WCAG contrast ratio between two colors, in `[1.0, 21.0]`.
fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let la = relative_luminance(a);
    let lb = relative_luminance(b);
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Minimum contrast ratio a derived chrome color must keep against its
/// surface. ~2.2:1 is below text-legibility thresholds but enough to keep
/// borders and muted text visibly separated from the surface.
const MIN_CHROME_CONTRAST: f32 = 2.2;

/// Ensure `color` contrasts `surface` by at least [`MIN_CHROME_CONTRAST`],
/// nudging it away from the surface (toward black on light surfaces, toward
/// white on dark surfaces) until it does.
fn ensure_contrast(
    color: (u8, u8, u8),
    surface: (u8, u8, u8),
    surface_is_dark: bool,
) -> (u8, u8, u8) {
    if contrast_ratio(color, surface) >= MIN_CHROME_CONTRAST {
        return color;
    }
    let target = if surface_is_dark {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    // Step toward the high-contrast target in fixed increments until the
    // threshold is met (integer steps to avoid float-loop pitfalls).
    for step in 1_u8..=6 {
        let t = f32::from(step) * 0.15;
        let candidate = blend(color, target, t);
        if contrast_ratio(candidate, surface) >= MIN_CHROME_CONTRAST {
            return candidate;
        }
    }
    target
}

// ---------------------------------------------------------------------------
//  Catppuccin Mocha -- the default theme
// ---------------------------------------------------------------------------

/// Catppuccin Mocha palette (dark).
///
/// Source: <https://github.com/catppuccin/catppuccin>
/// License: MIT
pub const CATPPUCCIN_MOCHA: ThemePalette = ThemePalette {
    name: "Catppuccin Mocha",
    slug: "catppuccin-mocha",
    foreground: (0xcd, 0xd6, 0xf4),  // Text
    background: (0x1e, 0x1e, 0x2e),  // Base
    cursor: (0xf5, 0xe0, 0xdc),      // Rosewater
    cursor_text: (0x11, 0x11, 0x1b), // Crust
    selection_bg: (0xa0, 0xa4, 0xb8),
    selection_fg: (0x11, 0x11, 0x1b),
    ansi: [
        (0x45, 0x47, 0x5a), // 0  Black    (Surface1)
        (0xf3, 0x8b, 0xa8), // 1  Red
        (0xa6, 0xe3, 0xa1), // 2  Green
        (0xf9, 0xe2, 0xaf), // 3  Yellow
        (0x89, 0xb4, 0xfa), // 4  Blue
        (0xf5, 0xc2, 0xe7), // 5  Magenta  (Pink)
        (0x94, 0xe2, 0xd5), // 6  Cyan     (Teal)
        (0xa6, 0xad, 0xc8), // 7  White    (Subtext0)
        (0x58, 0x5b, 0x70), // 8  BrightBlack  (Surface2)
        (0xf3, 0x77, 0x99), // 9  BrightRed
        (0x89, 0xd8, 0x8b), // 10 BrightGreen
        (0xeb, 0xd3, 0x91), // 11 BrightYellow
        (0x74, 0xa8, 0xfc), // 12 BrightBlue
        (0xf2, 0xae, 0xde), // 13 BrightMagenta
        (0x6b, 0xd7, 0xca), // 14 BrightCyan
        (0xba, 0xc2, 0xde), // 15 BrightWhite (Subtext1)
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Catppuccin Macchiato
// ---------------------------------------------------------------------------

/// Catppuccin Macchiato palette (dark).
///
/// Source: <https://github.com/catppuccin/catppuccin>
/// License: MIT
pub const CATPPUCCIN_MACCHIATO: ThemePalette = ThemePalette {
    name: "Catppuccin Macchiato",
    slug: "catppuccin-macchiato",
    foreground: (0xca, 0xd3, 0xf5),
    background: (0x24, 0x27, 0x3a),
    cursor: (0xf4, 0xdb, 0xd6),
    cursor_text: (0x18, 0x19, 0x26),
    selection_bg: (0xa5, 0xad, 0xce),
    selection_fg: (0x18, 0x19, 0x26),
    ansi: [
        (0x49, 0x4d, 0x64), // 0  Black
        (0xed, 0x87, 0x96), // 1  Red
        (0xa6, 0xda, 0x95), // 2  Green
        (0xee, 0xd4, 0x9f), // 3  Yellow
        (0x8a, 0xad, 0xf4), // 4  Blue
        (0xf5, 0xbd, 0xe6), // 5  Magenta
        (0x8b, 0xd5, 0xca), // 6  Cyan
        (0xa5, 0xad, 0xcb), // 7  White
        (0x5b, 0x60, 0x78), // 8  BrightBlack
        (0xed, 0x70, 0x83), // 9  BrightRed
        (0x87, 0xd2, 0x8e), // 10 BrightGreen
        (0xe5, 0xc6, 0x80), // 11 BrightYellow
        (0x73, 0x9d, 0xf2), // 12 BrightBlue
        (0xf0, 0xa4, 0xdb), // 13 BrightMagenta
        (0x63, 0xcb, 0xbe), // 14 BrightCyan
        (0xb8, 0xc0, 0xe0), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Catppuccin Frappe
// ---------------------------------------------------------------------------

/// Catppuccin Frappe palette (dark).
///
/// Source: <https://github.com/catppuccin/catppuccin>
/// License: MIT
pub const CATPPUCCIN_FRAPPE: ThemePalette = ThemePalette {
    name: "Catppuccin Frappe",
    slug: "catppuccin-frappe",
    foreground: (0xc6, 0xd0, 0xf5),
    background: (0x30, 0x34, 0x46),
    cursor: (0xf2, 0xd5, 0xcf),
    cursor_text: (0x23, 0x26, 0x34),
    selection_bg: (0xa5, 0xad, 0xce),
    selection_fg: (0x23, 0x26, 0x34),
    ansi: [
        (0x51, 0x57, 0x6d), // 0  Black
        (0xe7, 0x82, 0x84), // 1  Red
        (0xa6, 0xd1, 0x89), // 2  Green
        (0xe5, 0xc8, 0x90), // 3  Yellow
        (0x8c, 0xaa, 0xee), // 4  Blue
        (0xf4, 0xb8, 0xe4), // 5  Magenta
        (0x81, 0xc8, 0xbe), // 6  Cyan
        (0xa5, 0xad, 0xce), // 7  White
        (0x62, 0x68, 0x80), // 8  BrightBlack
        (0xe6, 0x71, 0x72), // 9  BrightRed
        (0x8e, 0xc7, 0x72), // 10 BrightGreen
        (0xd9, 0xba, 0x73), // 11 BrightYellow
        (0x7b, 0x9e, 0xf0), // 12 BrightBlue
        (0xf1, 0xa4, 0xda), // 13 BrightMagenta
        (0x5f, 0xbf, 0xb4), // 14 BrightCyan
        (0xb5, 0xbf, 0xe2), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Catppuccin Latte
// ---------------------------------------------------------------------------

/// Catppuccin Latte palette (light).
///
/// Source: <https://github.com/catppuccin/catppuccin>
/// License: MIT
pub const CATPPUCCIN_LATTE: ThemePalette = ThemePalette {
    name: "Catppuccin Latte",
    slug: "catppuccin-latte",
    foreground: (0x4c, 0x4f, 0x69),
    background: (0xef, 0xf1, 0xf5),
    cursor: (0xdc, 0x8a, 0x78),
    cursor_text: (0xef, 0xf1, 0xf5),
    selection_bg: (0x7c, 0x7f, 0x93),
    selection_fg: (0xef, 0xf1, 0xf5),
    ansi: [
        (0x5c, 0x5f, 0x77), // 0  Black
        (0xd2, 0x0f, 0x39), // 1  Red
        (0x40, 0xa0, 0x2b), // 2  Green
        (0xdf, 0x8e, 0x1d), // 3  Yellow
        (0x1e, 0x66, 0xf5), // 4  Blue
        (0xea, 0x76, 0xcb), // 5  Magenta
        (0x17, 0x92, 0x99), // 6  Cyan
        (0xac, 0xb0, 0xbe), // 7  White
        (0x6c, 0x6f, 0x85), // 8  BrightBlack
        (0xd2, 0x19, 0x2b), // 9  BrightRed
        (0x3d, 0x9a, 0x28), // 10 BrightGreen
        (0xd2, 0x82, 0x19), // 11 BrightYellow
        (0x1b, 0x5e, 0xf0), // 12 BrightBlue
        (0xe8, 0x66, 0xc1), // 13 BrightMagenta
        (0x14, 0x8f, 0x93), // 14 BrightCyan
        (0xbc, 0xc0, 0xcc), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Dracula
// ---------------------------------------------------------------------------

/// Dracula palette (dark).
///
/// Source: <https://github.com/dracula/dracula-theme>
/// License: MIT
pub const DRACULA: ThemePalette = ThemePalette {
    name: "Dracula",
    slug: "dracula",
    foreground: (0xf8, 0xf8, 0xf2),
    background: (0x28, 0x2a, 0x36),
    cursor: (0xf8, 0xf8, 0xf2),
    cursor_text: (0x28, 0x2a, 0x36),
    selection_bg: (0x44, 0x47, 0x5a),
    selection_fg: (0xf8, 0xf8, 0xf2),
    ansi: [
        (0x21, 0x22, 0x2c), // 0  Black
        (0xff, 0x55, 0x55), // 1  Red
        (0x50, 0xfa, 0x7b), // 2  Green
        (0xf1, 0xfa, 0x8c), // 3  Yellow
        (0xbd, 0x93, 0xf9), // 4  Blue
        (0xff, 0x79, 0xc6), // 5  Magenta
        (0x8b, 0xe9, 0xfd), // 6  Cyan
        (0xf8, 0xf8, 0xf2), // 7  White
        (0x62, 0x72, 0xa4), // 8  BrightBlack
        (0xff, 0x6e, 0x6e), // 9  BrightRed
        (0x69, 0xff, 0x94), // 10 BrightGreen
        (0xff, 0xff, 0xa5), // 11 BrightYellow
        (0xd6, 0xac, 0xff), // 12 BrightBlue
        (0xff, 0x92, 0xdf), // 13 BrightMagenta
        (0xa4, 0xff, 0xff), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Nord
// ---------------------------------------------------------------------------

/// Nord palette (dark).
///
/// Source: <https://github.com/nordtheme/nord>
/// License: MIT
pub const NORD: ThemePalette = ThemePalette {
    name: "Nord",
    slug: "nord",
    foreground: (0xd8, 0xde, 0xe9),
    background: (0x2e, 0x34, 0x40),
    cursor: (0xd8, 0xde, 0xe9),
    cursor_text: (0x2e, 0x34, 0x40),
    selection_bg: (0x4c, 0x56, 0x6a),
    selection_fg: (0xd8, 0xde, 0xe9),
    ansi: [
        (0x3b, 0x42, 0x52), // 0  Black
        (0xbf, 0x61, 0x6a), // 1  Red
        (0xa3, 0xbe, 0x8c), // 2  Green
        (0xeb, 0xcb, 0x8b), // 3  Yellow
        (0x81, 0xa1, 0xc1), // 4  Blue
        (0xb4, 0x8e, 0xad), // 5  Magenta
        (0x88, 0xc0, 0xd0), // 6  Cyan
        (0xe5, 0xe9, 0xf0), // 7  White
        (0x4c, 0x56, 0x6a), // 8  BrightBlack
        (0xbf, 0x61, 0x6a), // 9  BrightRed
        (0xa3, 0xbe, 0x8c), // 10 BrightGreen
        (0xeb, 0xcb, 0x8b), // 11 BrightYellow
        (0x81, 0xa1, 0xc1), // 12 BrightBlue
        (0xb4, 0x8e, 0xad), // 13 BrightMagenta
        (0x8f, 0xbc, 0xbb), // 14 BrightCyan
        (0xec, 0xef, 0xf4), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Solarized Dark
// ---------------------------------------------------------------------------

/// Solarized Dark palette (dark).
///
/// Source: <https://github.com/altercation/solarized>
/// License: MIT
pub const SOLARIZED_DARK: ThemePalette = ThemePalette {
    name: "Solarized Dark",
    slug: "solarized-dark",
    foreground: (0x83, 0x94, 0x96),
    background: (0x00, 0x2b, 0x36),
    cursor: (0x83, 0x94, 0x96),
    cursor_text: (0x00, 0x2b, 0x36),
    selection_bg: (0x07, 0x36, 0x42),
    selection_fg: (0x93, 0xa1, 0xa1),
    ansi: [
        (0x07, 0x36, 0x42), // 0  Black
        (0xdc, 0x32, 0x2f), // 1  Red
        (0x85, 0x99, 0x00), // 2  Green
        (0xb5, 0x89, 0x00), // 3  Yellow
        (0x26, 0x8b, 0xd2), // 4  Blue
        (0xd3, 0x36, 0x82), // 5  Magenta
        (0x2a, 0xa1, 0x98), // 6  Cyan
        (0xee, 0xe8, 0xd5), // 7  White
        (0x00, 0x2b, 0x36), // 8  BrightBlack
        (0xcb, 0x4b, 0x16), // 9  BrightRed
        (0x58, 0x6e, 0x75), // 10 BrightGreen
        (0x65, 0x7b, 0x83), // 11 BrightYellow
        (0x83, 0x94, 0x96), // 12 BrightBlue
        (0x6c, 0x71, 0xc4), // 13 BrightMagenta
        (0x93, 0xa1, 0xa1), // 14 BrightCyan
        (0xfd, 0xf6, 0xe3), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Solarized Light
// ---------------------------------------------------------------------------

/// Solarized Light palette (light).
///
/// Source: <https://github.com/altercation/solarized>
/// License: MIT
pub const SOLARIZED_LIGHT: ThemePalette = ThemePalette {
    name: "Solarized Light",
    slug: "solarized-light",
    foreground: (0x65, 0x7b, 0x83),
    background: (0xfd, 0xf6, 0xe3),
    cursor: (0x65, 0x7b, 0x83),
    cursor_text: (0xfd, 0xf6, 0xe3),
    selection_bg: (0xee, 0xe8, 0xd5),
    selection_fg: (0x58, 0x6e, 0x75),
    ansi: [
        (0x07, 0x36, 0x42), // 0  Black
        (0xdc, 0x32, 0x2f), // 1  Red
        (0x85, 0x99, 0x00), // 2  Green
        (0xb5, 0x89, 0x00), // 3  Yellow
        (0x26, 0x8b, 0xd2), // 4  Blue
        (0xd3, 0x36, 0x82), // 5  Magenta
        (0x2a, 0xa1, 0x98), // 6  Cyan
        (0xee, 0xe8, 0xd5), // 7  White
        (0x00, 0x2b, 0x36), // 8  BrightBlack
        (0xcb, 0x4b, 0x16), // 9  BrightRed
        (0x58, 0x6e, 0x75), // 10 BrightGreen
        (0x65, 0x7b, 0x83), // 11 BrightYellow
        (0x83, 0x94, 0x96), // 12 BrightBlue
        (0x6c, 0x71, 0xc4), // 13 BrightMagenta
        (0x93, 0xa1, 0xa1), // 14 BrightCyan
        (0xfd, 0xf6, 0xe3), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Gruvbox Dark
// ---------------------------------------------------------------------------

/// Gruvbox Dark palette (dark).
///
/// Source: <https://github.com/morhetz/gruvbox>
/// License: MIT
pub const GRUVBOX_DARK: ThemePalette = ThemePalette {
    name: "Gruvbox Dark",
    slug: "gruvbox-dark",
    foreground: (0xeb, 0xdb, 0xb2),
    background: (0x28, 0x28, 0x28),
    cursor: (0xeb, 0xdb, 0xb2),
    cursor_text: (0x28, 0x28, 0x28),
    selection_bg: (0x50, 0x49, 0x45),
    selection_fg: (0xeb, 0xdb, 0xb2),
    ansi: [
        (0x28, 0x28, 0x28), // 0  Black
        (0xcc, 0x24, 0x1d), // 1  Red
        (0x98, 0x97, 0x1a), // 2  Green
        (0xd7, 0x99, 0x21), // 3  Yellow
        (0x45, 0x85, 0x88), // 4  Blue
        (0xb1, 0x62, 0x86), // 5  Magenta
        (0x68, 0x9d, 0x6a), // 6  Cyan
        (0xa8, 0x99, 0x84), // 7  White
        (0x92, 0x83, 0x74), // 8  BrightBlack
        (0xfb, 0x49, 0x34), // 9  BrightRed
        (0xb8, 0xbb, 0x26), // 10 BrightGreen
        (0xfa, 0xbd, 0x2f), // 11 BrightYellow
        (0x83, 0xa5, 0x98), // 12 BrightBlue
        (0xd3, 0x86, 0x9b), // 13 BrightMagenta
        (0x8e, 0xc0, 0x7c), // 14 BrightCyan
        (0xeb, 0xdb, 0xb2), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Gruvbox Light
// ---------------------------------------------------------------------------

/// Gruvbox Light palette (light).
///
/// Source: <https://github.com/morhetz/gruvbox>
/// License: MIT
pub const GRUVBOX_LIGHT: ThemePalette = ThemePalette {
    name: "Gruvbox Light",
    slug: "gruvbox-light",
    foreground: (0x3c, 0x38, 0x36),
    background: (0xfb, 0xf1, 0xc7),
    cursor: (0x3c, 0x38, 0x36),
    cursor_text: (0xfb, 0xf1, 0xc7),
    selection_bg: (0xd5, 0xc4, 0xa1),
    selection_fg: (0x3c, 0x38, 0x36),
    ansi: [
        (0xfb, 0xf1, 0xc7), // 0  Black
        (0xcc, 0x24, 0x1d), // 1  Red
        (0x98, 0x97, 0x1a), // 2  Green
        (0xd7, 0x99, 0x21), // 3  Yellow
        (0x45, 0x85, 0x88), // 4  Blue
        (0xb1, 0x62, 0x86), // 5  Magenta
        (0x68, 0x9d, 0x6a), // 6  Cyan
        (0x7c, 0x6f, 0x64), // 7  White
        (0x92, 0x83, 0x74), // 8  BrightBlack
        (0x9d, 0x00, 0x06), // 9  BrightRed
        (0x79, 0x74, 0x0e), // 10 BrightGreen
        (0xb5, 0x76, 0x14), // 11 BrightYellow
        (0x07, 0x66, 0x78), // 12 BrightBlue
        (0x8f, 0x3f, 0x71), // 13 BrightMagenta
        (0x42, 0x7b, 0x58), // 14 BrightCyan
        (0x3c, 0x38, 0x36), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  One Dark
// ---------------------------------------------------------------------------

/// One Dark palette (dark).
///
/// Source: <https://github.com/atom/atom/tree/master/packages/one-dark-ui>
/// License: MIT
pub const ONE_DARK: ThemePalette = ThemePalette {
    name: "One Dark",
    slug: "one-dark",
    foreground: (0xab, 0xb2, 0xbf),
    background: (0x28, 0x2c, 0x34),
    cursor: (0x52, 0x8b, 0xff),
    cursor_text: (0x28, 0x2c, 0x34),
    selection_bg: (0x3e, 0x44, 0x51),
    selection_fg: (0xab, 0xb2, 0xbf),
    ansi: [
        (0x28, 0x2c, 0x34), // 0  Black
        (0xe0, 0x6c, 0x75), // 1  Red
        (0x98, 0xc3, 0x79), // 2  Green
        (0xe5, 0xc0, 0x7b), // 3  Yellow
        (0x61, 0xaf, 0xef), // 4  Blue
        (0xc6, 0x78, 0xdd), // 5  Magenta
        (0x56, 0xb6, 0xc2), // 6  Cyan
        (0xab, 0xb2, 0xbf), // 7  White
        (0x54, 0x58, 0x62), // 8  BrightBlack
        (0xe0, 0x6c, 0x75), // 9  BrightRed
        (0x98, 0xc3, 0x79), // 10 BrightGreen
        (0xe5, 0xc0, 0x7b), // 11 BrightYellow
        (0x61, 0xaf, 0xef), // 12 BrightBlue
        (0xc6, 0x78, 0xdd), // 13 BrightMagenta
        (0x56, 0xb6, 0xc2), // 14 BrightCyan
        (0xc8, 0xcc, 0xd4), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  One Light
// ---------------------------------------------------------------------------

/// One Light palette (light).
///
/// Source: <https://github.com/atom/atom/tree/master/packages/one-light-ui>
/// License: MIT
pub const ONE_LIGHT: ThemePalette = ThemePalette {
    name: "One Light",
    slug: "one-light",
    foreground: (0x38, 0x3a, 0x42),
    background: (0xfa, 0xfa, 0xfa),
    cursor: (0x52, 0x6f, 0xff),
    cursor_text: (0xfa, 0xfa, 0xfa),
    selection_bg: (0xe5, 0xe5, 0xe6),
    selection_fg: (0x38, 0x3a, 0x42),
    ansi: [
        (0x38, 0x3a, 0x42), // 0  Black
        (0xe4, 0x56, 0x49), // 1  Red
        (0x50, 0xa1, 0x4f), // 2  Green
        (0xc1, 0x84, 0x01), // 3  Yellow
        (0x40, 0x78, 0xf2), // 4  Blue
        (0xa6, 0x26, 0xa4), // 5  Magenta
        (0x01, 0x84, 0xbc), // 6  Cyan
        (0xa0, 0xa1, 0xa7), // 7  White
        (0x4f, 0x52, 0x5e), // 8  BrightBlack
        (0xe4, 0x56, 0x49), // 9  BrightRed
        (0x50, 0xa1, 0x4f), // 10 BrightGreen
        (0xc1, 0x84, 0x01), // 11 BrightYellow
        (0x40, 0x78, 0xf2), // 12 BrightBlue
        (0xa6, 0x26, 0xa4), // 13 BrightMagenta
        (0x01, 0x84, 0xbc), // 14 BrightCyan
        (0xfa, 0xfa, 0xfa), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Tokyo Night
// ---------------------------------------------------------------------------

/// Tokyo Night palette (dark).
///
/// Source: <https://github.com/enkia/tokyo-night-vscode-theme>
/// License: MIT
pub const TOKYO_NIGHT: ThemePalette = ThemePalette {
    name: "Tokyo Night",
    slug: "tokyo-night",
    foreground: (0xa9, 0xb1, 0xd6),
    background: (0x1a, 0x1b, 0x26),
    cursor: (0xc0, 0xca, 0xf5),
    cursor_text: (0x1a, 0x1b, 0x26),
    selection_bg: (0x33, 0x46, 0x7c),
    selection_fg: (0xc0, 0xca, 0xf5),
    ansi: [
        (0x15, 0x16, 0x1e), // 0  Black
        (0xf7, 0x76, 0x8e), // 1  Red
        (0x9e, 0xce, 0x6a), // 2  Green
        (0xe0, 0xaf, 0x68), // 3  Yellow
        (0x7a, 0xa2, 0xf7), // 4  Blue
        (0xbb, 0x9a, 0xf7), // 5  Magenta
        (0x7d, 0xcf, 0xff), // 6  Cyan
        (0xa9, 0xb1, 0xd6), // 7  White
        (0x41, 0x48, 0x68), // 8  BrightBlack
        (0xf7, 0x76, 0x8e), // 9  BrightRed
        (0x9e, 0xce, 0x6a), // 10 BrightGreen
        (0xe0, 0xaf, 0x68), // 11 BrightYellow
        (0x7a, 0xa2, 0xf7), // 12 BrightBlue
        (0xbb, 0x9a, 0xf7), // 13 BrightMagenta
        (0x7d, 0xcf, 0xff), // 14 BrightCyan
        (0xc0, 0xca, 0xf5), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Tokyo Night Storm
// ---------------------------------------------------------------------------

/// Tokyo Night Storm palette (dark).
///
/// Source: <https://github.com/enkia/tokyo-night-vscode-theme>
/// License: MIT
pub const TOKYO_NIGHT_STORM: ThemePalette = ThemePalette {
    name: "Tokyo Night Storm",
    slug: "tokyo-night-storm",
    foreground: (0xa9, 0xb1, 0xd6),
    background: (0x24, 0x28, 0x3b),
    cursor: (0xc0, 0xca, 0xf5),
    cursor_text: (0x24, 0x28, 0x3b),
    selection_bg: (0x33, 0x46, 0x7c),
    selection_fg: (0xc0, 0xca, 0xf5),
    ansi: [
        (0x1d, 0x20, 0x2f), // 0  Black
        (0xf7, 0x76, 0x8e), // 1  Red
        (0x9e, 0xce, 0x6a), // 2  Green
        (0xe0, 0xaf, 0x68), // 3  Yellow
        (0x7a, 0xa2, 0xf7), // 4  Blue
        (0xbb, 0x9a, 0xf7), // 5  Magenta
        (0x7d, 0xcf, 0xff), // 6  Cyan
        (0xa9, 0xb1, 0xd6), // 7  White
        (0x41, 0x48, 0x68), // 8  BrightBlack
        (0xf7, 0x76, 0x8e), // 9  BrightRed
        (0x9e, 0xce, 0x6a), // 10 BrightGreen
        (0xe0, 0xaf, 0x68), // 11 BrightYellow
        (0x7a, 0xa2, 0xf7), // 12 BrightBlue
        (0xbb, 0x9a, 0xf7), // 13 BrightMagenta
        (0x7d, 0xcf, 0xff), // 14 BrightCyan
        (0xc0, 0xca, 0xf5), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Kanagawa
// ---------------------------------------------------------------------------

/// Kanagawa palette (dark).
///
/// Source: <https://github.com/rebelot/kanagawa.nvim>
/// License: MIT
pub const KANAGAWA: ThemePalette = ThemePalette {
    name: "Kanagawa",
    slug: "kanagawa",
    foreground: (0xdc, 0xd7, 0xba),
    background: (0x1f, 0x1f, 0x28),
    cursor: (0xc8, 0xc0, 0x93),
    cursor_text: (0x1f, 0x1f, 0x28),
    selection_bg: (0x2d, 0x4f, 0x67),
    selection_fg: (0xc8, 0xc0, 0x93),
    ansi: [
        (0x16, 0x16, 0x1d), // 0  Black
        (0xc3, 0x40, 0x43), // 1  Red
        (0x76, 0x94, 0x6a), // 2  Green
        (0xc0, 0xa3, 0x6e), // 3  Yellow
        (0x7e, 0x9c, 0xd8), // 4  Blue
        (0x95, 0x7f, 0xb8), // 5  Magenta
        (0x6a, 0x95, 0x89), // 6  Cyan
        (0xc8, 0xc0, 0x93), // 7  White
        (0x72, 0x71, 0x69), // 8  BrightBlack
        (0xe8, 0x24, 0x24), // 9  BrightRed
        (0x98, 0xbb, 0x6c), // 10 BrightGreen
        (0xe6, 0xc3, 0x84), // 11 BrightYellow
        (0x7f, 0xb4, 0xca), // 12 BrightBlue
        (0x93, 0x8a, 0xa9), // 13 BrightMagenta
        (0x7a, 0xa8, 0x9f), // 14 BrightCyan
        (0xdc, 0xd7, 0xba), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Rose Pine
// ---------------------------------------------------------------------------

/// Rose Pine palette (dark).
///
/// Source: <https://github.com/rose-pine/rose-pine-theme>
/// License: MIT
pub const ROSE_PINE: ThemePalette = ThemePalette {
    name: "Rose Pine",
    slug: "rose-pine",
    foreground: (0xe0, 0xde, 0xf4),
    background: (0x19, 0x17, 0x24),
    cursor: (0x55, 0x51, 0x69),
    cursor_text: (0xe0, 0xde, 0xf4),
    selection_bg: (0x2a, 0x28, 0x3e),
    selection_fg: (0xe0, 0xde, 0xf4),
    ansi: [
        (0x26, 0x23, 0x3a), // 0  Black
        (0xeb, 0x6f, 0x92), // 1  Red
        (0x31, 0x74, 0x8f), // 2  Green
        (0xf6, 0xc1, 0x77), // 3  Yellow
        (0x9c, 0xcf, 0xd8), // 4  Blue
        (0xc4, 0xa7, 0xe7), // 5  Magenta
        (0xeb, 0xbc, 0xba), // 6  Cyan
        (0xe0, 0xde, 0xf4), // 7  White
        (0x6e, 0x6a, 0x86), // 8  BrightBlack
        (0xeb, 0x6f, 0x92), // 9  BrightRed
        (0x31, 0x74, 0x8f), // 10 BrightGreen
        (0xf6, 0xc1, 0x77), // 11 BrightYellow
        (0x9c, 0xcf, 0xd8), // 12 BrightBlue
        (0xc4, 0xa7, 0xe7), // 13 BrightMagenta
        (0xeb, 0xbc, 0xba), // 14 BrightCyan
        (0xe0, 0xde, 0xf4), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Rose Pine Moon
// ---------------------------------------------------------------------------

/// Rose Pine Moon palette (dark).
///
/// Source: <https://github.com/rose-pine/rose-pine-theme>
/// License: MIT
pub const ROSE_PINE_MOON: ThemePalette = ThemePalette {
    name: "Rose Pine Moon",
    slug: "rose-pine-moon",
    foreground: (0xe0, 0xde, 0xf4),
    background: (0x23, 0x21, 0x36),
    cursor: (0x56, 0x52, 0x6e),
    cursor_text: (0xe0, 0xde, 0xf4),
    selection_bg: (0x2a, 0x28, 0x3e),
    selection_fg: (0xe0, 0xde, 0xf4),
    ansi: [
        (0x39, 0x35, 0x52), // 0  Black
        (0xeb, 0x6f, 0x92), // 1  Red
        (0x3e, 0x8f, 0xb0), // 2  Green
        (0xf6, 0xc1, 0x77), // 3  Yellow
        (0x9c, 0xcf, 0xd8), // 4  Blue
        (0xc4, 0xa7, 0xe7), // 5  Magenta
        (0xea, 0x9a, 0x97), // 6  Cyan
        (0xe0, 0xde, 0xf4), // 7  White
        (0x6e, 0x6a, 0x86), // 8  BrightBlack
        (0xeb, 0x6f, 0x92), // 9  BrightRed
        (0x3e, 0x8f, 0xb0), // 10 BrightGreen
        (0xf6, 0xc1, 0x77), // 11 BrightYellow
        (0x9c, 0xcf, 0xd8), // 12 BrightBlue
        (0xc4, 0xa7, 0xe7), // 13 BrightMagenta
        (0xea, 0x9a, 0x97), // 14 BrightCyan
        (0xe0, 0xde, 0xf4), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Rose Pine Dawn
// ---------------------------------------------------------------------------

/// Rose Pine Dawn palette (light).
///
/// Source: <https://github.com/rose-pine/rose-pine-theme>
/// License: MIT
pub const ROSE_PINE_DAWN: ThemePalette = ThemePalette {
    name: "Rose Pine Dawn",
    slug: "rose-pine-dawn",
    foreground: (0x57, 0x52, 0x79),
    background: (0xfa, 0xf4, 0xed),
    cursor: (0x98, 0x93, 0xa5),
    cursor_text: (0x57, 0x52, 0x79),
    selection_bg: (0xdf, 0xda, 0xd9),
    selection_fg: (0x57, 0x52, 0x79),
    ansi: [
        (0xf2, 0xe9, 0xe1), // 0  Black
        (0xb4, 0x63, 0x7a), // 1  Red
        (0x28, 0x69, 0x83), // 2  Green
        (0xea, 0x9d, 0x34), // 3  Yellow
        (0x56, 0x94, 0x9f), // 4  Blue
        (0x90, 0x7a, 0xa9), // 5  Magenta
        (0xd7, 0x82, 0x7e), // 6  Cyan
        (0x57, 0x52, 0x79), // 7  White
        (0x98, 0x93, 0xa5), // 8  BrightBlack
        (0xb4, 0x63, 0x7a), // 9  BrightRed
        (0x28, 0x69, 0x83), // 10 BrightGreen
        (0xea, 0x9d, 0x34), // 11 BrightYellow
        (0x56, 0x94, 0x9f), // 12 BrightBlue
        (0x90, 0x7a, 0xa9), // 13 BrightMagenta
        (0xd7, 0x82, 0x7e), // 14 BrightCyan
        (0x57, 0x52, 0x79), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Monokai Pro
// ---------------------------------------------------------------------------

/// Monokai Pro palette (dark).
///
/// Source: <https://monokai.pro>
/// License: Proprietary (color values widely published for terminal use)
pub const MONOKAI_PRO: ThemePalette = ThemePalette {
    name: "Monokai Pro",
    slug: "monokai-pro",
    foreground: (0xfc, 0xfc, 0xfa),
    background: (0x2d, 0x2a, 0x2e),
    cursor: (0xfc, 0xfc, 0xfa),
    cursor_text: (0x2d, 0x2a, 0x2e),
    selection_bg: (0x40, 0x3e, 0x41),
    selection_fg: (0xfc, 0xfc, 0xfa),
    ansi: [
        (0x40, 0x3e, 0x41), // 0  Black
        (0xff, 0x61, 0x88), // 1  Red
        (0xa9, 0xdc, 0x76), // 2  Green
        (0xff, 0xd8, 0x66), // 3  Yellow
        (0xfc, 0x98, 0x67), // 4  Blue
        (0xab, 0x9d, 0xf2), // 5  Magenta
        (0x78, 0xdc, 0xe8), // 6  Cyan
        (0xfc, 0xfc, 0xfa), // 7  White
        (0x72, 0x70, 0x72), // 8  BrightBlack
        (0xff, 0x61, 0x88), // 9  BrightRed
        (0xa9, 0xdc, 0x76), // 10 BrightGreen
        (0xff, 0xd8, 0x66), // 11 BrightYellow
        (0xfc, 0x98, 0x67), // 12 BrightBlue
        (0xab, 0x9d, 0xf2), // 13 BrightMagenta
        (0x78, 0xdc, 0xe8), // 14 BrightCyan
        (0xfc, 0xfc, 0xfa), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Ayu Dark
// ---------------------------------------------------------------------------

/// Ayu Dark palette (dark).
///
/// Source: <https://github.com/ayu-theme/ayu-colors>
/// License: MIT
pub const AYU_DARK: ThemePalette = ThemePalette {
    name: "Ayu Dark",
    slug: "ayu-dark",
    foreground: (0xbf, 0xbd, 0xb6),
    background: (0x0d, 0x10, 0x17),
    cursor: (0xe6, 0xb4, 0x50),
    cursor_text: (0x0d, 0x10, 0x17),
    selection_bg: (0x27, 0x37, 0x47),
    selection_fg: (0xbf, 0xbd, 0xb6),
    ansi: [
        (0x01, 0x06, 0x0e), // 0  Black
        (0xea, 0x6c, 0x73), // 1  Red
        (0x91, 0xb3, 0x62), // 2  Green
        (0xf9, 0xaf, 0x4f), // 3  Yellow
        (0x53, 0xbd, 0xfa), // 4  Blue
        (0xfa, 0xe9, 0x94), // 5  Magenta
        (0x90, 0xe1, 0xc6), // 6  Cyan
        (0xc7, 0xc7, 0xc7), // 7  White
        (0x68, 0x68, 0x68), // 8  BrightBlack
        (0xf0, 0x71, 0x78), // 9  BrightRed
        (0xc2, 0xd9, 0x4c), // 10 BrightGreen
        (0xff, 0xb4, 0x54), // 11 BrightYellow
        (0x59, 0xc2, 0xff), // 12 BrightBlue
        (0xff, 0xee, 0x99), // 13 BrightMagenta
        (0x95, 0xe6, 0xcb), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Ayu Light
// ---------------------------------------------------------------------------

/// Ayu Light palette (light).
///
/// Source: <https://github.com/ayu-theme/ayu-colors>
/// License: MIT
pub const AYU_LIGHT: ThemePalette = ThemePalette {
    name: "Ayu Light",
    slug: "ayu-light",
    foreground: (0x5c, 0x61, 0x66),
    background: (0xfa, 0xfa, 0xfa),
    cursor: (0xff, 0x6a, 0x00),
    cursor_text: (0xfa, 0xfa, 0xfa),
    selection_bg: (0xd1, 0xe4, 0xf4),
    selection_fg: (0x5c, 0x61, 0x66),
    ansi: [
        (0x00, 0x00, 0x00), // 0  Black
        (0xf5, 0x18, 0x18), // 1  Red
        (0x36, 0xb2, 0x29), // 2  Green
        (0xf5, 0x87, 0x1f), // 3  Yellow
        (0x31, 0x99, 0xe1), // 4  Blue
        (0xa3, 0x7a, 0xcc), // 5  Magenta
        (0x36, 0xb2, 0xaf), // 6  Cyan
        (0xff, 0xff, 0xff), // 7  White
        (0x32, 0x32, 0x32), // 8  BrightBlack
        (0xf5, 0x31, 0x1d), // 9  BrightRed
        (0x86, 0xb2, 0x2e), // 10 BrightGreen
        (0xf5, 0xa6, 0x23), // 11 BrightYellow
        (0x39, 0x9e, 0xe6), // 12 BrightBlue
        (0x9e, 0x75, 0xc7), // 13 BrightMagenta
        (0x4c, 0xbf, 0x99), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Everforest Dark
// ---------------------------------------------------------------------------

/// Everforest Dark palette (dark).
///
/// Source: <https://github.com/sainnhe/everforest>
/// License: MIT
pub const EVERFOREST_DARK: ThemePalette = ThemePalette {
    name: "Everforest Dark",
    slug: "everforest-dark",
    foreground: (0xd3, 0xc6, 0xaa),
    background: (0x2d, 0x35, 0x3b),
    cursor: (0xd3, 0xc6, 0xaa),
    cursor_text: (0x2d, 0x35, 0x3b),
    selection_bg: (0x54, 0x3a, 0x48),
    selection_fg: (0xd3, 0xc6, 0xaa),
    ansi: [
        (0x4b, 0x56, 0x5c), // 0  Black
        (0xe6, 0x7e, 0x80), // 1  Red
        (0xa7, 0xc0, 0x80), // 2  Green
        (0xdb, 0xbc, 0x7f), // 3  Yellow
        (0x7f, 0xbb, 0xb3), // 4  Blue
        (0xd6, 0x99, 0xb6), // 5  Magenta
        (0x83, 0xc0, 0x92), // 6  Cyan
        (0xd3, 0xc6, 0xaa), // 7  White
        (0x7a, 0x84, 0x78), // 8  BrightBlack
        (0xe6, 0x7e, 0x80), // 9  BrightRed
        (0xa7, 0xc0, 0x80), // 10 BrightGreen
        (0xdb, 0xbc, 0x7f), // 11 BrightYellow
        (0x7f, 0xbb, 0xb3), // 12 BrightBlue
        (0xd6, 0x99, 0xb6), // 13 BrightMagenta
        (0x83, 0xc0, 0x92), // 14 BrightCyan
        (0xd3, 0xc6, 0xaa), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Everforest Light
// ---------------------------------------------------------------------------

/// Everforest Light palette (light).
///
/// Source: <https://github.com/sainnhe/everforest>
/// License: MIT
pub const EVERFOREST_LIGHT: ThemePalette = ThemePalette {
    name: "Everforest Light",
    slug: "everforest-light",
    foreground: (0x5c, 0x6a, 0x72),
    background: (0xfd, 0xf6, 0xe3),
    cursor: (0x5c, 0x6a, 0x72),
    cursor_text: (0xfd, 0xf6, 0xe3),
    selection_bg: (0xea, 0xed, 0xc8),
    selection_fg: (0x5c, 0x6a, 0x72),
    ansi: [
        (0x5c, 0x6a, 0x72), // 0  Black
        (0xf8, 0x55, 0x52), // 1  Red
        (0x8d, 0xa1, 0x01), // 2  Green
        (0xdf, 0xa0, 0x00), // 3  Yellow
        (0x3a, 0x94, 0xc5), // 4  Blue
        (0xdf, 0x69, 0xba), // 5  Magenta
        (0x35, 0xa7, 0x7c), // 6  Cyan
        (0xdf, 0xdd, 0xc8), // 7  White
        (0x93, 0x9f, 0x91), // 8  BrightBlack
        (0xf8, 0x55, 0x52), // 9  BrightRed
        (0x8d, 0xa1, 0x01), // 10 BrightGreen
        (0xdf, 0xa0, 0x00), // 11 BrightYellow
        (0x3a, 0x94, 0xc5), // 12 BrightBlue
        (0xdf, 0x69, 0xba), // 13 BrightMagenta
        (0x35, 0xa7, 0x7c), // 14 BrightCyan
        (0xdf, 0xdd, 0xc8), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Material Dark
// ---------------------------------------------------------------------------

/// Material Dark palette (dark).
///
/// Source: <https://github.com/equinusocio/material-theme>
/// License: MIT
pub const MATERIAL_DARK: ThemePalette = ThemePalette {
    name: "Material Dark",
    slug: "material-dark",
    foreground: (0xee, 0xff, 0xff),
    background: (0x21, 0x21, 0x21),
    cursor: (0xff, 0xcc, 0x00),
    cursor_text: (0x21, 0x21, 0x21),
    selection_bg: (0x3f, 0x3f, 0x3f),
    selection_fg: (0xee, 0xff, 0xff),
    ansi: [
        (0x00, 0x00, 0x00), // 0  Black
        (0xe5, 0x4b, 0x4b), // 1  Red
        (0x9e, 0xc4, 0x00), // 2  Green
        (0xe6, 0xdb, 0x74), // 3  Yellow
        (0x7a, 0xa6, 0xda), // 4  Blue
        (0xc3, 0x97, 0xd8), // 5  Magenta
        (0x70, 0xc0, 0xb1), // 6  Cyan
        (0xea, 0xea, 0xea), // 7  White
        (0x66, 0x66, 0x66), // 8  BrightBlack
        (0xff, 0x73, 0x73), // 9  BrightRed
        (0xb9, 0xec, 0x58), // 10 BrightGreen
        (0xff, 0xe7, 0x88), // 11 BrightYellow
        (0x9c, 0xc4, 0xff), // 12 BrightBlue
        (0xe2, 0xbb, 0xf3), // 13 BrightMagenta
        (0x90, 0xe7, 0xd4), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  XTerm Default
// ---------------------------------------------------------------------------

/// `XTerm` Default palette.
///
/// Source: <https://invisible-island.net/xterm/>
/// License: MIT
pub const XTERM_DEFAULT: ThemePalette = ThemePalette {
    name: "XTerm Default",
    slug: "xterm-default",
    foreground: (0xd0, 0xd0, 0xd0),
    background: (0x00, 0x00, 0x00),
    cursor: (0xd0, 0xd0, 0xd0),
    cursor_text: (0x00, 0x00, 0x00),
    selection_bg: (0x4d, 0x4d, 0x4d),
    selection_fg: (0xd0, 0xd0, 0xd0),
    ansi: [
        (0x00, 0x00, 0x00), // 0  Black
        (0xcd, 0x00, 0x00), // 1  Red
        (0x00, 0xcd, 0x00), // 2  Green
        (0xcd, 0xcd, 0x00), // 3  Yellow
        (0x00, 0x00, 0xee), // 4  Blue
        (0xcd, 0x00, 0xcd), // 5  Magenta
        (0x00, 0xcd, 0xcd), // 6  Cyan
        (0xe5, 0xe5, 0xe5), // 7  White
        (0x7f, 0x7f, 0x7f), // 8  BrightBlack
        (0xff, 0x00, 0x00), // 9  BrightRed
        (0x00, 0xff, 0x00), // 10 BrightGreen
        (0xff, 0xff, 0x00), // 11 BrightYellow
        (0x5c, 0x5c, 0xff), // 12 BrightBlue
        (0xff, 0x00, 0xff), // 13 BrightMagenta
        (0x00, 0xff, 0xff), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  WezTerm Default
// ---------------------------------------------------------------------------

/// `WezTerm` Default palette.
///
/// Source: <https://github.com/wez/wezterm> (term/src/color.rs)
pub const WEZTERM_DEFAULT: ThemePalette = ThemePalette {
    name: "WezTerm Default",
    slug: "wezterm-default",
    foreground: (0xb2, 0xb2, 0xb2),
    background: (0x00, 0x00, 0x00),
    cursor: (0x52, 0xad, 0x70),
    cursor_text: (0x00, 0x00, 0x00),
    selection_bg: (0x4d, 0x40, 0x60),
    selection_fg: (0xd0, 0xd0, 0xd0),
    ansi: [
        (0x00, 0x00, 0x00), // 0  Black
        (0xcc, 0x55, 0x55), // 1  Red
        (0x55, 0xcc, 0x55), // 2  Green
        (0xcd, 0xcd, 0x55), // 3  Yellow
        (0x54, 0x55, 0xcb), // 4  Blue
        (0xcc, 0x55, 0xcc), // 5  Magenta
        (0x7a, 0xca, 0xca), // 6  Cyan
        (0xcc, 0xcc, 0xcc), // 7  White
        (0x55, 0x55, 0x55), // 8  BrightBlack
        (0xff, 0x55, 0x55), // 9  BrightRed
        (0x55, 0xff, 0x55), // 10 BrightGreen
        (0xff, 0xff, 0x55), // 11 BrightYellow
        (0x55, 0x55, 0xff), // 12 BrightBlue
        (0xff, 0x55, 0xff), // 13 BrightMagenta
        (0x55, 0xff, 0xff), // 14 BrightCyan
        (0xff, 0xff, 0xff), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

// ---------------------------------------------------------------------------
//  Ghostty Default (Tomorrow Night)
// ---------------------------------------------------------------------------

/// Ghostty Default palette (Tomorrow Night).
///
/// Source: <https://github.com/ghostty-org/ghostty> (src/terminal/color.zig)
pub const GHOSTTY_DEFAULT: ThemePalette = ThemePalette {
    name: "Ghostty Default",
    slug: "ghostty-default",
    foreground: (0xff, 0xff, 0xff),
    background: (0x28, 0x2c, 0x34),
    cursor: (0xff, 0xff, 0xff),
    cursor_text: (0x28, 0x2c, 0x34),
    selection_bg: (0x3e, 0x44, 0x52),
    selection_fg: (0xc5, 0xc8, 0xc6),
    ansi: [
        (0x1d, 0x1f, 0x21), // 0  Black
        (0xcc, 0x66, 0x66), // 1  Red
        (0xb5, 0xbd, 0x68), // 2  Green
        (0xf0, 0xc6, 0x74), // 3  Yellow
        (0x81, 0xa2, 0xbe), // 4  Blue
        (0xb2, 0x94, 0xbb), // 5  Magenta
        (0x8a, 0xbe, 0xb7), // 6  Cyan
        (0xc5, 0xc8, 0xc6), // 7  White
        (0x66, 0x66, 0x66), // 8  BrightBlack
        (0xd5, 0x4e, 0x53), // 9  BrightRed
        (0xb9, 0xca, 0x4a), // 10 BrightGreen
        (0xe7, 0xc5, 0x47), // 11 BrightYellow
        (0x7a, 0xa6, 0xda), // 12 BrightBlue
        (0xc3, 0x97, 0xd8), // 13 BrightMagenta
        (0x70, 0xc0, 0xb1), // 14 BrightCyan
        (0xea, 0xea, 0xea), // 15 BrightWhite
    ],
    gutter_success: None,
    gutter_failure: None,
    gutter_running: None,
    chrome_surface: None,
    chrome_surface_variant: None,
    chrome_surface_hover: None,
    chrome_surface_active: None,
    chrome_border: None,
    chrome_text: None,
    chrome_text_muted: None,
};

/// The default theme used when no theme is configured or the configured slug
/// is not recognized.
pub const DEFAULT_THEME: &ThemePalette = &CATPPUCCIN_MOCHA;

// ---------------------------------------------------------------------------
//  All embedded themes (sorted alphabetically by name)
// ---------------------------------------------------------------------------

/// All embedded themes sorted alphabetically by display name.
///
/// **Maintainer note:** Keep this array sorted by the theme's `name` field
/// (case-insensitive ASCII order). When adding a new theme, insert it in the
/// correct alphabetical position.
static ALL_THEMES: &[&ThemePalette] = &[
    &AYU_DARK,
    &AYU_LIGHT,
    &CATPPUCCIN_FRAPPE,
    &CATPPUCCIN_LATTE,
    &CATPPUCCIN_MACCHIATO,
    &CATPPUCCIN_MOCHA,
    &DRACULA,
    &EVERFOREST_DARK,
    &EVERFOREST_LIGHT,
    &GHOSTTY_DEFAULT,
    &GRUVBOX_DARK,
    &GRUVBOX_LIGHT,
    &KANAGAWA,
    &MATERIAL_DARK,
    &MONOKAI_PRO,
    &NORD,
    &ONE_DARK,
    &ONE_LIGHT,
    &ROSE_PINE,
    &ROSE_PINE_DAWN,
    &ROSE_PINE_MOON,
    &SOLARIZED_DARK,
    &SOLARIZED_LIGHT,
    &TOKYO_NIGHT,
    &TOKYO_NIGHT_STORM,
    &WEZTERM_DEFAULT,
    &XTERM_DEFAULT,
];

/// Return all embedded themes sorted alphabetically by display name.
#[must_use]
pub fn all_themes() -> &'static [&'static ThemePalette] {
    ALL_THEMES
}

/// Look up an embedded theme by its slug.
///
/// Returns `None` if no theme matches.
#[must_use]
pub fn by_slug(slug: &str) -> Option<&'static ThemePalette> {
    ALL_THEMES.iter().find(|t| t.slug == slug).copied()
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn catppuccin_mocha_foreground_matches_gui_colors() {
        // TEXT in gui/colors.rs: Color32::from_rgb(0xcd, 0xd6, 0xf4)
        assert_eq!(CATPPUCCIN_MOCHA.foreground, (0xcd, 0xd6, 0xf4));
    }

    #[test]
    fn catppuccin_mocha_background_matches_gui_colors() {
        // BASE in gui/colors.rs: Color32::from_rgb(0x1e, 0x1e, 0x2e)
        assert_eq!(CATPPUCCIN_MOCHA.background, (0x1e, 0x1e, 0x2e));
    }

    #[test]
    fn catppuccin_mocha_cursor_matches_gui_colors() {
        assert_eq!(CATPPUCCIN_MOCHA.cursor, (0xf5, 0xe0, 0xdc));
        assert_eq!(CATPPUCCIN_MOCHA.cursor_text, (0x11, 0x11, 0x1b));
    }

    #[test]
    fn catppuccin_mocha_selection_matches_gui_colors() {
        assert_eq!(CATPPUCCIN_MOCHA.selection_bg, (0xa0, 0xa4, 0xb8));
        assert_eq!(CATPPUCCIN_MOCHA.selection_fg, (0x11, 0x11, 0x1b));
    }

    #[test]
    fn catppuccin_mocha_ansi_matches_default_index_to_rgb() {
        // Verify each ANSI color matches the hardcoded values in
        // freminal-common/src/colors.rs default_index_to_rgb()
        let expected: [(u8, u8, u8); 16] = [
            (0x45, 0x47, 0x5a), // 0  Black
            (0xf3, 0x8b, 0xa8), // 1  Red
            (0xa6, 0xe3, 0xa1), // 2  Green
            (0xf9, 0xe2, 0xaf), // 3  Yellow
            (0x89, 0xb4, 0xfa), // 4  Blue
            (0xf5, 0xc2, 0xe7), // 5  Magenta
            (0x94, 0xe2, 0xd5), // 6  Cyan
            (0xa6, 0xad, 0xc8), // 7  White
            (0x58, 0x5b, 0x70), // 8  BrightBlack
            (0xf3, 0x77, 0x99), // 9  BrightRed
            (0x89, 0xd8, 0x8b), // 10 BrightGreen
            (0xeb, 0xd3, 0x91), // 11 BrightYellow
            (0x74, 0xa8, 0xfc), // 12 BrightBlue
            (0xf2, 0xae, 0xde), // 13 BrightMagenta
            (0x6b, 0xd7, 0xca), // 14 BrightCyan
            (0xba, 0xc2, 0xde), // 15 BrightWhite
        ];

        for (i, exp) in expected.iter().enumerate() {
            assert_eq!(
                &CATPPUCCIN_MOCHA.ansi[i], exp,
                "ANSI color index {i} mismatch"
            );
        }
    }

    #[test]
    fn by_slug_finds_catppuccin_mocha() {
        let theme = by_slug("catppuccin-mocha").unwrap();
        assert_eq!(theme.name, "Catppuccin Mocha");
        assert_eq!(theme.slug, "catppuccin-mocha");
    }

    #[test]
    fn by_slug_returns_none_for_unknown() {
        assert!(by_slug("nonexistent-theme").is_none());
        assert!(by_slug("").is_none());
    }

    #[test]
    fn all_themes_is_non_empty() {
        assert!(!all_themes().is_empty());
    }

    #[test]
    fn all_themes_contains_27_themes() {
        assert_eq!(
            all_themes().len(),
            27,
            "expected 27 themes, got {}",
            all_themes().len()
        );
    }

    #[test]
    fn all_themes_contains_catppuccin_mocha() {
        assert!(
            all_themes().iter().any(|t| t.slug == "catppuccin-mocha"),
            "all_themes() must contain catppuccin-mocha"
        );
    }

    #[test]
    fn all_slugs_are_unique() {
        let themes = all_themes();
        for (i, a) in themes.iter().enumerate() {
            for b in &themes[i + 1..] {
                assert_ne!(a.slug, b.slug, "duplicate slug: {}", a.slug);
            }
        }
    }

    #[test]
    fn all_names_are_unique() {
        let themes = all_themes();
        for (i, a) in themes.iter().enumerate() {
            for b in &themes[i + 1..] {
                assert_ne!(a.name, b.name, "duplicate name: {}", a.name);
            }
        }
    }

    #[test]
    fn default_theme_is_catppuccin_mocha() {
        assert_eq!(*DEFAULT_THEME, CATPPUCCIN_MOCHA);
    }

    #[test]
    fn all_themes_by_slug_roundtrip() {
        for theme in all_themes() {
            let found = by_slug(theme.slug).unwrap();
            assert_eq!(found.slug, theme.slug);
            assert_eq!(found.name, theme.name);
        }
    }

    #[test]
    fn all_ansi_arrays_have_16_entries() {
        for theme in all_themes() {
            assert_eq!(
                theme.ansi.len(),
                16,
                "theme {} has {} ANSI colors, expected 16",
                theme.slug,
                theme.ansi.len()
            );
        }
    }

    #[test]
    fn all_themes_sorted_alphabetically_by_name() {
        let themes = all_themes();
        for window in themes.windows(2) {
            let a = window[0].name.to_ascii_lowercase();
            let b = window[1].name.to_ascii_lowercase();
            assert!(
                a <= b,
                "ALL_THEMES is not sorted alphabetically by name: \
                 {:?} should come after {:?}",
                window[0].name,
                window[1].name,
            );
        }
    }

    #[test]
    fn all_themes_default_gutter_overrides_to_none() {
        // No shipped theme customizes the gutter colors yet; they all
        // rely on the ANSI-derived fallback.
        for theme in all_themes() {
            assert_eq!(theme.gutter_success, None, "theme {}", theme.slug);
            assert_eq!(theme.gutter_failure, None, "theme {}", theme.slug);
            assert_eq!(theme.gutter_running, None, "theme {}", theme.slug);
        }
    }

    #[test]
    fn gutter_color_for_falls_back_to_ansi_when_unset() {
        let t = CATPPUCCIN_MOCHA;
        assert_eq!(t.gutter_color_for(CommandStatus::Success), t.ansi[2]);
        assert_eq!(t.gutter_color_for(CommandStatus::Failure(1)), t.ansi[1]);
        assert_eq!(t.gutter_color_for(CommandStatus::Running), t.ansi[3]);
        // Unknown has no override; it always uses normal white (ansi[7]).
        assert_eq!(t.gutter_color_for(CommandStatus::Unknown), t.ansi[7]);
    }

    #[test]
    fn gutter_color_for_failure_ignores_exit_code_value() {
        let t = CATPPUCCIN_MOCHA;
        // The exit code carried by Failure does not change the color.
        assert_eq!(
            t.gutter_color_for(CommandStatus::Failure(1)),
            t.gutter_color_for(CommandStatus::Failure(137)),
        );
    }

    #[test]
    fn gutter_color_for_prefers_override_when_set() {
        let mut t = CATPPUCCIN_MOCHA;
        t.gutter_success = Some((1, 2, 3));
        t.gutter_failure = Some((4, 5, 6));
        t.gutter_running = Some((7, 8, 9));
        assert_eq!(t.gutter_color_for(CommandStatus::Success), (1, 2, 3));
        assert_eq!(t.gutter_color_for(CommandStatus::Failure(2)), (4, 5, 6));
        assert_eq!(t.gutter_color_for(CommandStatus::Running), (7, 8, 9));
        // Unknown is unaffected by overrides.
        assert_eq!(t.gutter_color_for(CommandStatus::Unknown), t.ansi[7]);
    }

    // --- Chrome-role resolver (112.3c) -----------------------------------

    #[test]
    fn chrome_roles_default_to_none_on_all_themes() {
        for theme in all_themes() {
            assert_eq!(theme.chrome_surface, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_surface_variant, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_surface_hover, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_surface_active, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_border, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_text, None, "theme {}", theme.slug);
            assert_eq!(theme.chrome_text_muted, None, "theme {}", theme.slug);
        }
    }

    #[test]
    fn chrome_role_prefers_authored_value() {
        let mut t = CATPPUCCIN_MOCHA;
        t.chrome_surface = Some((1, 2, 3));
        t.chrome_surface_variant = Some((4, 5, 6));
        t.chrome_surface_hover = Some((7, 8, 9));
        t.chrome_surface_active = Some((10, 11, 12));
        t.chrome_border = Some((13, 14, 15));
        t.chrome_text = Some((16, 17, 18));
        t.chrome_text_muted = Some((19, 20, 21));

        assert_eq!(t.chrome_role(ChromeRole::Surface), (1, 2, 3));
        assert_eq!(t.chrome_role(ChromeRole::SurfaceVariant), (4, 5, 6));
        assert_eq!(t.chrome_role(ChromeRole::SurfaceHover), (7, 8, 9));
        assert_eq!(t.chrome_role(ChromeRole::SurfaceActive), (10, 11, 12));
        assert_eq!(t.chrome_role(ChromeRole::Border), (13, 14, 15));
        assert_eq!(t.chrome_role(ChromeRole::Text), (16, 17, 18));
        assert_eq!(t.chrome_role(ChromeRole::TextMuted), (19, 20, 21));
    }

    #[test]
    fn chrome_role_best_fits_from_palette_when_unset() {
        // With no authored values, surface == background, text == foreground,
        // active == selection_bg (colors the palette already defines).
        let t = CATPPUCCIN_MOCHA;
        assert_eq!(t.chrome_role(ChromeRole::Surface), t.background);
        assert_eq!(t.chrome_role(ChromeRole::Text), t.foreground);
        assert_eq!(t.chrome_role(ChromeRole::SurfaceActive), t.selection_bg);
    }

    #[test]
    fn chrome_border_always_contrasts_the_surface() {
        // The fix for "invisible" borders: on EVERY built-in theme, the
        // resolved border must visibly separate from the surface.
        for theme in all_themes() {
            let surface = theme.chrome_role(ChromeRole::Surface);
            let border = theme.chrome_role(ChromeRole::Border);
            assert!(
                contrast_ratio(border, surface) > 1.2,
                "theme {}: border {border:?} indistinguishable from surface {surface:?}",
                theme.slug
            );
        }
    }

    #[test]
    fn chrome_role_contrast_fallback_meets_threshold() {
        // A pathological palette whose background and foreground are nearly
        // identical must still yield a muted-text color that separates from
        // the surface via the contrast fallback.
        let mut t = CATPPUCCIN_MOCHA;
        t.background = (20, 20, 20);
        t.foreground = (28, 28, 28); // very low contrast with background
        let muted = t.chrome_role(ChromeRole::TextMuted);
        assert!(
            contrast_ratio(muted, t.background) >= MIN_CHROME_CONTRAST,
            "muted text {muted:?} must meet MIN_CHROME_CONTRAST against {:?}",
            t.background
        );
    }

    #[test]
    fn chrome_text_on_picks_higher_contrast() {
        let t = CATPPUCCIN_MOCHA;
        // On a near-white surface, the chosen text must be dark; on a
        // near-black surface, light — whichever contrasts more.
        let on_white = t.chrome_text_on((250, 250, 250));
        let on_black = t.chrome_text_on((5, 5, 5));
        assert!(
            contrast_ratio(on_white, (250, 250, 250)) >= 3.0,
            "text on near-white must be reasonably dark"
        );
        assert!(
            contrast_ratio(on_black, (5, 5, 5)) >= 3.0,
            "text on near-black must be reasonably light"
        );
    }

    #[test]
    fn contrast_ratio_is_symmetric_and_bounded() {
        let a = (0, 0, 0);
        let b = (255, 255, 255);
        let r = contrast_ratio(a, b);
        assert!((r - contrast_ratio(b, a)).abs() < f32::EPSILON);
        // Black vs white is the maximum WCAG ratio, 21:1.
        assert!((r - 21.0).abs() < 0.1, "black/white must be ~21:1, got {r}");
    }
}
