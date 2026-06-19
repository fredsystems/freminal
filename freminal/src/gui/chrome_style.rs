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
use freminal_common::themes::{ChromeRole, ThemePalette};

use super::colors::{internal_color_to_egui_with_alpha, rgb_to_color32};

// ---------------------------------------------------------------------------
//  Color derivation helpers
// ---------------------------------------------------------------------------

/// Perceptual luminance of an `(r, g, b)` color in `[0.0, 1.0]`.
///
/// Uses the standard Rec. 601 luma coefficients.  Used to decide whether a
/// palette reads as "dark" or "light" (which flips `Visuals::dark_mode` and
/// the direction surface colors are nudged).
fn luminance(rgb: (u8, u8, u8)) -> f32 {
    let r = f32::from(rgb.0) / 255.0;
    let g = f32::from(rgb.1) / 255.0;
    let b = f32::from(rgb.2) / 255.0;
    0.299_f32.mul_add(r, 0.587_f32.mul_add(g, 0.114 * b))
}

/// Linearly blend two colors by `t` in `[0.0, 1.0]` (`0.0` = `a`, `1.0` = `b`).
fn blend(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| -> u8 {
        let xf = f32::from(x);
        let yf = f32::from(y);
        (yf - xf)
            .mul_add(t, xf)
            .round()
            .approx_as::<u8>()
            .unwrap_or(x)
    };
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Nudge `base` toward black (dark palettes) or white (light palettes) by `t`.
///
/// Produces a surface that contrasts the base background in the
/// palette-appropriate direction: on a dark theme a `TextEdit` well should be
/// *darker* than the panel; on a light theme it should be *lighter*.
fn contrast_surface(base: (u8, u8, u8), t: f32, is_dark: bool) -> (u8, u8, u8) {
    let target = if is_dark { (0, 0, 0) } else { (255, 255, 255) };
    blend(base, target, t)
}

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

    // All chrome colors come from resolved theme roles (authored upstream for
    // vetted themes, best-fit/contrast-derived for raw palettes). No ANSI-slot
    // guessing — that produced invisible borders and low-contrast tabs.
    let border = rgb_to_color32(palette.chrome_role(ChromeRole::Border));

    // The fill for each state and the text drawn on it. The
    // active/open/pressed state uses the saturated Accent fill with the
    // theme's authored OnAccent text (a dedicated, designed pair that pops and
    // is legible by construction). Other states derive legible text via the
    // WCAG-AA `chrome_text_on` picker against their own fill.
    let (fill_rgb, text_rgb, expansion) = match role {
        WidgetRole::NonInteractive => {
            let f = palette.chrome_role(ChromeRole::Surface);
            (f, palette.chrome_text_on(f), 0.0)
        }
        WidgetRole::Inactive => {
            let f = palette.chrome_role(ChromeRole::SurfaceVariant);
            (f, palette.chrome_text_on(f), 0.0)
        }
        WidgetRole::Hovered => {
            let f = palette.chrome_role(ChromeRole::SurfaceHover);
            (f, palette.chrome_text_on(f), exp)
        }
        // Active (pressed) and Open (combo/menu expanded) use the accent pair.
        WidgetRole::Active | WidgetRole::Open => (
            palette.chrome_role(ChromeRole::Accent),
            palette.chrome_role(ChromeRole::OnAccent),
            exp,
        ),
    };

    let fill = rgb_to_color32(fill_rgb);
    let text = rgb_to_color32(text_rgb);

    WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(sw, border),
        corner_radius: cr,
        fg_stroke: Stroke::new(sw, text),
        expansion,
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
/// | `window_stroke`          | `stroke_width` + `ChromeRole::Border`                 |
/// | `window_corner_radius`   | `gui_theme.corner_radius`                             |
/// | `menu_corner_radius`     | `gui_theme.menu_corner_radius`                        |
/// | `selection.bg_fill`      | `ChromeRole::Accent` (selected tab / dropdown item)   |
/// | `selection.stroke`       | 1 px + `ChromeRole::OnAccent` (selected text)         |
/// | `override_text_color`    | `None` (would nullify per-state/selection text)       |
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

    // Window border: the resolved chrome Border role, guaranteed to contrast
    // the surface on every theme (no more invisible borders).
    let border = rgb_to_color32(palette.chrome_role(ChromeRole::Border));
    let window_stroke = Stroke::new(gui_theme.stroke_width, border);

    // ── Selection ────────────────────────────────────────────────────────────
    //
    // CRITICAL: egui draws the *selected* state of `SelectableValue` /
    // `selectable_label` (the active tab) and the selected item in a combo-box
    // dropdown from `visuals.selection.bg_fill` + `visuals.selection.stroke` —
    // NOT from `widgets.active`. So the active/selected element must be driven
    // here. We use the dedicated Accent (saturated fill) + OnAccent (legible
    // text) pair so the selected element pops and its text is readable, rather
    // than the muted terminal `selection_bg` (which left active tabs as a
    // washed-out grey with illegible text).
    let accent = rgb_to_color32(palette.chrome_role(ChromeRole::Accent));
    let on_accent = rgb_to_color32(palette.chrome_role(ChromeRole::OnAccent));
    let selection = Selection {
        bg_fill: accent,
        stroke: Stroke::new(1.0, on_accent),
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

    let is_dark = luminance(palette.background) < 0.5;

    let mut visuals = if is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.dark_mode = is_dark;

    visuals.widgets = widgets;
    visuals.window_fill = window_fill;
    visuals.panel_fill = panel_fill;
    visuals.window_stroke = window_stroke;
    visuals.window_corner_radius = window_cr;
    visuals.menu_corner_radius = menu_cr;
    visuals.selection = selection;
    // Deliberately DO NOT set `override_text_color`. egui bakes that color
    // into every glyph at layout time, which silently nullifies all per-state
    // text colors — including `selection.stroke.color` (selected-tab/dropdown
    // text) and every `widgets.*.fg_stroke`. Leaving it `None` lets each
    // widget state resolve its own text color: plain labels use
    // `widgets.noninteractive.fg_stroke` (chrome text, set via `widget()`),
    // and selected elements use `selection.stroke` (OnAccent, set above).
    visuals.override_text_color = None;

    // ── Surface backgrounds ───────────────────────────────────────────────────
    //
    // These egui surfaces default to near-black in dark mode; left unset they
    // make TextEdit wells, scrollbar tracks, grid stripes, and inline-code
    // spans render black regardless of the palette.  Derive them from the
    // resolved chrome roles so they follow the theme's authored UI ladder.
    //
    // `extreme_bg_color` backs TextEdit/scrollbar/progress tracks — use the
    // input/well fill role (surface_variant).
    visuals.extreme_bg_color = rgb_to_color32(palette.chrome_role(ChromeRole::SurfaceVariant));
    // `faint_bg_color` backs striped-grid alternate rows — the chrome surface.
    visuals.faint_bg_color = rgb_to_color32(palette.chrome_role(ChromeRole::Surface));
    // `code_bg_color` backs inline `code` spans — the hover surface reads as a
    // slightly raised inline block.
    visuals.code_bg_color = rgb_to_color32(palette.chrome_role(ChromeRole::SurfaceHover));

    // ── Semantic foreground colors (previously hard-coded orange/red/blue) ────
    //
    // Map to the palette's ANSI slots so warnings/errors/links match the theme.
    visuals.warn_fg_color = rgb_to_color32(palette.ansi[3]); // yellow
    visuals.error_fg_color = rgb_to_color32(palette.ansi[1]); // red
    visuals.hyperlink_color = rgb_to_color32(palette.ansi[4]); // blue

    // ── Shadows (window + popup/menu) ─────────────────────────────────────────
    //
    // Keep egui's shadow geometry (offset/blur/spread) but tint the color from
    // the palette so drop shadows read correctly on both light and dark themes.
    // Menus, combo-box dropdowns, and context menus all use `popup_shadow`.
    let shadow_color = rgb_to_color32(contrast_surface(palette.background, 0.6, is_dark));
    let shadow_tint =
        Color32::from_rgba_unmultiplied(shadow_color.r(), shadow_color.g(), shadow_color.b(), 96);
    visuals.window_shadow.color = shadow_tint;
    visuals.popup_shadow.color = shadow_tint;

    visuals
}

/// Apply the chrome [`GuiTheme`] padding to an egui [`Style`]'s [`Spacing`].
///
/// `Spacing` lives on [`egui::Style`], not [`egui::Visuals`], so padding
/// cannot be returned from [`build_visuals`]; this sibling is applied
/// alongside `set_visuals` (the per-frame style hook in 112.4, and the gallery
/// example). It bumps button/menu/window/item padding so chrome does not feel
/// cramped — Modern is more generous than Retro per the profile defaults.
///
/// `TextEdit` inner padding has no global `Spacing` field in egui; callers
/// that want roomier inputs pass [`text_edit_margin`] to `TextEdit::margin`.
pub fn apply_chrome_spacing(style: &mut egui::Style, gui_theme: &GuiTheme) {
    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(gui_theme.item_spacing.0, gui_theme.item_spacing.1);
    s.button_padding = egui::vec2(gui_theme.button_padding.0, gui_theme.button_padding.1);
    s.menu_margin = egui::Margin::same(i8_from_u8(gui_theme.menu_padding));
    s.window_margin = egui::Margin::same(f32_to_i8(gui_theme.window_padding));
}

/// The [`egui::Margin`] to apply to a `TextEdit` for the theme's input
/// padding (egui has no global spacing field for text-input inner padding).
#[must_use]
pub fn text_edit_margin(gui_theme: &GuiTheme) -> egui::Margin {
    egui::Margin::symmetric(
        f32_to_i8(gui_theme.text_edit_padding.0),
        f32_to_i8(gui_theme.text_edit_padding.1),
    )
}

/// Convert a `u8` padding value to the `i8` egui `Margin` expects, saturating
/// at the `i8` ceiling (padding never legitimately exceeds 127px).
fn i8_from_u8(v: u8) -> i8 {
    v.value_as::<i8>().unwrap_or(i8::MAX)
}

/// Convert an `f32` padding value to the `i8` egui `Margin` expects, clamped
/// to the non-negative `i8` range.
fn f32_to_i8(v: f32) -> i8 {
    v.round()
        .clamp(0.0, f32::from(i8::MAX))
        .approx_as::<i8>()
        .unwrap_or(0)
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

    // ── Override text color ─────────────────────────────────────────────────

    #[test]
    fn override_text_color_is_none() {
        // Must be None: a global override bakes one color into every glyph and
        // nullifies selection/per-state text colors (the active-tab bug).
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        assert_eq!(
            v.override_text_color, None,
            "override_text_color must be None so per-state text colors apply"
        );
    }

    #[test]
    fn selection_uses_accent_and_on_accent() {
        // The selected tab / dropdown item reads selection.bg_fill +
        // selection.stroke; these must be the Accent / OnAccent pair so the
        // selected element is saturated and its text legible.
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        assert_eq!(
            v.selection.bg_fill,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::Accent)),
            "selection bg must be the Accent role"
        );
        assert_eq!(
            v.selection.stroke.color,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::OnAccent)),
            "selection stroke (selected text) must be the OnAccent role"
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

    // ── Surface backgrounds derive from the palette ──────────────────────────

    #[test]
    fn surface_backgrounds_are_not_egui_dark_defaults() {
        // On a non-black palette, extreme/faint/code backgrounds must be
        // palette-derived, not egui's hard-coded near-black dark defaults.
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        let dark_default = egui::Visuals::dark();
        assert_ne!(
            v.extreme_bg_color, dark_default.extreme_bg_color,
            "extreme_bg_color must be palette-derived, not the egui dark default"
        );
        assert_ne!(
            v.faint_bg_color, dark_default.faint_bg_color,
            "faint_bg_color must be palette-derived, not the egui dark default"
        );
        assert_ne!(
            v.code_bg_color, dark_default.code_bg_color,
            "code_bg_color must be palette-derived, not the egui dark default"
        );
    }

    #[test]
    fn semantic_fg_colors_map_to_palette_ansi() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.warn_fg_color,
            rgb_to_color32(PALETTE.ansi[3]),
            "warn_fg_color must map to ANSI yellow (ansi[3])"
        );
        assert_eq!(
            v.error_fg_color,
            rgb_to_color32(PALETTE.ansi[1]),
            "error_fg_color must map to ANSI red (ansi[1])"
        );
        assert_eq!(
            v.hyperlink_color,
            rgb_to_color32(PALETTE.ansi[4]),
            "hyperlink_color must map to ANSI blue (ansi[4])"
        );
    }

    #[test]
    fn popup_shadow_is_tinted_from_palette() {
        // Menus / combo dropdowns / context menus all use popup_shadow; its
        // color must be palette-tinted (semi-transparent), not the egui default.
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);

        assert_eq!(
            v.popup_shadow.color, v.window_shadow.color,
            "popup and window shadows share the palette-derived tint"
        );
        assert_eq!(
            v.popup_shadow.color.a(),
            96,
            "shadow tint alpha must be the palette-derived 96"
        );
    }

    #[test]
    fn light_palette_flips_dark_mode_false() {
        // A light palette must set dark_mode = false so egui's implicit
        // shading is correct.
        let gt = StyleProfile::Modern.defaults();
        let light = build_visuals(&gt, &themes::CATPPUCCIN_LATTE, 1.0, true);
        assert!(
            !light.dark_mode,
            "Catppuccin Latte is a light theme; dark_mode must be false"
        );

        let dark = build_visuals(&gt, &themes::CATPPUCCIN_MOCHA, 1.0, true);
        assert!(
            dark.dark_mode,
            "Catppuccin Mocha is a dark theme; dark_mode must be true"
        );
    }

    // ── Role-sourced colors (112.3e) ─────────────────────────────────────────

    #[test]
    fn window_stroke_uses_resolved_border_role() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        let expected = rgb_to_color32(PALETTE.chrome_role(ChromeRole::Border));
        assert_eq!(
            v.window_stroke.color, expected,
            "window border must come from the resolved Border role"
        );
    }

    #[test]
    fn widget_fills_use_resolved_surface_roles() {
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        assert_eq!(
            v.widgets.inactive.bg_fill,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::SurfaceVariant)),
            "inactive fill = SurfaceVariant"
        );
        assert_eq!(
            v.widgets.hovered.bg_fill,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::SurfaceHover)),
            "hovered fill = SurfaceHover"
        );
        // The active/open state uses the dedicated saturated Accent fill, not
        // the muted SurfaceActive.
        assert_eq!(
            v.widgets.active.bg_fill,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::Accent)),
            "active fill = Accent"
        );
    }

    #[test]
    fn active_tab_uses_accent_and_on_accent() {
        // The active tab/button uses the dedicated Accent + OnAccent pair
        // (a designed, legible-by-construction combo) rather than a derived
        // text on a muted surface.
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        assert_eq!(
            v.widgets.active.bg_fill,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::Accent)),
            "active fill must be the Accent role"
        );
        assert_eq!(
            v.widgets.active.fg_stroke.color,
            rgb_to_color32(PALETTE.chrome_role(ChromeRole::OnAccent)),
            "active text must be the OnAccent role"
        );
    }

    #[test]
    fn no_ansi_slots_borrowed_for_chrome_fills() {
        // Guard against regressing to ANSI-slot guessing: the inactive fill
        // must not equal ansi[0] (the old "dim border"/fill source) unless the
        // theme genuinely resolves there.
        let gt = StyleProfile::Modern.defaults();
        let v = build_visuals(&gt, PALETTE, 1.0, true);
        // Mocha authors a distinct Surface0 variant != ansi[0] (Surface1).
        assert_ne!(
            v.widgets.inactive.bg_fill,
            rgb_to_color32(PALETTE.ansi[0]),
            "inactive fill must come from a role, not ansi[0]"
        );
    }

    // ── Padding (112.3e) ─────────────────────────────────────────────────────

    #[test]
    fn apply_chrome_spacing_bumps_padding() {
        let gt = StyleProfile::Modern.defaults();
        let mut style = egui::Style::default();
        apply_chrome_spacing(&mut style, &gt);
        assert_eq!(
            style.spacing.button_padding,
            egui::vec2(gt.button_padding.0, gt.button_padding.1)
        );
        assert_eq!(
            style.spacing.menu_margin,
            egui::Margin::same(i8_from_u8(gt.menu_padding))
        );
        assert_eq!(
            style.spacing.item_spacing,
            egui::vec2(gt.item_spacing.0, gt.item_spacing.1)
        );
        // Modern padding is more generous than egui's default button_padding.
        assert!(style.spacing.button_padding.x > 4.0);
    }

    #[test]
    fn text_edit_margin_reflects_theme() {
        let gt = StyleProfile::Modern.defaults();
        let m = text_edit_margin(&gt);
        assert_eq!(m, egui::Margin::symmetric(f32_to_i8(8.0), f32_to_i8(5.0)));
    }
}
