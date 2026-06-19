// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toolkit-agnostic GUI styling geometry types for the Freminal UI.
//!
//! This module defines the *shape* of the UI (corner radii, stroke widths,
//! spacing) without any dependency on a specific rendering framework.  Colors
//! are intentionally absent here — they live in [`crate::themes::ThemePalette`].
//!
//! The main types are:
//!
//! - [`StyleProfile`] — a named preset (`Modern` or `Retro`) that seeds a
//!   [`GuiTheme`].
//! - [`GuiTheme`] — a flat bag of geometry values consumed by subtask 112.3a
//!   to produce the actual egui `Visuals`.
//!
//! The baseline numbers chosen here are **starting-point defaults** that will
//! be empirically tuned in subtask 112.3a once the values can be evaluated
//! in a running UI.  Do not treat them as final.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
//  StyleProfile
// ---------------------------------------------------------------------------

/// Selects a named geometry preset.
///
/// Each variant provides a [`defaults`](StyleProfile::defaults) method that
/// returns a [`GuiTheme`] populated with sensible baseline values for that
/// visual style.  These baselines are **starting-point estimates** — they
/// will be empirically tuned in subtask 112.3a once the values can be
/// evaluated in a running UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StyleProfile {
    /// Rounded corners, soft borders, and generous spacing.
    ///
    /// Intended to match the visual style of modern applications such as
    /// VS Code and Ghostty.
    Modern,

    /// Sharp (zero-radius) corners, harder borders, and tighter spacing.
    ///
    /// Intended to evoke the look of classic terminal applications and
    /// older desktop environments.
    Retro,
}

impl Default for StyleProfile {
    /// Returns [`StyleProfile::Modern`].
    fn default() -> Self {
        Self::Modern
    }
}

impl StyleProfile {
    /// Return the baseline [`GuiTheme`] geometry for this profile.
    ///
    /// The values here are **starting-point defaults** and will be
    /// empirically tuned in subtask 112.3a.  Treat them as reasonable
    /// first guesses, not final decisions.
    ///
    /// # Modern baseline rationale
    ///
    /// - `corner_radius = 6` — matches the default rounding used by egui's
    ///   built-in visuals and is close to the radius used by Ghostty and
    ///   VS Code.
    /// - `stroke_width = 1.0` — one logical pixel, barely visible, giving
    ///   a clean hairline border.
    /// - `item_spacing = (8.0, 4.0)` — egui's own default is (8, 3);
    ///   a small extra vertical gap improves legibility at small font sizes.
    /// - `window_padding = 8.0` — matches egui's default inner padding so
    ///   existing layout code does not shift.
    /// - `menu_corner_radius = 4` — menus are nested UI so a slightly
    ///   smaller radius keeps the visual hierarchy clear.
    /// - `widget_hover_expansion = 2.0` — gentle grow-on-hover; keeps
    ///   interactive elements crisp without jarring motion.
    ///
    /// # Retro baseline rationale
    ///
    /// - `corner_radius = 0` — hard rectangles everywhere.
    /// - `stroke_width = 1.5` — slightly heavier than Modern to make
    ///   borders readable without rounding.
    /// - `item_spacing = (6.0, 2.0)` — tighter to maximise information
    ///   density, closer to old-school TUI conventions.
    /// - `window_padding = 4.0` — minimal breathing room.
    /// - `menu_corner_radius = 0` — consistent with no rounding elsewhere.
    /// - `widget_hover_expansion = 0.0` — no expansion; hover is expressed
    ///   through colour change only (handled in the palette layer).
    #[must_use]
    pub const fn defaults(self) -> GuiTheme {
        match self {
            Self::Modern => GuiTheme {
                profile: Self::Modern,
                corner_radius: 6,
                stroke_width: 1.0,
                item_spacing: (8.0, 6.0),
                window_padding: 10.0,
                menu_corner_radius: 4,
                widget_hover_expansion: 2.0,
                button_padding: (10.0, 6.0),
                menu_padding: 8,
                text_edit_padding: (8.0, 5.0),
            },
            Self::Retro => GuiTheme {
                profile: Self::Retro,
                corner_radius: 0,
                stroke_width: 1.5,
                item_spacing: (6.0, 4.0),
                window_padding: 6.0,
                menu_corner_radius: 0,
                widget_hover_expansion: 0.0,
                button_padding: (8.0, 4.0),
                menu_padding: 6,
                text_edit_padding: (6.0, 4.0),
            },
        }
    }
}

// ---------------------------------------------------------------------------
//  GuiTheme
// ---------------------------------------------------------------------------

