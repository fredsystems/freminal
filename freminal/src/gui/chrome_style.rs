// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Centralized mapping from [`GuiTheme`] geometry and [`ThemePalette`] colors
//! to an egui [`Visuals`].
//!
//! # Design
//!
//! `build_visuals` is a **pure function**: given style geometry and a color
//! palette it produces a fully-specified `Visuals`.  It does **not** apply
//! the result to any egui context — that is wired in subtask 112.4.
//!
//! ## Widget state color scheme
//!
//! All colors are derived from the active [`ThemePalette`].  No hues are
//! hard-coded except for the documented reverse-video white.
//!
//! | State           | `bg_fill` / `weak_bg_fill`               | `bg_stroke` color        | `fg_stroke` color      | `expansion`                    |
//! |-----------------|------------------------------------------|--------------------------|------------------------|-------------------------------|
//! | `noninteractive`| `background` (window/panel surface)      | `ansi[0]` (dim border)   | `foreground` (text)    | `0.0`                          |
//! | `inactive`      | `ansi[0]` (button rest bg)               | `ansi[0]` (dim border)   | `foreground` (text)    | `0.0`                          |
//! | `hovered`       | `selection_bg` (highlight bg)            | `selection_fg`           | `foreground`           | `widget_hover_expansion`       |
//! | `active`        | `selection_bg` at 80% alpha (pressed)    | `selection_fg`           | `foreground`           | `widget_hover_expansion`       |
//! | `open`          | `ansi[8]` (bright-black, lifted bg)      | `foreground`             | `foreground`           | `widget_hover_expansion`       |
//!
//! `ansi[0]` is the "black" slot in the palette — typically a very dark or
//! slightly-lifted neutral that reads as a dim border against the background.
//! `ansi[8]` is the "bright black" slot — slightly lighter than `ansi[0]`,
//! used for the *open* (combo-box / drop-down) state to visually distinguish
//! it from the inactive resting state.

use conv2::ConvUtil;
use egui::style::{Selection, WidgetVisuals, Widgets};
use egui::{Color32, CornerRadius, Stroke};
use freminal_common::colors::TerminalColor;
use freminal_common::gui_theme::GuiTheme;
use freminal_common::themes::ThemePalette;

use super::colors::{internal_color_to_egui_with_alpha, rgb_to_color32};

// ---------------------------------------------------------------------------
//  WidgetRole — private helper for per-state parameterisation
// ---------------------------------------------------------------------------

/// Identifies the interaction state for a [`WidgetVisuals`] instance.
///
/// Used by [`widget`] to pick the correct colors and expansion without
/// duplicating the construction five times.
#[derive(Clone, Copy)]
enum WidgetRole {
    /// Non-interactive chrome: labels, separators, window outlines.
    NonInteractive,
    /// Interactive widget at rest (button, checkbox, …).
    Inactive,
    /// Widget under the pointer (hovered / highlighted).
    Hovered,
    /// Widget being clicked or dragged.
    Active,
    /// Widget with an open submenu / combo-box.
    Open,
}

// ---------------------------------------------------------------------------
//  Private per-state builder
// ---------------------------------------------------------------------------

/// Build a single [`WidgetVisuals`] for the given interaction `role`.
///
/// See the module-level doc table for the full color scheme rationale.
fn widget(palette: &ThemePalette, gui_theme: &GuiTheme, role: WidgetRole) -> WidgetVisuals {
    let cr = CornerRadius::same(gui_theme.corner_radius);
    let sw = gui_theme.stroke_width;
    let exp = gui_theme.widget_hover_expansion;

    let foreground = rgb_to_color32(palette.foreground);
    let background = rgb_to_color32(palette.background);
    let dim_border = rgb_to_color32(palette.ansi[0]);
    // selection_bg is the fill color for hover/active states.
    let sel_fill = rgb_to_color32(palette.selection_bg);
    // selection_fg is used for strokes on hover/active states.
    let sel_text = rgb_to_color32(palette.selection_fg);
    let lifted = rgb_to_color32(palette.ansi[8]);

    // Active state: selection_bg at 80% alpha — signals "pressed" without
    // being identical to the hover fill.
    let active_fill = {
        let [red, green, blue, _] = sel_fill.to_array();
        let alpha = (0.8_f32 * 255.0).approx_as::<u8>().unwrap_or(204_u8);
        Color32::from_rgba_unmultiplied(red, green, blue, alpha)
    };

    match role {
        WidgetRole::NonInteractive => WidgetVisuals {
            bg_fill: background,
            weak_bg_fill: background,
            bg_stroke: Stroke::new(sw, dim_border),
            corner_radius: cr,
            fg_stroke: Stroke::new(sw, foreground),
            expansion: 0.0,
        },
        WidgetRole::Inactive => WidgetVisuals {
            bg_fill: dim_border,
            weak_bg_fill: dim_border,
            bg_stroke: Stroke::new(sw, dim_border),
            corner_radius: cr,
            fg_stroke: Stroke::new(sw, foreground),
            expansion: 0.0,
        },
        WidgetRole::Hovered => WidgetVisuals {
            bg_fill: sel_fill,
            weak_bg_fill: sel_fill,
            bg_stroke: Stroke::new(sw, sel_text),
            corner_radius: cr,
            fg_stroke: Stroke::new(sw, foreground),
            expansion: exp,
        },
        WidgetRole::Active => WidgetVisuals {
            bg_fill: active_fill,
            weak_bg_fill: active_fill,
            bg_stroke: Stroke::new(sw, sel_text),
            corner_radius: cr,
            fg_stroke: Stroke::new(sw, foreground),
            expansion: exp,
        },
        WidgetRole::Open => WidgetVisuals {
            bg_fill: lifted,
            weak_bg_fill: lifted,
            bg_stroke: Stroke::new(sw, foreground),
            corner_radius: cr,
            fg_stroke: Stroke::new(sw, foreground),
            expansion: exp,
        },
    }
}

// ---------------------------------------------------------------------------
//  Public API
// ---------------------------------------------------------------------------

/// Build a fully-specified [`egui::Visuals`] from style geometry and palette.
///
/// # Parameters
///
/// - `gui_theme` — geometry (corner radii, stroke widths, spacing, hover
///   expansion).  All geometry comes from here; no values are hard-coded.
/// - `palette` — color data.  Every color in the returned `Visuals` is
///   derived from this palette, **except** for the reverse-video white
///   documented in the `normal_display == false` case below.
/// - `bg_opacity` — background opacity in `[0.0, 1.0]`.  Applied to
///   `panel_fill` only; `window_fill` is always fully opaque (so menus,
///   settings modal, and chrome remain readable over transparent terminals).
/// - `normal_display` — when `true` (normal display mode) both fills are
///   derived from `palette.background`.  When `false` (reverse-video mode)
///   both fills are forced to solid white, matching the existing behaviour
///   in `app_impl.rs`.
///
/// # Color derivation table
///
/// | `Visuals` field          | Source                                                |
/// |--------------------------|-------------------------------------------------------|
/// | `window_fill`            | `palette.background` (opaque) / white (reverse video) |
/// | `panel_fill`             | `palette.background` at `bg_opacity` / white          |
/// | `window_stroke`          | `stroke_width` + `palette.foreground` (border colour) |
/// | `window_corner_radius`   | `gui_theme.corner_radius`                             |
/// | `menu_corner_radius`     | `gui_theme.menu_corner_radius`                        |
/// | `selection.bg_fill`      | `palette.selection_bg`                                |
/// | `selection.stroke`       | 1 px + `palette.selection_fg`                         |
/// | `override_text_color`    | `Some(palette.foreground)`                            |
/// | `widgets.*`              | see module-level doc table                            |
///
/// # Determinism
///
/// This function is deterministic: the same inputs always produce the same
/// output.  It does not call `Visuals::dark()` / `Visuals::light()` as a
/// base — it constructs `Visuals` fully from inputs so behaviour is never
/// inherited from egui defaults.
#[must_use]
pub fn build_visuals(
    gui_theme: &GuiTheme,
    palette: &ThemePalette,
    bg_opacity: f32,
    normal_display: bool,
) -> egui::Visuals {
    // ── Fill colors ─────────────────────────────────────────────────────────

    let (window_fill, panel_fill) = if normal_display {
        // Normal display: derive both fills from the palette background.
        // window_fill is fully opaque (chrome must be readable).
        // panel_fill respects bg_opacity so the terminal area can be
        // semi-transparent.
        let wf = internal_color_to_egui_with_alpha(
            TerminalColor::DefaultBackground,
            false,
            palette,
            1.0,
        );
        let pf = internal_color_to_egui_with_alpha(
            TerminalColor::DefaultBackground,
            false,
            palette,
            bg_opacity,
        );
        (wf, pf)
    } else {
        // Reverse-video / alternate-screen invert: force white fills.
        // This is the only hard-coded color in the module (intentional —
        // white is the semantic "inverted background" for reverse video).
        let alpha = (bg_opacity.clamp(0.0, 1.0) * 255.0)
            .approx_as::<u8>()
            .unwrap_or(255_u8);
        let wf = Color32::from_rgba_unmultiplied(255, 255, 255, 255);
        let pf = Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
        (wf, pf)
    };

    // ── Derived scalar types ─────────────────────────────────────────────────

    let window_cr = CornerRadius::same(gui_theme.corner_radius);
    let menu_cr = CornerRadius::same(gui_theme.menu_corner_radius);

    // Window border: foreground color with stroke_width.
    // Using the palette foreground gives a readable, theme-consistent border
    // that stands out against the background without being jarring.
    let fg = rgb_to_color32(palette.foreground);
    let window_stroke = Stroke::new(gui_theme.stroke_width, fg);

    // ── Selection ────────────────────────────────────────────────────────────

    let selection = Selection {
        bg_fill: rgb_to_color32(palette.selection_bg),
        stroke: Stroke::new(1.0, rgb_to_color32(palette.selection_fg)),
    };

    // ── Widgets (all five states) ─────────────────────────────────────────────

    let widgets = Widgets {
        noninteractive: widget(palette, gui_theme, WidgetRole::NonInteractive),
        inactive: widget(palette, gui_theme, WidgetRole::Inactive),
        hovered: widget(palette, gui_theme, WidgetRole::Hovered),
        active: widget(palette, gui_theme, WidgetRole::Active),
        open: widget(palette, gui_theme, WidgetRole::Open),
    };

    // ── Assemble Visuals ─────────────────────────────────────────────────────
    //
    // We start from `Visuals::dark()` to inherit any fields we do not
    // explicitly care about (e.g. `text_cursor`, `clip_rect_margin`,
    // `image_loading_spinners`, etc.), then override every field that
    // `build_visuals` is responsible for.  All palette/geometry-derived
    // fields are set explicitly so the mapping is deterministic regardless
    // of what egui's defaults produce.

    let mut visuals = egui::Visuals::dark();

    visuals.widgets = widgets;
    visuals.window_fill = window_fill;
    visuals.panel_fill = panel_fill;
    visuals.window_stroke = window_stroke;
    visuals.window_corner_radius = window_cr;
    visuals.menu_corner_radius = menu_cr;
    visuals.selection = selection;
    visuals.override_text_color = Some(rgb_to_color32(palette.foreground));

    visuals
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use freminal_common::gui_theme::StyleProfile;
    use freminal_common::themes;

    /// Use Catppuccin Mocha as the test palette — a dark theme with clearly
    /// non-white background and distinct foreground/selection colors, so
    /// assertions about "not white" and alpha hold unambiguously.
    const PALETTE: &ThemePalette = &themes::CATPPUCCIN_MOCHA;

    // ── Corner radius propagation ────────────────────────────────────────────

    #[test]
    fn retro_produces_zero_corner_radius() {
        let gt = StyleProfile::Retro.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.window_corner_radius,
            CornerRadius::same(0),
            "Retro window_corner_radius must be CornerRadius::same(0)"
        );
        assert_eq!(
            v.widgets.noninteractive.corner_radius,
            CornerRadius::same(0),
            "Retro noninteractive corner_radius must be zero"
        );
        assert_eq!(
            v.widgets.inactive.corner_radius,
            CornerRadius::same(0),
            "Retro inactive corner_radius must be zero"
        );
        assert_eq!(
            v.widgets.hovered.corner_radius,
            CornerRadius::same(0),
            "Retro hovered corner_radius must be zero"
        );
        assert_eq!(
            v.widgets.active.corner_radius,
            CornerRadius::same(0),
            "Retro active corner_radius must be zero"
        );
        assert_eq!(
            v.widgets.open.corner_radius,
            CornerRadius::same(0),
            "Retro open corner_radius must be zero"
        );
    }

    #[test]
    fn modern_produces_nonzero_corner_radius() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_ne!(
            v.window_corner_radius,
            CornerRadius::same(0),
            "Modern window_corner_radius must be nonzero"
        );
        // Spot-check widget states too.
        assert_ne!(
            v.widgets.inactive.corner_radius,
            CornerRadius::same(0),
            "Modern inactive corner_radius must be nonzero"
        );
        assert_ne!(
            v.widgets.hovered.corner_radius,
            CornerRadius::same(0),
            "Modern hovered corner_radius must be nonzero"
        );
    }

    // ── Selection derivation ─────────────────────────────────────────────────

    #[test]
    fn selection_bg_fill_matches_palette() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.selection.bg_fill,
            rgb_to_color32(PALETTE.selection_bg),
            "selection.bg_fill must equal rgb_to_color32(palette.selection_bg)"
        );
    }

    // ── Override text color ─────────────────────────────────────────────────

    #[test]
    fn override_text_color_is_foreground() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.override_text_color,
            Some(rgb_to_color32(PALETTE.foreground)),
            "override_text_color must be Some(palette.foreground)"
        );
    }

    // ── Reverse-video fill ───────────────────────────────────────────────────

    #[test]
    fn reverse_video_forces_white_window_fill() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, false);

        assert_eq!(
            v.window_fill,
            Color32::from_rgba_unmultiplied(255, 255, 255, 255),
            "Reverse-video window_fill must be solid white"
        );
    }

    // ── Normal-display fill ─────────────────────────────────────────────────

    #[test]
    fn normal_display_window_fill_is_opaque_and_palette_derived() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        // Alpha must be 255 (fully opaque).
        assert_eq!(
            v.window_fill.a(),
            255,
            "Normal-display window_fill must be fully opaque"
        );

        // RGB must match palette.background (Catppuccin Mocha background is
        // not pure white, so this assertion distinguishes it from the reverse
        // video path).
        let expected = rgb_to_color32(PALETTE.background);
        assert_eq!(
            v.window_fill.r(),
            expected.r(),
            "window_fill R must match palette background"
        );
        assert_eq!(
            v.window_fill.g(),
            expected.g(),
            "window_fill G must match palette background"
        );
        assert_eq!(
            v.window_fill.b(),
            expected.b(),
            "window_fill B must match palette background"
        );
    }

    // ── bg_opacity reflected in panel_fill alpha ────────────────────────────

    #[test]
    fn bg_opacity_half_reflected_in_panel_fill() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 0.5, true);

        // 0.5 * 255.0 = 127.5 → approx_as::<u8> truncates to 127.
        // Allow ±1 for rounding differences.
        let a = v.panel_fill.a();
        assert!(
            (127_i32 - i32::from(a)).abs() <= 1,
            "panel_fill alpha at bg_opacity=0.5 must be ~127, got {a}"
        );
    }

    #[test]
    fn bg_opacity_full_makes_panel_fill_opaque() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.panel_fill.a(),
            255,
            "panel_fill must be fully opaque at bg_opacity=1.0"
        );
    }

    // ── Hover expansion (interactive states only) ────────────────────────────

    #[test]
    fn modern_interactive_states_have_nonzero_expansion() {
        let gt = StyleProfile::Modern.defaults();
        assert!(
            gt.widget_hover_expansion > 0.0,
            "Modern widget_hover_expansion must be nonzero (pre-condition)"
        );
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.widgets.hovered.expansion, gt.widget_hover_expansion,
            "hovered.expansion must equal widget_hover_expansion"
        );
        assert_eq!(
            v.widgets.active.expansion, gt.widget_hover_expansion,
            "active.expansion must equal widget_hover_expansion"
        );
        assert_eq!(
            v.widgets.open.expansion, gt.widget_hover_expansion,
            "open.expansion must equal widget_hover_expansion"
        );
    }

    #[test]
    fn noninteractive_and_inactive_have_zero_expansion() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.widgets.noninteractive.expansion, 0.0,
            "noninteractive.expansion must be 0.0"
        );
        assert_eq!(
            v.widgets.inactive.expansion, 0.0,
            "inactive.expansion must be 0.0"
        );
    }

    #[test]
    fn retro_all_states_have_zero_expansion() {
        let gt = StyleProfile::Retro.defaults();
        // Retro widget_hover_expansion is 0.0, so all states must be 0.0.
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(v.widgets.noninteractive.expansion, 0.0);
        assert_eq!(v.widgets.inactive.expansion, 0.0);
        assert_eq!(v.widgets.hovered.expansion, 0.0);
        assert_eq!(v.widgets.active.expansion, 0.0);
        assert_eq!(v.widgets.open.expansion, 0.0);
    }

    // ── menu_corner_radius ───────────────────────────────────────────────────

    #[test]
    fn menu_corner_radius_matches_gui_theme() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        assert_eq!(
            v.menu_corner_radius,
            CornerRadius::same(gt.menu_corner_radius),
            "menu_corner_radius must match gui_theme.menu_corner_radius"
        );
    }

    // ── Reverse-video panel_fill alpha ───────────────────────────────────────

    #[test]
    fn reverse_video_panel_fill_alpha_reflects_opacity() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 0.5, false);

        // In reverse-video mode panel_fill is white with opacity applied.
        // 0.5 * 255 ≈ 127; allow ±1 for rounding.
        let a = v.panel_fill.a();
        assert!(
            (127_i32 - i32::from(a)).abs() <= 1,
            "Reverse-video panel_fill alpha at bg_opacity=0.5 must be ~127, got {a}"
        );
        // RGB must be pure white — check via to_srgba_unmultiplied() because
        // Color32 stores premultiplied alpha internally, so .r()/.g()/.b()
        // return the premultiplied values (≈127 at 50% opacity, not 255).
        let [red, green, blue, _] = v.panel_fill.to_srgba_unmultiplied();
        assert_eq!(red, 255, "panel_fill R (unmultiplied) must be 255");
        assert_eq!(green, 255, "panel_fill G (unmultiplied) must be 255");
        assert_eq!(blue, 255, "panel_fill B (unmultiplied) must be 255");
    }
}