/// Toolkit-agnostic geometry settings for the Freminal GUI.
///
/// This struct carries the *shape* of the UI — radii, weights, and spacing —
/// without any dependency on egui or any other GUI crate.  It is consumed by
/// subtask 112.3a to derive an egui [`Visuals`](egui::Visuals) combined with
/// a [`ThemePalette`](crate::themes::ThemePalette) for colors.
///
/// **No color values are stored here.**  Use [`ThemePalette`](crate::themes::ThemePalette)
/// for color data.
///
/// The canonical way to obtain a `GuiTheme` is via
/// [`StyleProfile::defaults`].  Fields may then be overridden per the user's
/// preferences (that wiring happens in subtask 112.13).
///
/// # Note on `PartialEq` but not `Eq`
///
/// `GuiTheme` derives `PartialEq` but deliberately does **not** derive `Eq`
/// because it contains `f32` fields, which do not satisfy the `Eq` contract
/// (NaN ≠ NaN).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GuiTheme {
    /// The style profile this theme was built from.
    ///
    /// Stored so serialized themes are self-describing; also used for
    /// display in a future settings UI.
    pub profile: StyleProfile,

    /// Uniform corner radius applied to most widgets (buttons, text fields,
    /// panels), in logical pixels.
    ///
    /// `0` produces hard rectangles; values above ~8 start to look pill-like.
    pub corner_radius: u8,

    /// Width of border strokes in logical pixels.
    ///
    /// Applies to widget outlines and panel frames.  A value of `1.0` gives
    /// a clean hairline.  Retro UIs typically use `1.5` for crisper edges.
    pub stroke_width: f32,

    /// Horizontal and vertical item-to-item spacing in logical pixels
    /// `(x, y)`.
    ///
    /// Controls the gap between adjacent widgets inside a layout.  Higher
    /// values feel airier; lower values increase information density.
    pub item_spacing: (f32, f32),

    /// Inner padding of windows and panels in logical pixels.
    ///
    /// Applied symmetrically to all four edges of a window's content area.
    pub window_padding: f32,

    /// Corner radius for menu and dropdown widgets in logical pixels.
    ///
    /// Kept separate from [`corner_radius`](GuiTheme::corner_radius) because
    /// menus are visually nested and often warrant a slightly smaller radius
    /// than top-level containers to maintain hierarchy.  Set equal to
    /// `corner_radius` if no distinction is desired.
    pub menu_corner_radius: u8,

    /// Additional size added to the bounding rect of a widget on hover, in
    /// logical pixels (applied symmetrically).
    ///
    /// A non-zero value gives a subtle grow-on-hover effect.  Set to `0.0`
    /// to express hover state through color alone (Retro default).
    pub widget_hover_expansion: f32,

    /// Inner padding of buttons / selectable widgets in logical pixels
    /// `(x, y)`.
    ///
    /// Maps to egui's `Spacing.button_padding`.  Larger values give roomier,
    /// more modern-feeling controls; the egui default is `(4.0, 1.0)`.
    pub button_padding: (f32, f32),

    /// Inner padding of menu / dropdown / popup frames in logical pixels.
    ///
    /// Maps to egui's `Spacing.menu_margin` (applied to every combo-box
    /// dropdown, context menu, and popup).  The egui default is `6`.
    pub menu_padding: u8,

    /// Inner padding of text-input fields in logical pixels `(x, y)`.
    ///
    /// egui has no global `Spacing` field for this, so it is applied per
    /// `TextEdit` via `.margin(...)`.  The egui default is `(4.0, 2.0)`.
    pub text_edit_padding: (f32, f32),
}

impl Default for GuiTheme {
    /// Returns the result of <code>[StyleProfile::Modern].defaults()</code>.
    ///
    /// This is the default GUI theme used when no explicit theme is
    /// configured.
    fn default() -> Self {
        StyleProfile::Modern.defaults()
    }
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    //  defaults() geometry checks
    // -----------------------------------------------------------------------

    #[test]
    fn modern_defaults_corner_radius_is_nonzero() {
        let theme = StyleProfile::Modern.defaults();
        assert!(
            theme.corner_radius > 0,
            "Modern profile should have a nonzero corner radius (got {})",
            theme.corner_radius
        );
    }

    #[test]
    fn retro_defaults_corner_radius_is_zero() {
        let theme = StyleProfile::Retro.defaults();
        assert_eq!(
            theme.corner_radius, 0,
            "Retro profile must have corner_radius == 0"
        );
    }

    #[test]
    fn modern_and_retro_defaults_differ() {
        let modern = StyleProfile::Modern.defaults();
        let retro = StyleProfile::Retro.defaults();
        assert_ne!(
            modern, retro,
            "Modern and Retro defaults must not be identical"
        );
    }

    #[test]
    fn modern_defaults_have_expected_baseline_values() {
        let t = StyleProfile::Modern.defaults();
        assert_eq!(t.corner_radius, 6);
        assert_eq!(t.stroke_width, 1.0_f32);
        assert_eq!(t.item_spacing, (8.0_f32, 6.0_f32));
        assert_eq!(t.window_padding, 10.0_f32);
        assert_eq!(t.menu_corner_radius, 4);
        assert_eq!(t.widget_hover_expansion, 2.0_f32);
        assert_eq!(t.button_padding, (10.0_f32, 6.0_f32));
        assert_eq!(t.menu_padding, 8);
        assert_eq!(t.text_edit_padding, (8.0_f32, 5.0_f32));
    }

    #[test]
    fn retro_defaults_have_expected_baseline_values() {
        let t = StyleProfile::Retro.defaults();
        assert_eq!(t.corner_radius, 0);
        assert_eq!(t.stroke_width, 1.5_f32);
        assert_eq!(t.item_spacing, (6.0_f32, 4.0_f32));
        assert_eq!(t.window_padding, 6.0_f32);
        assert_eq!(t.menu_corner_radius, 0);
        assert_eq!(t.widget_hover_expansion, 0.0_f32);
        assert_eq!(t.button_padding, (8.0_f32, 4.0_f32));
        assert_eq!(t.menu_padding, 6);
        assert_eq!(t.text_edit_padding, (6.0_f32, 4.0_f32));
    }

    // -----------------------------------------------------------------------
    //  Default impl
    // -----------------------------------------------------------------------

    #[test]
    fn default_gui_theme_equals_modern_defaults() {
        assert_eq!(GuiTheme::default(), StyleProfile::Modern.defaults());
    }

    #[test]
    fn default_style_profile_is_modern() {
        assert_eq!(StyleProfile::default(), StyleProfile::Modern);
    }

    // -----------------------------------------------------------------------
    //  Serde round-trip (TOML)
    // -----------------------------------------------------------------------

    #[test]
    fn gui_theme_toml_roundtrip_modern() {
        let original = StyleProfile::Modern.defaults();
        let toml_str = toml::to_string_pretty(&original).expect("serialize");
        let parsed: GuiTheme = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn gui_theme_toml_roundtrip_retro() {
        let original = StyleProfile::Retro.defaults();
        let toml_str = toml::to_string_pretty(&original).expect("serialize");
        let parsed: GuiTheme = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed, original);
    }

    /// Helper wrapper so we can round-trip `StyleProfile` through TOML.
    ///
    /// TOML requires a root key=value structure; a bare string value at the
    /// document root is not valid TOML.  Wrapping in a struct avoids this
    /// limitation while still exercising the `rename_all = "lowercase"` serde
    /// attribute on `StyleProfile`.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct ProfileWrapper {
        profile: StyleProfile,
    }

    #[test]
    fn style_profile_serializes_lowercase() {
        // serde rename_all = "lowercase" must produce "modern" / "retro" in
        // the serialized TOML value.  We use a wrapper struct because TOML
        // cannot serialize a bare string at the document root.
        let modern_str = toml::to_string(&ProfileWrapper {
            profile: StyleProfile::Modern,
        })
        .expect("serialize");
        let retro_str = toml::to_string(&ProfileWrapper {
            profile: StyleProfile::Retro,
        })
        .expect("serialize");

        // The serialized form must contain the lowercase name.
        assert!(
            modern_str.contains("modern"),
            "expected 'modern' in TOML output, got: {modern_str}"
        );
        assert!(
            retro_str.contains("retro"),
            "expected 'retro' in TOML output, got: {retro_str}"
        );
        // Must NOT serialize as the Rust variant name (PascalCase).
        assert!(
            !modern_str.contains("Modern"),
            "must not contain PascalCase 'Modern' in TOML output"
        );
        assert!(
            !retro_str.contains("Retro"),
            "must not contain PascalCase 'Retro' in TOML output"
        );
    }

    #[test]
    fn style_profile_deserializes_lowercase() {
        // TOML requires key=value; use a wrapper struct to test round-trip.
        let modern: ProfileWrapper = toml::from_str("profile = \"modern\"").expect("parse modern");
        let retro: ProfileWrapper = toml::from_str("profile = \"retro\"").expect("parse retro");
        assert_eq!(modern.profile, StyleProfile::Modern);
        assert_eq!(retro.profile, StyleProfile::Retro);
    }

    #[test]
    fn gui_theme_profile_field_survives_roundtrip() {
        // Ensure the `profile` field inside GuiTheme is also serialized
        // in lowercase and deserializes correctly.
        let theme = StyleProfile::Retro.defaults();
        let toml_str = toml::to_string_pretty(&theme).expect("serialize");
        assert!(
            toml_str.contains("retro"),
            "profile field must serialize as 'retro', got:\n{toml_str}"
        );
        let parsed: GuiTheme = toml::from_str(&toml_str).expect("parse");
        assert_eq!(parsed.profile, StyleProfile::Retro);
    }
}
