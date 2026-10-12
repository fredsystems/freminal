// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Toast measurement, anchoring and layout: turns measured toasts into the
//! GPU-facing pill quads, text runs and hit rects (see [`layout_toasts`]).

use std::time::Instant;

use super::buttons::{
    BUTTON_TOP_GAP_PTS, ToastButtonInput, button_flow, button_row_width, layout_buttons,
    measure_buttons,
};
use super::{ToastAnim, ToastKind, ToastMeasureSizes, ToastPosition, ToastStack};
use crate::gui::WindowId;
use crate::gui::font_manager::FontManager;
use crate::gui::icons::ChromeIcon;
use crate::gui::panes::PaneId;
use crate::gui::renderer::{
    ToastQuad, ToastRenderState, ToastTextMetrics, ToastTextRenderer, ToastTextRun,
};

// ---------------------------------------------------------------------------
//  Owned-renderer layout model (issue #433)
// ---------------------------------------------------------------------------
//
// The types and function below turn a list of toasts (plus their measured
// text and per-toast animation phase) into the GPU-facing draw data for the
// fully-owned toast renderer (`super::renderer::toast_pass` /
// `toast_text_pass`): one `ToastQuad` per pill and a handful of
// `ToastTextRun`s (icon/label/detail) per toast, laid out and animated in
// the *same* pass so the pill and its text can never drift apart. Hit
// rectangles for hover/dismiss are also computed here, in logical points
// (egui's own coordinate space), so [`ToastStack::show`] can hit-test
// pointer input against them without recomputing geometry.
//
// This module is pure and GL-free — no `FontManager`, no `glow::Context` —
// so it is fully unit-testable with synthesized `ToastTextMetrics`. It is
// wired into the frame by [`ToastStack::show`], which measures/shapes text
// on the main thread, calls `layout_toasts`, and hands the resulting
// `ToastQuad`/`ToastTextRun` data to a single `PaintCallback`.

/// Inner padding between a pill's edge and its content, logical points.
const PILL_PAD_X_PTS: f32 = 14.0;
/// Inner padding between a pill's top/bottom edge and its content, logical points.
const PILL_PAD_Y_PTS: f32 = 10.0;
/// Gap between the icon glyph and the label/detail column, logical points.
const ICON_LABEL_GAP_PTS: f32 = 8.0;
/// Gap between the label/detail column and the dismiss button, logical points.
const LABEL_CLOSE_GAP_PTS: f32 = 8.0;
/// Side length of the (square) dismiss button, logical points.
const CLOSE_SIZE_PTS: f32 = 18.0;
/// Vertical gap between successive toasts in the stack, logical points.
const TOAST_STACK_GAP_PTS: f32 = 8.0;
/// Gap between the label line and the detail line, logical points.
const DETAIL_GAP_PTS: f32 = 4.0;
/// Maximum pill width, logical points. Content wider than this clamps.
const MAX_PILL_WIDTH_PTS: f32 = 360.0;
/// Minimum pill height, logical points, regardless of content.
const MIN_PILL_HEIGHT_PTS: f32 = 40.0;
/// Floor applied to the theme's `menu_corner_radius`, logical points.
const MIN_CORNER_RADIUS_PTS: f32 = 8.0;
/// Inset from the window's right edge to the stack's right edge, logical points.
const STACK_RIGHT_INSET_PTS: f32 = 12.0;
/// Inset from the window's top edge to the stack's first toast, logical points.
const STACK_TOP_INSET_PTS: f32 = 44.0;

/// Geometry inputs to [`layout_toasts`], in LOGICAL points (egui's own
/// coordinate space — the same space `ctx.content_rect()` and pane layout
/// rects live in).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ToastGeometry {
    /// The central-panel content rect — the terminal area, i.e. the window
    /// *minus* the menu/tab chrome (it is
    /// `win.published.cached_central_rect()`). This is deliberately not the
    /// full OS-window rect: toasts anchor within the
    /// content area so they never overlap chrome, and a future
    /// [`ToastPosition::WindowCentered`] caller centers within this content
    /// rect, not the whole window.
    pub content_rect: egui::Rect,
    /// The currently-active pane's rect, if known. Consulted only by
    /// [`ToastPosition::PaneCentered`].
    pub active_pane_rect: Option<egui::Rect>,
}

/// Per-toast input to [`layout_toasts`]: its measured content plus its
/// current animation phase.
///
/// Carries the exact text (`label_text`/`detail_text`, needed to build the
/// actual [`ToastTextRun`]s), the physical pixel size each was measured at
/// (`label_size_px`/`detail_size_px`/`icon_size_px` — needed so the run
/// handed to [`ToastTextRenderer::build_instances`] later
/// rasterizes at *exactly* the size [`ToastTextMetrics`] was measured at;
/// any mismatch would desync the pill's width from the glyphs actually
/// drawn), and each string's pre-measured [`ToastTextMetrics`] (needed for
/// layout math). The caller measures text exactly once (via
/// [`ToastTextRenderer::measure`]) and this function never
/// re-shapes it.
#[derive(Debug, Clone)]
pub(super) struct ToastLayoutInput {
    /// Stable id, matching [`Toast::id`](super::Toast::id), used to key the returned outputs
    /// back to their source toast (e.g. for hit-testing dismiss clicks).
    pub id: u64,
    /// The toast's severity — drives color and icon.
    pub kind: ToastKind,
    /// The exact `"Kind: title"` label string that was measured — already
    /// whitespace-normalized and, if it would otherwise overflow the pill,
    /// truncated with a trailing ellipsis (see [`fit_text`]). This is *the*
    /// string handed to the emitted [`ToastTextRun`], so it is guaranteed to
    /// fit the width [`Self::label`] (below) reports.
    pub label_text: String,
    /// The physical pixel size `label_text` was measured/rasterized at.
    pub label_size_px: f32,
    /// Measured `label_text`, in **physical** pixels (see [`ToastTextMetrics`]).
    pub label: ToastTextMetrics,
    /// The exact detail string that was measured — already
    /// whitespace-normalized and fitted the same way as [`Self::label_text`]
    /// (see [`fit_text`]). Empty when `has_detail` is `false`.
    pub detail_text: String,
    /// The physical pixel size `detail_text` was measured/rasterized at.
    pub detail_size_px: f32,
    /// Measured `detail_text`, in physical pixels. Zero-valued when
    /// `has_detail` is `false`.
    pub detail: ToastTextMetrics,
    /// Whether this toast has a detail line to render.
    pub has_detail: bool,
    /// The physical pixel size the icon glyph was measured/rasterized at.
    pub icon_size_px: f32,
    /// Measured icon glyph (a single glyph), in physical pixels.
    pub icon: ToastTextMetrics,
    /// The physical pixel size the dismiss ("x") glyph was
    /// measured/rasterized at.
    pub close_size_px: f32,
    /// Measured dismiss ("x") glyph, in physical pixels.
    pub close: ToastTextMetrics,
    /// This toast's current animation phase (see [`Toast::anim`](super::Toast::anim)).
    pub anim: ToastAnim,
    /// Measured button labels, in order. Empty for every unkeyed toast, and
    /// then the toast's geometry is exactly that of a toast with no buttons.
    pub buttons: Vec<ToastButtonInput>,
}

/// One laid-out toast, ready to hand to the toast/toast-text GPU passes.
///
/// All fields describing GPU draw data (`pill`, `runs`) are in **physical**
/// pixels, viewport-local to the toast overlay's `PaintCallback` rect
/// (origin = top-left of the callback's clip rect) — matching [`ToastQuad`]
/// and [`ToastTextRun`]'s own documented coordinate space. The hit-test
/// rects are in **logical** points (window space) since that is the space
/// egui's own pointer/hover queries operate in.
#[derive(Debug, Clone)]
pub(super) struct ToastLayoutOutput {
    /// Matches the source [`ToastLayoutInput::id`].
    pub id: u64,
    /// The pill background quad.
    pub pill: ToastQuad,
    /// Text runs to draw for this toast: icon, label, and (if present)
    /// detail, in that order.
    pub runs: Vec<ToastTextRun>,
    /// Interactable pill rect, logical points, window space:
    /// `[min_x, min_y, max_x, max_y]`. The whole pill is the click target —
    /// a primary click anywhere within this rect closes the toast (see
    /// [`ToastStack::hit_test`]). For an unkeyed toast the drawn "x" glyph
    /// (in `runs`) is purely a visual affordance; for a keyed toast it is a
    /// separate hit region ([`Self::dismiss_rect_logical`]) that is tested
    /// first and reports `Closed` without `Activated`.
    pub hit_rect_logical: [f32; 4],
    /// The dismiss ("x") hit square, logical points, window space, in the
    /// same `[min_x, min_y, max_x, max_y]` form as
    /// [`Self::hit_rect_logical`]: [`CLOSE_SIZE_PTS`] on a side, centred on
    /// the drawn "x" glyph and clamped to lie within `hit_rect_logical`.
    pub dismiss_rect_logical: [f32; 4],
    /// One quad per button, in label order, to draw immediately after
    /// [`Self::pill`] (physical pixels, same space as the pill).
    pub button_quads: Vec<ToastQuad>,
    /// One hit rect per button, in label order (index _i_ is button _i_ + 1),
    /// logical points, window space, clamped into [`Self::hit_rect_logical`].
    pub button_rects_logical: Vec<[f32; 4]>,
}

/// The maximum corner radius (in logical points) among the theme's
/// `menu_corner_radius` four corners, floored at [`MIN_CORNER_RADIUS_PTS`].
fn corner_radius_pts(visuals: &egui::Visuals) -> f32 {
    let corner = visuals.menu_corner_radius;
    let max_corner = corner.nw.max(corner.ne).max(corner.sw).max(corner.se);
    f32::from(max_corner).max(MIN_CORNER_RADIUS_PTS)
}

/// Convert a straight-alpha [`egui::Color32`] to straight (non-premultiplied)
/// `[r, g, b, a]` floats in `0..=1`.
fn color32_to_rgba(color: egui::Color32) -> [f32; 4] {
    // `Color32`'s accessors (`r()`/`g()`/`b()`) return components
    // *premultiplied* by alpha (see the `ecolor::Color32` type docs) — this
    // pass's `ToastQuad`/`ToastTextRun` colors are documented as *straight*
    // RGBA, so the alpha must be un-premultiplied via `to_srgba_unmultiplied`
    // rather than reading the accessors directly.
    let straight = color.to_srgba_unmultiplied();
    [
        f32::from(straight[0]) / 255.0,
        f32::from(straight[1]) / 255.0,
        f32::from(straight[2]) / 255.0,
        f32::from(straight[3]) / 255.0,
    ]
}

/// Shift each color channel of `rgba` by `amount` (positive lightens,
/// negative darkens), clamped to `0..=1`. Alpha is left untouched.
pub(super) fn lighten(rgba: [f32; 4], amount: f32) -> [f32; 4] {
    [
        (rgba[0] + amount).clamp(0.0, 1.0),
        (rgba[1] + amount).clamp(0.0, 1.0),
        (rgba[2] + amount).clamp(0.0, 1.0),
        rgba[3],
    ]
}

/// Available width, in physical pixels, for the label/detail text portion
/// of a toast pill, given a measured `icon_width`.
///
/// This mirrors the exact non-text geometry [`settled_pill_size`] subtracts
/// from [`MAX_PILL_WIDTH_PTS`] to derive its clamped pill width. It is used
/// by [`ToastStack::measure_inputs`] to fit (truncate + ellipsize, see
/// [`fit_text`]) the label/detail strings *before* they are measured for
/// pill sizing, so `settled_pill_size` and the emitted [`ToastTextRun`]s
/// always agree on how wide the text is — the regression this fixes: the
/// pill width was clamped, but the un-truncated text was still measured
/// (and rendered) past that clamp, all the way to the window edge.
fn text_width_budget(icon_width: f32, ppp: f32) -> f32 {
    let pad_x = PILL_PAD_X_PTS * ppp;
    let icon_label_gap = ICON_LABEL_GAP_PTS * ppp;
    let label_close_gap = LABEL_CLOSE_GAP_PTS * ppp;
    let close_size = CLOSE_SIZE_PTS * ppp;
    let max_pill_width = MAX_PILL_WIDTH_PTS * ppp;
    let non_text_width = pad_x.mul_add(
        2.0,
        icon_width + icon_label_gap + label_close_gap + close_size,
    );
    max_pill_width - non_text_width
}

/// Collapse embedded newlines and other whitespace runs (`\n`, `\r`, `\t`,
/// runs of plain spaces, ...) to a single space, and trim the result.
///
/// Toast text runs are shaped as a single line (see the module's
/// single-face-per-run docs in `renderer::toast_text_pass`), so raw
/// multi-line input — a shader compile log, a layout/session error
/// carrying a multi-line path — must never reach a [`ToastTextRun`] with
/// its newlines intact: they would shape as tofu/invisible glyphs rather
/// than an actual line break.
fn normalize_ws(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fit `raw` for display as a single-line toast text run within
/// `max_width` physical pixels: normalize embedded whitespace (see
/// [`normalize_ws`]), then, if the normalized text still measures wider
/// than `max_width`, truncate it on a char boundary and append a trailing
/// ellipsis ("…") — binary-searching the longest char-prefix that still
/// fits alongside the ellipsis, since width is monotonically
/// non-decreasing in prefix length.
///
/// Returns the (possibly-truncated) string together with its own measured
/// [`ToastTextMetrics`]. Callers MUST use both the returned string (in the
/// emitted [`ToastTextRun`]) and the returned metrics (for pill sizing) —
/// never the original `raw` text — so the pill's width and the text
/// actually drawn can never disagree.
///
/// **Must be called on the main thread** — like [`ToastTextRenderer::measure`]
/// itself, it takes `&mut FontManager`.
pub(super) fn fit_text(
    renderer: &ToastTextRenderer,
    raw: &str,
    size_px: f32,
    max_width: f32,
    font_manager: &mut FontManager,
) -> (String, ToastTextMetrics) {
    let normalized = normalize_ws(raw);
    let metrics = renderer.measure(&normalized, size_px, font_manager);
    if metrics.width <= max_width {
        return (normalized, metrics);
    }

    let ellipsis_metrics = renderer.measure("…", size_px, font_manager);
    if ellipsis_metrics.width > max_width {
        // Nothing shorter to try — even a bare ellipsis doesn't fit. Return
        // it anyway (best effort); the caller floors `max_width` at this
        // same width for the label/detail budget it passes in, so this is
        // only reachable via a mismatched budget (e.g. a different, larger
        // font size than the one `max_width` was floored against).
        return ("…".to_owned(), ellipsis_metrics);
    }

    let chars: Vec<char> = normalized.chars().collect();
    // Binary search the longest char-prefix length `p` in `0..=chars.len()`
    // such that `chars[..p]` plus a trailing ellipsis still fits
    // `max_width`. `lo` always holds a known-fitting length — `0` fits,
    // since the bare ellipsis was just confirmed to fit above — and `best`
    // is kept in lock-step with `lo` so the loop can return it directly.
    let mut lo = 0_usize;
    let mut hi = chars.len();
    let mut best = ("…".to_owned(), ellipsis_metrics);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        let candidate_metrics = renderer.measure(&candidate, size_px, font_manager);
        if candidate_metrics.width <= max_width {
            best = (candidate, candidate_metrics);
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    best
}

/// The settled (pre-animation) pill width/height, in physical pixels, for
/// one toast's measured content.
fn settled_pill_size(input: &ToastLayoutInput, ppp: f32) -> (f32, f32) {
    let pad_x = PILL_PAD_X_PTS * ppp;
    let pad_y = PILL_PAD_Y_PTS * ppp;
    let icon_label_gap = ICON_LABEL_GAP_PTS * ppp;
    let label_close_gap = LABEL_CLOSE_GAP_PTS * ppp;
    let close_size = CLOSE_SIZE_PTS * ppp;
    let detail_gap = DETAIL_GAP_PTS * ppp;
    let max_width = MAX_PILL_WIDTH_PTS * ppp;
    let min_height = MIN_PILL_HEIGHT_PTS * ppp;

    let text_width = input.label.width.max(input.detail.width);
    let content_width =
        input.icon.width + icon_label_gap + text_width + label_close_gap + close_size;
    let width = pad_x.mul_add(2.0, content_width).min(max_width);

    let text_height = text_block_height(input, detail_gap);
    if input.buttons.is_empty() {
        let height = pad_y.mul_add(2.0, text_height).max(min_height);
        return (width, height);
    }

    // Widen to fit the buttons on one row where they fit, up to the maximum
    // pill width; past that the rows wrap (see `button_flow`).
    let inner_max = 2.0_f32.mul_add(-pad_x, max_width);
    let row_width = button_row_width(&input.buttons, ppp).min(inner_max);
    let width = width.max(pad_x.mul_add(2.0, row_width));
    let flow = button_flow(&input.buttons, 2.0_f32.mul_add(-pad_x, width), ppp);
    let content_height = BUTTON_TOP_GAP_PTS.mul_add(ppp, text_height) + flow.height;
    let height = pad_y.mul_add(2.0, content_height).max(min_height);

    (width, height)
}

/// Height, physical pixels, of the label (and optional detail) block.
pub(super) fn text_block_height(input: &ToastLayoutInput, detail_gap: f32) -> f32 {
    let mut text_height = input.label.height;
    if input.has_detail {
        text_height += detail_gap + input.detail.height;
    }
    text_height
}

/// Compute the settled (pre-animation) top-left origin, in physical pixels,
/// of each toast's pill, for the given anchor `position`.
///
/// `sizes[i]` is `(width, height)` for `inputs[i]`, as returned by
/// [`settled_pill_size`]. Returns one `(x, y)` pair per input, in the same
/// order.
fn settled_origins(
    sizes: &[(f32, f32)],
    position: ToastPosition,
    geom: ToastGeometry,
    ppp: f32,
) -> Vec<(f32, f32)> {
    let gap = TOAST_STACK_GAP_PTS * ppp;

    // Cumulative vertical offset of each toast's top edge, relative to the
    // stack's own top (offset 0 for the first toast).
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut cumulative = 0.0;
    for &(_, height) in sizes {
        offsets.push(cumulative);
        cumulative += height + gap;
    }
    let total_height = if sizes.is_empty() {
        0.0
    } else {
        cumulative - gap
    };

    let rect_for = |rect: egui::Rect| {
        (
            rect.min.x * ppp,
            rect.min.y * ppp,
            rect.max.x * ppp,
            rect.max.y * ppp,
        )
    };

    match position {
        ToastPosition::TopRightStack => {
            let (_, win_min_y, win_max_x, _) = rect_for(geom.content_rect);
            let base_y = STACK_TOP_INSET_PTS.mul_add(ppp, win_min_y);
            let right_edge = STACK_RIGHT_INSET_PTS.mul_add(-ppp, win_max_x);
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (right_edge - width, base_y + offset))
                .collect()
        }
        ToastPosition::WindowCentered => {
            let (win_min_x, win_min_y, win_max_x, win_max_y) = rect_for(geom.content_rect);
            let center_x = win_min_x.midpoint(win_max_x);
            let base_y = win_min_y.midpoint(win_max_y) - total_height / 2.0;
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (center_x - width / 2.0, base_y + offset))
                .collect()
        }
        ToastPosition::PaneCentered => {
            let pane_rect = geom.active_pane_rect.unwrap_or(geom.content_rect);
            let (pane_min_x, pane_min_y, pane_max_x, pane_max_y) = rect_for(pane_rect);
            let center_x = pane_min_x.midpoint(pane_max_x);
            let base_y = pane_min_y.midpoint(pane_max_y) - total_height / 2.0;
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (center_x - width / 2.0, base_y + offset))
                .collect()
        }
    }
}

/// Precomputed, `pixels_per_point`-scaled layout constants shared across
/// every toast in one [`layout_toasts`] call. Bundled into one struct so the
/// per-toast helper functions stay under the `too_many_arguments` limit.
pub(super) struct ToastLayoutMetrics {
    /// `pixels_per_point`, kept alongside the already-scaled fields below
    /// since logical-to-physical hit-rect conversion still needs it raw.
    pub(super) ppp: f32,
    /// [`PILL_PAD_X_PTS`], scaled to physical pixels.
    pub(super) pad_x: f32,
    /// [`PILL_PAD_Y_PTS`], scaled to physical pixels.
    pub(super) pad_y: f32,
    /// [`ICON_LABEL_GAP_PTS`], scaled to physical pixels.
    icon_label_gap: f32,
    /// [`CLOSE_SIZE_PTS`], scaled to physical pixels.
    close_size: f32,
    /// [`DETAIL_GAP_PTS`], scaled to physical pixels.
    pub(super) detail_gap: f32,
    /// The theme's corner radius (see [`corner_radius_pts`]), scaled to
    /// physical pixels.
    pub(super) corner_radius: f32,
    /// The neutral chrome border width (from the theme's `window_stroke`,
    /// Modern 1.0 / Retro 1.5 pt), scaled to physical pixels. Issue #433
    /// redesign: replaces the removed glow as the pill's separation from the
    /// terminal, and gives Modern/Retro a subtly different border weight.
    border_width: f32,
}

/// The pill/border/text colors for one toast, derived from the active
/// theme and its current animation opacity.
///
/// Issue #433 visual redesign: the pill fill is a neutral, solid theme
/// color (see [`ToastKind::background`]); the notification kind shows only in
/// the left accent bar and the icon tint. A neutral chrome border replaces
/// the old outer glow for separation from the terminal behind it.
struct ToastColors {
    /// Pill background, top stop (a very subtle vertical gradient is kept for
    /// depth, but both stops are near the neutral solid fill).
    color_top: [f32; 4],
    /// Pill background, bottom stop.
    color_bottom: [f32; 4],
    /// Neutral chrome border color (straight RGBA), from the theme's
    /// `window_stroke`. Alpha 0 disables the border in the shader.
    border_color: [f32; 4],
    /// Left accent-bar color (the notification kind's semantic color).
    accent: [f32; 4],
    /// Label/detail text color, contrast-picked against the pill fill.
    text_color: [f32; 4],
    /// Icon glyph color (tinted to the kind's semantic color).
    icon_color: [f32; 4],
}

/// Derive [`ToastColors`] for `kind` from the active theme, at the given
/// (already-clamped-elsewhere) animation `opacity`.
///
/// Issue #433 visual redesign: the fill is the neutral solid theme
/// background, the border is the neutral `window_stroke` color, and only the
/// accent bar + icon carry the kind's semantic color.
fn toast_colors(kind: ToastKind, visuals: &egui::Visuals, opacity: f32) -> ToastColors {
    let background = ToastKind::background(visuals);
    let base_rgba = color32_to_rgba(background);
    // A whisper of vertical gradient (±3%) keeps the flat pill from looking
    // dead without reintroducing the old jarring saturated bubble.
    let color_top = lighten(base_rgba, 0.03);
    let color_bottom = lighten(base_rgba, -0.03);

    let semantic_rgba = color32_to_rgba(kind.semantic(visuals));
    let accent = [semantic_rgba[0], semantic_rgba[1], semantic_rgba[2], 1.0];

    // Neutral chrome border for separation from the terminal (replaces the
    // removed outer glow). The stroke color is already the theme's border
    // role; its width (Modern 1.0 / Retro 1.5 pt) is applied per-pill by the
    // layout via `metrics.border_width`.
    let border_color = color32_to_rgba(visuals.window_stroke.color);

    let text_alpha = opacity.clamp(0.0, 1.0);
    let text_rgb = if ToastKind::prefers_dark_text(background) {
        [0.08, 0.08, 0.08]
    } else {
        [0.96, 0.96, 0.96]
    };
    let text_color = [text_rgb[0], text_rgb[1], text_rgb[2], text_alpha];
    let icon_color = [
        semantic_rgba[0],
        semantic_rgba[1],
        semantic_rgba[2],
        text_alpha,
    ];

    ToastColors {
        color_top,
        color_bottom,
        border_color,
        accent,
        text_color,
        icon_color,
    }
}

/// Origin `(origin_x, baseline_y)`, physical pixels, of the dismiss ("x")
/// glyph run for a pill at `(x, y)` of `width`. Shared by [`build_text_runs`]
/// (to place the run) and [`layout_one_toast`] (to centre the dismiss hit
/// square on it), so the two can never disagree.
fn close_glyph_origin(
    input: &ToastLayoutInput,
    x: f32,
    y: f32,
    width: f32,
    metrics: &ToastLayoutMetrics,
) -> (f32, f32) {
    let close_right = x + width - metrics.pad_x;
    let close_left = close_right - metrics.close_size;
    let origin_x = (close_left + (metrics.close_size - input.close.width) / 2.0).max(close_left);
    // Share the label row's baseline so the "x" aligns with the title text
    // beside it (top-aligned header row), rather than centering in the full
    // pill height — which visually drops it too low, more so on a
    // two-line (detail) toast.
    let baseline_y = y + metrics.pad_y + input.label.ascent;
    (origin_x, baseline_y)
}

/// Build the icon/label/(optional detail)/dismiss [`ToastTextRun`]s for one
/// toast, anchored to the pill's already-animated top-left corner `(x, y)`
/// and sized `width` x `height` (physical pixels, post-animation scale —
/// the same rect the pill quad itself occupies, so the dismiss glyph never
/// drifts from the pill under the scale-in animation).
fn build_text_runs(
    input: &ToastLayoutInput,
    x: f32,
    y: f32,
    width: f32,
    metrics: &ToastLayoutMetrics,
    colors: &ToastColors,
) -> Vec<ToastTextRun> {
    let icon_origin_x = x + metrics.pad_x;
    let icon_baseline_y = y + metrics.pad_y + input.icon.ascent;
    let mut runs = vec![ToastTextRun {
        text: input.kind.icon().glyph(),
        origin_x: icon_origin_x,
        baseline_y: icon_baseline_y,
        size_px: input.icon_size_px,
        color: colors.icon_color,
    }];

    let label_origin_x = icon_origin_x + input.icon.width + metrics.icon_label_gap;
    let label_baseline_y = y + metrics.pad_y + input.label.ascent;
    runs.push(ToastTextRun {
        text: input.label_text.clone(),
        origin_x: label_origin_x,
        baseline_y: label_baseline_y,
        size_px: input.label_size_px,
        color: colors.text_color,
    });

    if input.has_detail {
        let detail_origin_x = x + metrics.pad_x;
        let detail_baseline_y =
            y + metrics.pad_y + input.label.height + metrics.detail_gap + input.detail.ascent;
        runs.push(ToastTextRun {
            text: input.detail_text.clone(),
            origin_x: detail_origin_x,
            baseline_y: detail_baseline_y,
            size_px: input.detail_size_px,
            color: colors.text_color,
        });
    }

    // Dismiss ("x") glyph, drawn within the reserved close-button region at
    // the pill's right edge (physical pixels, since text runs are physical).
    let (close_origin_x, close_baseline_y) = close_glyph_origin(input, x, y, width, metrics);
    // Dimmed relative to the label/detail text so it reads as a secondary
    // affordance rather than competing with the message; still respects the
    // animation's opacity via `colors.text_color`'s alpha.
    let close_color = [
        colors.text_color[0],
        colors.text_color[1],
        colors.text_color[2],
        colors.text_color[3] * 0.7,
    ];
    runs.push(ToastTextRun {
        text: ChromeIcon::Close.glyph(),
        origin_x: close_origin_x,
        baseline_y: close_baseline_y,
        size_px: input.close_size_px,
        color: close_color,
    });

    runs
}

/// Lay out one toast: apply its animation to the settled pill rect, derive
/// its colors, and build its text runs and hit rects.
fn layout_one_toast(
    input: &ToastLayoutInput,
    width: f32,
    height: f32,
    settled_x: f32,
    settled_y: f32,
    metrics: &ToastLayoutMetrics,
    visuals: &egui::Visuals,
) -> ToastLayoutOutput {
    let anim = input.anim;

    // Scale about the settled rect's center, then apply the horizontal
    // slide (converted from logical points to physical pixels) on top.
    let scaled_width = width * anim.scale;
    let scaled_height = height * anim.scale;
    let center_x = settled_x + width / 2.0;
    let center_y = settled_y + height / 2.0;
    let x = anim
        .slide_x
        .mul_add(metrics.ppp, center_x - scaled_width / 2.0);
    let y = center_y - scaled_height / 2.0;

    let colors = toast_colors(input.kind, visuals, anim.opacity);
    // Clamp the corner radius to half the pill's smaller dimension: a large
    // theme `menu_corner_radius` on a short pill could otherwise exceed the
    // half-extent and distort the SDF into a lozenge (review finding #6).
    let corner_radius = metrics
        .corner_radius
        .min(scaled_width.min(scaled_height) / 2.0)
        .max(0.0);
    // Clamp the border to half the smaller dimension so a chunky border on a
    // tiny (mid-scale-in) pill can't cross the pill's center and invert.
    let border_width = metrics
        .border_width
        .min(scaled_width.min(scaled_height) / 2.0)
        .max(0.0);
    let pill = ToastQuad {
        x,
        y,
        width: scaled_width,
        height: scaled_height,
        corner_radius,
        color_top: colors.color_top,
        color_bottom: colors.color_bottom,
        border_color: colors.border_color,
        border_width,
        accent: colors.accent,
        opacity: anim.opacity.clamp(0.0, 1.0),
    };

    let mut runs = build_text_runs(input, x, y, scaled_width, metrics, &colors);

    let hit_rect_logical = [
        x / metrics.ppp,
        y / metrics.ppp,
        (x + scaled_width) / metrics.ppp,
        (y + scaled_height) / metrics.ppp,
    ];

    let buttons = layout_buttons(
        input,
        [settled_x, settled_y, width, height],
        &pill,
        hit_rect_logical,
        metrics,
        colors.text_color,
    );
    runs.extend(buttons.runs);

    // Dismiss hit square: centred on the drawn "x" glyph (same origin the
    // run was placed at), CLOSE_SIZE_PTS on a side, clamped into the pill's
    // hit rect so a mid-animation (scaled-down) pill never has a dismiss
    // region poking outside it.
    let (close_origin_x, close_baseline_y) = close_glyph_origin(input, x, y, scaled_width, metrics);
    let glyph_center_x = (close_origin_x + input.close.width / 2.0) / metrics.ppp;
    let glyph_center_y =
        (close_baseline_y - input.close.ascent + input.close.height / 2.0) / metrics.ppp;
    let half = CLOSE_SIZE_PTS / 2.0;
    let dismiss_rect_logical = [
        (glyph_center_x - half).max(hit_rect_logical[0]),
        (glyph_center_y - half).max(hit_rect_logical[1]),
        (glyph_center_x + half).min(hit_rect_logical[2]),
        (glyph_center_y + half).min(hit_rect_logical[3]),
    ];

    ToastLayoutOutput {
        id: input.id,
        pill,
        runs,
        hit_rect_logical,
        dismiss_rect_logical,
        button_quads: buttons.quads,
        button_rects_logical: buttons.rects_logical,
    }
}

/// Turn `inputs` into GPU-facing draw data: one [`ToastQuad`] pill and a
/// handful of [`ToastTextRun`]s per toast, plus logical-point hit rects.
///
/// Pure and GL-free (no `FontManager`, no `glow::Context`) — `inputs` already
/// carries pre-measured [`ToastTextMetrics`] and each toast's animation
/// phase, so this function only does geometry and color arithmetic.
pub(super) fn layout_toasts(
    inputs: &[ToastLayoutInput],
    position: ToastPosition,
    geom: ToastGeometry,
    visuals: &egui::Visuals,
    pixels_per_point: f32,
) -> Vec<ToastLayoutOutput> {
    if inputs.is_empty() {
        return Vec::new();
    }

    // Guard against a pathological `pixels_per_point` of 0 from the windowing
    // layer: `ppp` is used as a divisor when deriving the logical hit rect
    // (`layout_one_toast`), so a zero would yield `Inf`/`NaN` rects that never
    // match a pointer. Floor it at a tiny epsilon (review finding #5).
    let ppp = pixels_per_point.max(f32::EPSILON);
    let metrics = ToastLayoutMetrics {
        ppp,
        pad_x: PILL_PAD_X_PTS * ppp,
        pad_y: PILL_PAD_Y_PTS * ppp,
        icon_label_gap: ICON_LABEL_GAP_PTS * ppp,
        close_size: CLOSE_SIZE_PTS * ppp,
        detail_gap: DETAIL_GAP_PTS * ppp,
        corner_radius: corner_radius_pts(visuals) * ppp,
        border_width: visuals.window_stroke.width * ppp,
    };

    let sizes: Vec<(f32, f32)> = inputs.iter().map(|i| settled_pill_size(i, ppp)).collect();
    let origins = settled_origins(&sizes, position, geom, ppp);

    inputs
        .iter()
        .zip(&sizes)
        .zip(&origins)
        .map(|((input, &(width, height)), &(settled_x, settled_y))| {
            layout_one_toast(
                input, width, height, settled_x, settled_y, &metrics, visuals,
            )
        })
        .collect()
}

/// Resolve the position + geometry a toast should actually be laid out at
/// in the window currently rendering it.
///
/// `toast_position`/`origin_pane` describe the toast's own declared anchor
/// and (for [`ToastPosition::PaneCentered`]) its origin pane;
/// `is_origin_window` is `true` iff the toast's origin window (see
/// [`Toast::origin`](super::Toast::origin)) is the window currently rendering. Comparing
/// `WindowId`s themselves is left to the caller
/// ([`ToastStack::layout_positioned`]): `WindowId` only exposes a
/// `winit`-backed constructor, so no second, distinct `WindowId` can be
/// fabricated in a unit test — taking the comparison result as a `bool`
/// here instead keeps this function unit-testable.
///
/// - [`ToastPosition::TopRightStack`] and [`ToastPosition::WindowCentered`]
///   always resolve against `content_rect` (this window's content area);
///   `resolve_pane_rect` is not consulted.
/// - [`ToastPosition::PaneCentered`] resolves the origin pane's rect via
///   `resolve_pane_rect` only when `is_origin_window` is `true` and
///   `origin_pane` is `Some`; if `resolve_pane_rect` returns `None` (the
///   pane is not in this window, e.g. it has since closed) it falls back to
///   [`ToastPosition::WindowCentered`], same as a foreign window or a
///   missing origin.
fn resolve_anchor(
    toast_position: ToastPosition,
    is_origin_window: bool,
    origin_pane: Option<PaneId>,
    content_rect: egui::Rect,
    resolve_pane_rect: &impl Fn(PaneId) -> Option<egui::Rect>,
) -> (ToastPosition, ToastGeometry) {
    let window_geom = ToastGeometry {
        content_rect,
        active_pane_rect: None,
    };
    match toast_position {
        ToastPosition::TopRightStack => (ToastPosition::TopRightStack, window_geom),
        ToastPosition::WindowCentered => (ToastPosition::WindowCentered, window_geom),
        ToastPosition::PaneCentered => {
            if is_origin_window
                && let Some(pane) = origin_pane
                && let Some(pane_rect) = resolve_pane_rect(pane)
            {
                (
                    ToastPosition::PaneCentered,
                    ToastGeometry {
                        content_rect,
                        active_pane_rect: Some(pane_rect),
                    },
                )
            } else {
                (ToastPosition::WindowCentered, window_geom)
            }
        }
    }
}

/// The concrete rect a resolved anchor lays its stack out against, or `None`
/// for the top-right stack (which anchors to a corner, not a center).
///
/// This is the *grouping identity* used by [`group_and_layout`]: two toasts
/// belong in the same layout group iff their `layout_key`s are equal. It
/// deliberately collapses [`ToastPosition::WindowCentered`] and
/// [`ToastPosition::PaneCentered`] onto the *same* key when they center on
/// the *same* rect — the common single-pane case, where a pane's rect equals
/// the window content rect. Without this collapse the two would form separate
/// groups laid out independently and render exactly on top of each other,
/// occluding (and, since `topmost_hit` picks the newest at a shared rect,
/// making un-dismissable) the older toast until it expires.
fn layout_key(position: ToastPosition, geom: ToastGeometry) -> Option<egui::Rect> {
    match position {
        ToastPosition::TopRightStack => None,
        ToastPosition::WindowCentered => Some(geom.content_rect),
        ToastPosition::PaneCentered => Some(geom.active_pane_rect.unwrap_or(geom.content_rect)),
    }
}

/// The canonical `(position, geometry)` to feed [`layout_toasts`] for a group
/// identified by `key` (see [`layout_key`]). A `None` key is the top-right
/// stack; a `Some(rect)` key is a centered stack, always laid out as
/// [`ToastPosition::WindowCentered`] over that rect (both centered variants
/// share identical centering math in [`settled_origins`], so collapsing them
/// onto one representative keeps coincident centered groups stacked rather
/// than overlapping).
const fn canonical_anchor(
    key: Option<egui::Rect>,
    content_rect: egui::Rect,
) -> (ToastPosition, ToastGeometry) {
    // A centered key lays out as `WindowCentered` over the keyed rect (both
    // centered variants share identical centering math); a `None` key is the
    // top-right stack over `content_rect`.
    if let Some(rect) = key {
        (
            ToastPosition::WindowCentered,
            ToastGeometry {
                content_rect: rect,
                active_pane_rect: None,
            },
        )
    } else {
        (
            ToastPosition::TopRightStack,
            ToastGeometry {
                content_rect,
                active_pane_rect: None,
            },
        )
    }
}

/// Group `inputs` by the effective anchor each was resolved to (`anchors[i]`
/// for `inputs[i]`, same length, same order — see [`resolve_anchor`]), run
/// [`layout_toasts`] once per group of toasts sharing an anchor (so they
/// still stack against each other), then scatter every group's outputs back
/// into one `Vec<ToastLayoutOutput>` in the SAME order as `inputs`.
///
/// Grouping identity is [`layout_key`], NOT the raw `(position, geometry)`:
/// two centered toasts that resolve to the same rect (e.g. a pane-centered
/// toast in a single-pane window, whose pane rect equals the window content
/// rect, and a window-centered toast) share one group and stack, rather than
/// forming two independent groups that render on top of each other.
///
/// Preserving input order is required: [`ToastStack::hit_test`] and
/// `topmost_hit` rely on `outputs[i]` corresponding to the toast `inputs[i]`
/// was built from, and on "newest last in the vec" meaning "drawn on top" —
/// grouping toasts by anchor must never reorder them relative to each
/// other.
///
/// Deliberately takes no [`Toast`](super::Toast)/[`WindowId`] — only the already-resolved
/// `anchors` — so this (unlike [`ToastStack::layout_positioned`], its only
/// caller) is unit-testable without fabricating a `WindowId`.
///
/// Groups are found via a linear scan rather than a hash map:
/// [`layout_key`] carries `f32` rects, which must never be used as a hash
/// key, and the toast count is capped at [`MAX_TOASTS`](super::MAX_TOASTS) (5), so a linear
/// scan stays cheap.
fn group_and_layout(
    inputs: &[ToastLayoutInput],
    anchors: &[(ToastPosition, ToastGeometry)],
    visuals: &egui::Visuals,
    ppp: f32,
) -> Vec<ToastLayoutOutput> {
    debug_assert_eq!(inputs.len(), anchors.len());

    // Bucket each input (by its original index) into the group of toasts
    // sharing the same layout key, preserving first-seen group order. The
    // key collapses coincident centered anchors (see `layout_key`); the
    // per-group `content_rect` is retained so `canonical_anchor` can rebuild
    // the top-right stack's geometry when the key is `None`.
    let mut groups: Vec<(Option<egui::Rect>, egui::Rect, Vec<usize>)> = Vec::new();
    for (idx, &(position, geom)) in anchors.iter().enumerate() {
        let key = layout_key(position, geom);
        if let Some(group) = groups.iter_mut().find(|(gk, _, _)| *gk == key) {
            group.2.push(idx);
        } else {
            groups.push((key, geom.content_rect, vec![idx]));
        }
    }

    // Lay out each group independently (each stacks internally, per its own
    // anchor), then scatter the results back into `inputs` order.
    let mut outputs: Vec<Option<ToastLayoutOutput>> = inputs.iter().map(|_| None).collect();
    for (key, content_rect, indices) in groups {
        let (position, geom) = canonical_anchor(key, content_rect);
        let group_inputs: Vec<ToastLayoutInput> =
            indices.iter().map(|&i| inputs[i].clone()).collect();
        let group_outputs = layout_toasts(&group_inputs, position, geom, visuals, ppp);
        for (i, out) in indices.into_iter().zip(group_outputs) {
            outputs[i] = Some(out);
        }
    }

    // Every original index was placed into exactly one group above, so
    // every slot is `Some` here — `flatten` just unwraps them. The
    // debug-assert keeps a future invariant break (a group whose
    // `layout_toasts` returns fewer outputs than its indices) loud rather
    // than silently shortening `outputs` and desyncing the
    // `outputs[i] <-> entries[i]` mapping `hit_test`/`topmost_hit` rely on.
    let laid_out: Vec<ToastLayoutOutput> = outputs.into_iter().flatten().collect();
    debug_assert_eq!(laid_out.len(), inputs.len());
    laid_out
}

impl ToastStack {
    /// Resolve every entry's effective anchor in `this_window_id` (via
    /// [`resolve_anchor`]) and delegate to [`group_and_layout`] to actually
    /// lay them out.
    ///
    /// This method itself takes a real [`WindowId`], which — unlike
    /// [`resolve_anchor`] and [`group_and_layout`] — cannot be fabricated in
    /// a unit test (`WindowId` only exposes a `winit`-backed constructor,
    /// private to `freminal-windowing`). It is therefore intentionally thin:
    /// all its actual decision logic (anchor resolution, grouping, scatter)
    /// lives in those two pure, directly-tested functions, leaving nothing
    /// non-trivial un-exercised here.
    pub(super) fn layout_positioned(
        &self,
        inputs: &[ToastLayoutInput],
        this_window_id: WindowId,
        content_rect: egui::Rect,
        resolve_pane_rect: &impl Fn(PaneId) -> Option<egui::Rect>,
        visuals: &egui::Visuals,
        ppp: f32,
    ) -> Vec<ToastLayoutOutput> {
        debug_assert_eq!(inputs.len(), self.entries.len());

        let anchors: Vec<(ToastPosition, ToastGeometry)> = self
            .entries
            .iter()
            .map(|toast| {
                let (is_origin_window, origin_pane) = match toast.origin() {
                    Some((window, pane)) => (window == this_window_id, Some(pane)),
                    None => (false, None),
                };
                resolve_anchor(
                    toast.position(),
                    is_origin_window,
                    origin_pane,
                    content_rect,
                    resolve_pane_rect,
                )
            })
            .collect();

        group_and_layout(inputs, &anchors, visuals, ppp)
    }

    /// Measure and shape every active toast's text, returning the
    /// [`ToastLayoutInput`]s [`layout_toasts`] needs plus whether any toast
    /// is currently mid-animation (entry or exit). Split out of
    /// [`Self::show`] to keep it under the line-count limit.
    ///
    /// The label/detail text is fit to the available pill content width
    /// (see [`text_width_budget`] / [`fit_text`]) *before* being measured
    /// here, so the metrics stored on the returned [`ToastLayoutInput`] —
    /// and therefore both [`settled_pill_size`] and the emitted
    /// [`ToastTextRun`]s — always describe the exact same (possibly
    /// truncated + ellipsized) string.
    pub(super) fn measure_inputs(
        &self,
        rs: &ToastRenderState,
        font_manager: &mut FontManager,
        sizes: ToastMeasureSizes,
        now: Instant,
    ) -> (Vec<ToastLayoutInput>, bool) {
        let ToastMeasureSizes {
            label_size_px,
            detail_size_px,
            icon_size_px,
            ppp,
        } = sizes;
        let mut inputs = Vec::with_capacity(self.entries.len());
        let mut any_animating = false;
        for toast in &self.entries {
            let label_text = format!("{}: {}", toast.kind.label(), toast.title);
            let has_detail = toast.detail.is_some();
            let detail_text = toast.detail.clone().unwrap_or_default();
            let icon_text = toast.kind.icon().glyph();
            let close_text = ChromeIcon::Close.glyph();

            // Icon and close glyph are single fixed glyphs (never
            // user-controlled text), so they are measured as-is — only
            // label/detail (arbitrary, possibly very long, possibly
            // multi-line, user/program-controlled text) go through
            // `fit_text`. The icon must be measured first: its width feeds
            // `text_width_budget`.
            let icon = rs.text.measure(&icon_text, icon_size_px, font_manager);
            // Rasterized at the same size as the kind icon, for visual
            // consistency between the leading icon and the trailing "x".
            let close_size_px = icon_size_px;
            let close = rs.text.measure(&close_text, close_size_px, font_manager);

            let raw_budget = text_width_budget(icon.width, ppp);
            // Floor the budget at the width of a single ellipsis glyph
            // (measured at the label's, generally larger, font size) so a
            // pathological input (a tiny window, an oversized icon glyph)
            // still leaves `fit_text` a positive width to terminate
            // against, rather than a zero/negative budget.
            let ellipsis_floor = rs.text.measure("…", label_size_px, font_manager).width;
            let text_budget = raw_budget.max(ellipsis_floor).max(1.0);

            let (label_text, label) = fit_text(
                &rs.text,
                &label_text,
                label_size_px,
                text_budget,
                font_manager,
            );
            let (detail_text, detail) = if has_detail {
                fit_text(
                    &rs.text,
                    &detail_text,
                    detail_size_px,
                    text_budget,
                    font_manager,
                )
            } else {
                (
                    detail_text,
                    ToastTextMetrics {
                        width: 0.0,
                        height: 0.0,
                        ascent: 0.0,
                    },
                )
            };

            let buttons =
                measure_buttons(&toast.buttons, &rs.text, detail_size_px, ppp, font_manager);

            let anim = toast.anim(now);
            if anim.opacity < 1.0 || anim.slide_x.abs() > f32::EPSILON || anim.scale < 1.0 {
                any_animating = true;
            }

            inputs.push(ToastLayoutInput {
                id: toast.id,
                kind: toast.kind,
                label_text,
                label_size_px,
                label,
                detail_text,
                detail_size_px,
                detail,
                has_detail,
                icon_size_px,
                icon,
                close_size_px,
                close,
                anim,
                buttons,
            });
        }
        (inputs, any_animating)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::toast::hit::rect_contains;
    use crate::gui::toast::test_support::{
        ENTRY_ANIM, SETTLED_ANIM, button_input, buttons_output, rect_inside, test_input,
        test_window_rect,
    };
    #[test]
    fn toast_colors_border_from_window_stroke_accent_from_semantic() {
        // Issue #433 redesign: the border is the neutral window_stroke color;
        // only the accent bar (and icon) carry the kind's semantic color.
        let mut v = egui::Visuals::dark();
        v.error_fg_color = egui::Color32::from_rgb(220, 40, 40);
        v.window_fill = egui::Color32::from_rgb(30, 32, 36);
        v.window_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 92, 96));

        let colors = toast_colors(ToastKind::Error, &v, 1.0);

        // Border is the neutral stroke color, fully opaque.
        assert!((colors.border_color[0] - 90.0 / 255.0).abs() < 1e-3);
        assert!((colors.border_color[1] - 92.0 / 255.0).abs() < 1e-3);
        assert!((colors.border_color[2] - 96.0 / 255.0).abs() < 1e-3);
        assert!((colors.border_color[3] - 1.0).abs() < 1e-3);

        // Accent is red-dominant (the Error semantic), fully opaque.
        assert!(
            colors.accent[0] > colors.accent[1] && colors.accent[0] > colors.accent[2],
            "accent bar carries the red Error semantic"
        );
        assert!((colors.accent[3] - 1.0).abs() < 1e-3);

        // The fill's two gradient stops straddle the neutral window_fill and
        // are near-neutral (not saturated red).
        for stop in [colors.color_top, colors.color_bottom] {
            assert!(
                (stop[0] - stop[1]).abs() < 0.1 && (stop[1] - stop[2]).abs() < 0.1,
                "fill stop is near-neutral gray, not a saturated tint"
            );
        }
    }

    // -----------------------------------------------------------------
    //  normalize_ws / fit_text — single-line truncation (regression: the
    //  pill's width was clamped but the label/detail text was not, so long
    //  text rendered straight past the pill's right edge; see the `fit_text`
    //  doc comment).
    // -----------------------------------------------------------------

    /// A real `FontManager`, exactly as `renderer::toast_text_pass`'s own
    /// `measure`/`build_instances` test suite constructs one — `fit_text`
    /// calls `ToastTextRenderer::measure`, which needs real shaping.
    fn test_font_manager() -> FontManager {
        FontManager::new(&freminal_common::config::Config::default(), 1.0).unwrap()
    }

    #[test]
    fn normalize_ws_collapses_embedded_newlines_and_trims() {
        // A raw multi-line shader-compile-log-shaped string, the real
        // trigger for this regression (`push_error_toast("Shader error",
        // Some(multi_line_glsl_log))`).
        let raw = "  line one\nline two\r\n\tline three  ";
        assert_eq!(normalize_ws(raw), "line one line two line three");
    }

    #[test]
    fn normalize_ws_leaves_single_line_text_unchanged() {
        assert_eq!(normalize_ws("Shader error"), "Shader error");
    }

    #[test]
    fn fit_text_short_text_is_returned_unchanged() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let (text, metrics) = fit_text(&renderer, "spawn failed", 14.0, 1000.0, &mut fm);
        assert_eq!(text, "spawn failed");
        assert!(!text.ends_with('…'));
        assert!(metrics.width > 0.0);
    }

    #[test]
    fn fit_text_long_text_truncates_with_ellipsis_within_budget() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let long = "x".repeat(500);
        let max_width = 100.0;
        let (text, metrics) = fit_text(&renderer, &long, 14.0, max_width, &mut fm);
        assert!(
            text.ends_with('…'),
            "truncated text must end in an ellipsis: {text:?}"
        );
        assert!(
            text.chars().count() < long.chars().count(),
            "truncated text must be shorter than the input"
        );
        assert!(
            metrics.width <= max_width,
            "truncated text ({}) must measure within the budget ({max_width})",
            metrics.width
        );
    }

    #[test]
    fn fit_text_respects_char_boundaries_for_multibyte_text() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        // "é" is 2 bytes in UTF-8; a byte-index truncation would either
        // panic or split the codepoint. Building `text` here at all (a
        // `String`) already proves it is valid UTF-8.
        let long = "é".repeat(200);
        let (text, metrics) = fit_text(&renderer, &long, 14.0, 60.0, &mut fm);
        assert!(text.ends_with('…'));
        assert!(text.chars().all(|c| c == 'é' || c == '…'));
        assert!(metrics.width <= 60.0);
    }

    #[test]
    fn fit_text_normalizes_embedded_newlines_before_truncating() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let raw = "line one\nline two\nline three";
        let (text, _metrics) = fit_text(&renderer, raw, 14.0, 10_000.0, &mut fm);
        assert!(!text.contains('\n'), "no newline should survive: {text:?}");
        assert_eq!(text, "line one line two line three");
    }

    #[test]
    fn fit_text_falls_back_to_bare_ellipsis_when_budget_only_fits_that() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let ellipsis_width = renderer.measure("…", 14.0, &mut fm).width;
        let (text, metrics) = fit_text(
            &renderer,
            "hello world, this is long",
            14.0,
            ellipsis_width,
            &mut fm,
        );
        assert_eq!(text, "…");
        assert!((metrics.width - ellipsis_width).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------
    //  measure_inputs — end-to-end: truncation keeps the pill within the
    //  clamp, short toasts are unaffected.
    // -----------------------------------------------------------------

    #[test]
    fn measure_inputs_leaves_a_short_toast_unchanged() {
        let mut stack = ToastStack::default();
        let rs = ToastRenderState::default();
        // `test_font_manager()` does real font loading and can take
        // meaningfully long (well over `ANIM_IN`'s 300ms) on a cold cache or
        // loaded CI runner — see the `flaky-tests-are-bugs` skill. Do it
        // BEFORE pushing the toast, so no real wall-clock time elapses
        // between `push` (which stamps `created = Instant::now()`) and the
        // `Instant::now()` passed to `measure_inputs` below, and the
        // any-animating assertion cannot flake on setup being slow.
        let mut fm = test_font_manager();
        stack.error("short", None);
        let ppp = 1.0;
        let sizes = ToastMeasureSizes {
            label_size_px: ToastStack::LABEL_SIZE_PTS * ppp,
            detail_size_px: ToastStack::DETAIL_SIZE_PTS * ppp,
            icon_size_px: ToastStack::LABEL_SIZE_PTS * ppp,
            ppp,
        };
        let (inputs, any_animating) = stack.measure_inputs(&rs, &mut fm, sizes, Instant::now());
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].label_text, "Error: short");
        assert!(!inputs[0].label_text.ends_with('…'));
        // Subtask 121.14: measured immediately after `push`, so the toast is
        // still within its entry-animation window (age 0 < `ANIM_IN`).
        assert!(
            any_animating,
            "a just-pushed toast must report as animating (entry fade in progress)"
        );
    }

    #[test]
    fn measure_inputs_truncates_long_detail_and_pill_stays_within_max_width() {
        let mut stack = ToastStack::default();
        // A raw multi-line GLSL compile log, the real trigger path
        // (`push_error_toast("Shader error", Some(msg))`) that produced the
        // overflow this fixes.
        let long_detail = "ERROR: 0:12: 'foo' : undeclared identifier\n".repeat(20);
        let rs = ToastRenderState::default();
        // See the comment in `measure_inputs_leaves_a_short_toast_unchanged`:
        // font-manager construction happens BEFORE the push so no real time
        // elapses between `push` and the `any_animating` measurement below.
        let mut fm = test_font_manager();
        stack.error("Shader error", Some(long_detail));
        let ppp = 1.0;
        let sizes = ToastMeasureSizes {
            label_size_px: ToastStack::LABEL_SIZE_PTS * ppp,
            detail_size_px: ToastStack::DETAIL_SIZE_PTS * ppp,
            icon_size_px: ToastStack::LABEL_SIZE_PTS * ppp,
            ppp,
        };
        let (inputs, any_animating) = stack.measure_inputs(&rs, &mut fm, sizes, Instant::now());
        assert_eq!(inputs.len(), 1);
        let input = &inputs[0];
        assert!(
            !input.detail_text.contains('\n'),
            "no newline should survive into the emitted run: {:?}",
            input.detail_text
        );
        assert!(input.detail_text.ends_with('…'));
        // Subtask 121.14: same reasoning as the short-toast test above.
        assert!(
            any_animating,
            "a just-pushed toast must report as animating (entry fade in progress)"
        );

        let (width, _height) = settled_pill_size(input, ppp);
        assert!(
            width <= MAX_PILL_WIDTH_PTS.mul_add(ppp, 0.01),
            "pill width ({width}) must stay within the max-width clamp"
        );
    }

    // -----------------------------------------------------------------
    //  layout_toasts — pure geometry
    // -----------------------------------------------------------------

    #[test]
    fn top_right_stack_places_second_toast_below_first() {
        let inputs = [
            test_input(0, 100.0, false, SETTLED_ANIM),
            test_input(1, 100.0, false, SETTLED_ANIM),
        ];
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, 1.0);

        assert_eq!(out.len(), 2);
        let gap = TOAST_STACK_GAP_PTS; // ppp == 1.0
        assert!(
            (out[1].pill.y - out[0].pill.y - (out[0].pill.height + gap)).abs() < 0.01,
            "second toast must sit `height + gap` below the first: {} vs {}",
            out[1].pill.y,
            out[0].pill.y + out[0].pill.height + gap
        );

        // Both right edges align near the window's right edge.
        let right0 = out[0].pill.x + out[0].pill.width;
        let right1 = out[1].pill.x + out[1].pill.width;
        assert!((right0 - right1).abs() < 0.01);
        let expected_right = test_window_rect().max.x - STACK_RIGHT_INSET_PTS;
        assert!((right0 - expected_right).abs() < 0.01);
    }

    /// Issue #433 redesign: the neutral border width flows end-to-end from
    /// `visuals.window_stroke.width` (scaled by ppp) into the final
    /// [`ToastQuad.border_width`], and the border color comes from
    /// `window_stroke.color`. This is the plumbing the visual redesign adds;
    /// `toast_colors` alone (unit-tested elsewhere) doesn't prove it reaches
    /// the quad through `layout_toasts`.
    #[test]
    fn border_width_flows_from_window_stroke_into_quad() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let mut visuals = egui::Visuals::dark();
        visuals.window_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(90, 92, 96));

        let ppp = 2.0;
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            ppp,
        );

        assert_eq!(out.len(), 1);
        // window_stroke.width (1.5 pt) * ppp (2.0) = 3.0 physical px, and the
        // settled (scale 1.0) pill is far larger than 2 * 3.0, so the
        // half-dimension clamp does not bite here.
        let expected = 1.5 * ppp;
        assert!(
            (out[0].pill.border_width - expected).abs() < 0.01,
            "border_width must be window_stroke.width * ppp, got {}",
            out[0].pill.border_width
        );
        // Border color is the neutral stroke color, fully opaque.
        assert!((out[0].pill.border_color[0] - 90.0 / 255.0).abs() < 1e-3);
        assert!((out[0].pill.border_color[3] - 1.0).abs() < 1e-3);
    }

    /// The border width is clamped to half the pill's smaller (animated)
    /// dimension so a chunky border on a tiny mid-scale-in pill can't cross
    /// the pill center and invert. Force the clamp with an absurd stroke
    /// width and a small-scale entry animation.
    #[test]
    fn border_width_clamped_to_half_smaller_dimension() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let mut visuals = egui::Visuals::dark();
        // Absurdly thick stroke to force the clamp.
        visuals.window_stroke = egui::Stroke::new(1000.0, egui::Color32::from_rgb(90, 92, 96));

        let out = layout_toasts(
            &[test_input(0, 100.0, false, ENTRY_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );

        assert_eq!(out.len(), 1);
        let pill = out[0].pill;
        let half_min = pill.width.min(pill.height) / 2.0;
        assert!(
            pill.border_width <= half_min + 0.01,
            "border_width {} must be clamped to <= half the smaller dimension {half_min}",
            pill.border_width
        );
        assert!(
            pill.border_width >= 0.0,
            "border_width must be non-negative"
        );
    }

    #[test]
    fn wider_label_produces_wider_pill_up_to_clamp() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let narrow = layout_toasts(
            &[test_input(0, 50.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let wide = layout_toasts(
            &[test_input(0, 200.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!(wide[0].pill.width > narrow[0].pill.width);

        // An extremely long label must clamp to the max pill width.
        let huge = layout_toasts(
            &[test_input(0, 10_000.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!((huge[0].pill.width - MAX_PILL_WIDTH_PTS).abs() < 0.01);
    }

    #[test]
    fn detail_line_adds_height() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let without = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let with_detail = layout_toasts(
            &[test_input(0, 100.0, true, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!(with_detail[0].pill.height > without[0].pill.height);
        // Detail run must actually be emitted; every toast also gets a
        // trailing dismiss-glyph run.
        assert_eq!(
            with_detail[0].runs.len(),
            4,
            "icon + label + detail + close"
        );
        assert_eq!(without[0].runs.len(), 3, "icon + label + close only");
    }

    #[test]
    fn entry_animation_starts_transparent_and_displaced() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let entry = layout_toasts(
            &[test_input(0, 100.0, false, ENTRY_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let settled = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );

        assert!(entry[0].pill.opacity.abs() < f32::EPSILON);
        assert!(
            entry[0].pill.x > settled[0].pill.x,
            "entry pill (x={}) must be displaced right of the settled pill (x={})",
            entry[0].pill.x,
            settled[0].pill.x
        );
        for run in &entry[0].runs {
            assert!(
                run.color[3].abs() < f32::EPSILON,
                "entry text must be transparent"
            );
        }
        assert!(settled[0].pill.x.is_finite());
        assert!((settled[0].pill.opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn settled_animation_is_unslid_and_opaque() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!((out[0].pill.opacity - 1.0).abs() < f32::EPSILON);
        // The icon/label/detail runs are fully opaque when settled; the
        // trailing dismiss-glyph run is intentionally dimmed to read as a
        // secondary affordance (see `build_text_runs`), so it is checked
        // separately: still visible, but strictly less opaque.
        let (close_run, primary_runs) = out[0].runs.split_last().expect("at least one run");
        for run in primary_runs {
            assert!((run.color[3] - 1.0).abs() < f32::EPSILON);
        }
        assert!(
            close_run.color[3] > 0.0,
            "close glyph must still be visible"
        );
        assert!(
            close_run.color[3] < 1.0,
            "close glyph is dimmed relative to primary text"
        );
    }

    #[test]
    fn window_centered_centers_pill_in_window_rect() {
        let window_rect = test_window_rect();
        let geom = ToastGeometry {
            content_rect: window_rect,
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::WindowCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - window_rect.center().x).abs() < 0.01);
        assert!((center_y - window_rect.center().y).abs() < 0.01);
    }

    #[test]
    fn pane_centered_uses_active_pane_rect() {
        let window_rect = test_window_rect();
        let pane_rect =
            egui::Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(400.0, 300.0));
        let geom = ToastGeometry {
            content_rect: window_rect,
            active_pane_rect: Some(pane_rect),
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::PaneCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - pane_rect.center().x).abs() < 0.01);
        assert!((center_y - pane_rect.center().y).abs() < 0.01);
        // Sanity: the pane center differs from the window center in this test.
        assert!((pane_rect.center().x - window_rect.center().x).abs() > 1.0);
    }

    #[test]
    fn pane_centered_falls_back_to_window_rect_when_no_active_pane() {
        let window_rect = test_window_rect();
        let geom = ToastGeometry {
            content_rect: window_rect,
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::PaneCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - window_rect.center().x).abs() < 0.01);
        assert!((center_y - window_rect.center().y).abs() < 0.01);
    }

    #[test]
    fn hit_rect_logical_divides_out_pixels_per_point() {
        let ppp = 2.0;
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            ppp,
        );
        let pill = &out[0].pill;
        let hit = out[0].hit_rect_logical;
        assert!((hit[0] - pill.x / ppp).abs() < 0.01);
        assert!((hit[1] - pill.y / ppp).abs() < 0.01);
        assert!((hit[2] - (pill.x + pill.width) / ppp).abs() < 0.01);
        assert!((hit[3] - (pill.y + pill.height) / ppp).abs() < 0.01);
    }

    #[test]
    fn output_id_matches_input_id() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let inputs = [
            test_input(42, 100.0, false, SETTLED_ANIM),
            test_input(7, 100.0, false, SETTLED_ANIM),
        ];
        let out = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, 1.0);
        assert_eq!(out[0].id, 42);
        assert_eq!(out[1].id, 7);
    }

    #[test]
    fn close_glyph_run_sits_inside_the_pill_near_the_right_padding() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let ppp = 1.0;
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            ppp,
        );
        let pill = &out[0].pill;
        // icon + label + close, in that order (no detail on this toast).
        assert_eq!(out[0].runs.len(), 3, "icon + label + close");
        let close_run = &out[0].runs[2];
        assert_eq!(
            close_run.text,
            ChromeIcon::Close.glyph(),
            "final run is the dismiss glyph"
        );

        let pill_left = pill.x;
        let pill_right = pill.x + pill.width;
        let pill_top = pill.y;
        let pill_bottom = pill.y + pill.height;

        // The close glyph sits right of the label origin, within the pill's
        // horizontal extent, and inset from the pill's right edge (the
        // padding reserved for the dismiss button).
        let label_run = &out[0].runs[1];
        assert!(
            close_run.origin_x > label_run.origin_x,
            "close glyph is right of the label"
        );
        assert!(close_run.origin_x > pill_left, "close glyph inside pill");
        assert!(
            close_run.origin_x < pill_right,
            "close glyph inset from the pill's right edge"
        );
        assert!(close_run.baseline_y >= pill_top);
        assert!(close_run.baseline_y <= pill_bottom);
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        assert!(layout_toasts(&[], ToastPosition::TopRightStack, geom, &visuals, 1.0).is_empty());
    }

    // -----------------------------------------------------------------
    //  resolve_anchor — per-window anchor resolution (issue #433 slice 2)
    // -----------------------------------------------------------------
    //
    // `resolve_anchor` takes the origin-window comparison as a pre-computed
    // `bool` (`is_origin_window`) rather than a `WindowId` + `this_window_id`
    // pair: `WindowId` only exposes a `winit`-backed constructor (private to
    // `freminal-windowing`), so a second, distinct `WindowId` cannot be
    // fabricated here. See `ToastStack::layout_positioned`, the only caller,
    // for where the real `WindowId` comparison happens.

    #[test]
    fn resolve_anchor_top_right_stack_uses_content_rect_and_ignores_pane() {
        let content_rect = test_window_rect();
        // A resolver that would panic if ever consulted, proving
        // `TopRightStack` never calls it.
        let never_called = |_: PaneId| -> Option<egui::Rect> {
            panic!("TopRightStack must not consult resolve_pane_rect")
        };
        let (position, geom) = resolve_anchor(
            ToastPosition::TopRightStack,
            true,
            Some(PaneId::first()),
            content_rect,
            &never_called,
        );
        assert_eq!(position, ToastPosition::TopRightStack);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, None);
    }

    #[test]
    fn resolve_anchor_window_centered_uses_content_rect_and_ignores_pane() {
        let content_rect = test_window_rect();
        let never_called = |_: PaneId| -> Option<egui::Rect> {
            panic!("WindowCentered must not consult resolve_pane_rect")
        };
        let (position, geom) = resolve_anchor(
            ToastPosition::WindowCentered,
            true,
            Some(PaneId::first()),
            content_rect,
            &never_called,
        );
        assert_eq!(position, ToastPosition::WindowCentered);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, None);
    }

    #[test]
    fn resolve_anchor_pane_centered_resolves_in_origin_window() {
        let content_rect = test_window_rect();
        let pane = PaneId::first();
        let pane_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(30.0, 40.0));
        let resolver = |p: PaneId| (p == pane).then_some(pane_rect);

        let (position, geom) = resolve_anchor(
            ToastPosition::PaneCentered,
            true, // is_origin_window
            Some(pane),
            content_rect,
            &resolver,
        );
        assert_eq!(position, ToastPosition::PaneCentered);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, Some(pane_rect));
    }

    #[test]
    fn resolve_anchor_pane_centered_falls_back_when_not_origin_window() {
        let content_rect = test_window_rect();
        let pane = PaneId::first();
        let pane_rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(30.0, 40.0));
        let resolver = |p: PaneId| (p == pane).then_some(pane_rect);

        let (position, geom) = resolve_anchor(
            ToastPosition::PaneCentered,
            false, // NOT the origin window
            Some(pane),
            content_rect,
            &resolver,
        );
        assert_eq!(position, ToastPosition::WindowCentered);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, None);
    }

    #[test]
    fn resolve_anchor_pane_centered_falls_back_when_no_origin_pane() {
        let content_rect = test_window_rect();
        let never_called =
            |_: PaneId| -> Option<egui::Rect> { panic!("must not be called with no origin pane") };

        let (position, geom) = resolve_anchor(
            ToastPosition::PaneCentered,
            true,
            None, // no origin pane at all
            content_rect,
            &never_called,
        );
        assert_eq!(position, ToastPosition::WindowCentered);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, None);
    }

    #[test]
    fn resolve_anchor_pane_centered_falls_back_when_pane_rect_unavailable() {
        let content_rect = test_window_rect();
        let pane = PaneId::first();
        // Origin window, origin pane present, but the pane no longer exists
        // in this window (e.g. it closed).
        let always_none = |_: PaneId| -> Option<egui::Rect> { None };

        let (position, geom) = resolve_anchor(
            ToastPosition::PaneCentered,
            true,
            Some(pane),
            content_rect,
            &always_none,
        );
        assert_eq!(position, ToastPosition::WindowCentered);
        assert_eq!(geom.content_rect, content_rect);
        assert_eq!(geom.active_pane_rect, None);
    }

    // -----------------------------------------------------------------
    //  group_and_layout — per-anchor grouping + order-preserving scatter
    // -----------------------------------------------------------------
    //
    // Deliberately free of `Toast`/`WindowId` (see its own doc comment), so
    // — unlike `ToastStack::layout_positioned`, its only caller — it is
    // directly unit-testable.

    #[test]
    fn group_and_layout_preserves_input_order_across_mixed_anchors() {
        let content_rect = test_window_rect();
        let top_right_geom = ToastGeometry {
            content_rect,
            active_pane_rect: None,
        };
        let window_centered_geom = ToastGeometry {
            content_rect,
            active_pane_rect: None,
        };
        let inputs = [
            test_input(1, 100.0, false, SETTLED_ANIM),
            test_input(2, 100.0, false, SETTLED_ANIM),
            test_input(3, 100.0, false, SETTLED_ANIM),
        ];
        let anchors = [
            (ToastPosition::TopRightStack, top_right_geom),
            (ToastPosition::WindowCentered, window_centered_geom),
            (ToastPosition::TopRightStack, top_right_geom),
        ];
        let visuals = egui::Visuals::dark();
        let outputs = group_and_layout(&inputs, &anchors, &visuals, 1.0);

        // Same length, and in `inputs`' original order — required by
        // `hit_test`/`topmost_hit` (entries-order == outputs-order).
        assert_eq!(outputs.len(), 3);
        assert_eq!(
            outputs.iter().map(|o| o.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn group_and_layout_stacks_toasts_sharing_the_same_anchor() {
        // Two toasts resolved to the same `TopRightStack` anchor must still
        // stack against EACH OTHER (not just render independently at the
        // same spot) — i.e. `group_and_layout` must match calling
        // `layout_toasts` directly on the whole group.
        let content_rect = test_window_rect();
        let geom = ToastGeometry {
            content_rect,
            active_pane_rect: None,
        };
        let inputs = [
            test_input(10, 100.0, false, SETTLED_ANIM),
            test_input(11, 100.0, false, SETTLED_ANIM),
        ];
        let anchors = [
            (ToastPosition::TopRightStack, geom),
            (ToastPosition::TopRightStack, geom),
        ];
        let visuals = egui::Visuals::dark();
        let grouped = group_and_layout(&inputs, &anchors, &visuals, 1.0);
        let direct = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, 1.0);

        assert_eq!(grouped.len(), direct.len());
        for (g, d) in grouped.iter().zip(direct.iter()) {
            assert_eq!(g.id, d.id);
            assert!((g.pill.x - d.pill.x).abs() < 0.01);
            assert!((g.pill.y - d.pill.y).abs() < 0.01);
        }
    }

    #[test]
    fn group_and_layout_separates_different_pane_centered_anchors() {
        // Two toasts both `PaneCentered`, but resolved against DIFFERENT
        // pane rects, must land in different groups (and therefore not
        // stack against each other) — grouping keys on the resolved rect,
        // not just the `ToastPosition` discriminant.
        let content_rect = test_window_rect();
        let pane_left_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let pane_right_rect =
            egui::Rect::from_min_size(egui::pos2(400.0, 0.0), egui::vec2(200.0, 200.0));
        let geom_left = ToastGeometry {
            content_rect,
            active_pane_rect: Some(pane_left_rect),
        };
        let geom_right = ToastGeometry {
            content_rect,
            active_pane_rect: Some(pane_right_rect),
        };
        let inputs = [
            test_input(1, 100.0, false, SETTLED_ANIM),
            test_input(2, 100.0, false, SETTLED_ANIM),
        ];
        let anchors = [
            (ToastPosition::PaneCentered, geom_left),
            (ToastPosition::PaneCentered, geom_right),
        ];
        let visuals = egui::Visuals::dark();
        let outputs = group_and_layout(&inputs, &anchors, &visuals, 1.0);

        // Each toast centers within its OWN pane rect, not a shared stack —
        // their pill centers must differ (pane A and pane B are disjoint).
        assert_eq!(outputs.len(), 2);
        assert!(
            (outputs[0].pill.x - outputs[1].pill.x).abs() > 1.0,
            "toasts centered in disjoint panes must not share an x position"
        );
    }

    #[test]
    fn group_and_layout_merges_coincident_centered_anchors() {
        // Regression (adversarial review, slice 2): a `PaneCentered` toast
        // whose pane rect equals the window content rect (the common
        // single-pane case) and a `WindowCentered` toast center on the SAME
        // rect. They must share one group and STACK (distinct y positions),
        // not be laid out independently and rendered exactly on top of each
        // other (which would occlude — and, via `topmost_hit`, make
        // un-dismissable — the older toast until it expires).
        let content_rect = test_window_rect();
        let pane_geom = ToastGeometry {
            content_rect,
            // Pane rect == content rect: the single-pane / unsplit case.
            active_pane_rect: Some(content_rect),
        };
        let window_geom = ToastGeometry {
            content_rect,
            active_pane_rect: None,
        };
        let inputs = [
            test_input(1, 100.0, false, SETTLED_ANIM),
            test_input(2, 100.0, false, SETTLED_ANIM),
        ];
        let anchors = [
            (ToastPosition::PaneCentered, pane_geom),
            (ToastPosition::WindowCentered, window_geom),
        ];
        let visuals = egui::Visuals::dark();
        let outputs = group_and_layout(&inputs, &anchors, &visuals, 1.0);

        assert_eq!(outputs.len(), 2);
        // Same center column (one shared centered stack) ...
        assert!(
            (outputs[0].pill.x - outputs[1].pill.x).abs() < 0.01,
            "coincident centered toasts must share the same center x (one stack)"
        );
        // ... but stacked vertically, never overlapping.
        assert!(
            (outputs[0].pill.y - outputs[1].pill.y).abs() > 1.0,
            "coincident centered toasts must stack vertically, not overlap"
        );
    }

    #[test]
    fn layout_key_collapses_coincident_centered_but_keeps_top_right_distinct() {
        let content_rect = test_window_rect();
        let window_geom = ToastGeometry {
            content_rect,
            active_pane_rect: None,
        };
        let pane_eq_geom = ToastGeometry {
            content_rect,
            active_pane_rect: Some(content_rect),
        };
        let pane_diff_geom = ToastGeometry {
            content_rect,
            active_pane_rect: Some(egui::Rect::from_min_size(
                egui::pos2(400.0, 0.0),
                egui::vec2(200.0, 200.0),
            )),
        };
        // Window-centered and pane-centered-on-the-same-rect share a key.
        assert_eq!(
            layout_key(ToastPosition::WindowCentered, window_geom),
            layout_key(ToastPosition::PaneCentered, pane_eq_geom)
        );
        // A pane centered on a DIFFERENT rect does not.
        assert_ne!(
            layout_key(ToastPosition::WindowCentered, window_geom),
            layout_key(ToastPosition::PaneCentered, pane_diff_geom)
        );
        // Top-right is never a centered key.
        assert_eq!(layout_key(ToastPosition::TopRightStack, window_geom), None);
    }

    // -----------------------------------------------------------------
    //  Pure color helpers
    // -----------------------------------------------------------------

    #[test]
    fn color32_to_rgba_is_straight_normalized() {
        let c = egui::Color32::from_rgba_unmultiplied(255, 128, 0, 64);
        let rgba = color32_to_rgba(c);
        assert!((rgba[0] - 1.0).abs() < f32::EPSILON);
        assert!((rgba[1] - 128.0 / 255.0).abs() < f32::EPSILON);
        assert!(rgba[2].abs() < f32::EPSILON);
        assert!((rgba[3] - 64.0 / 255.0).abs() < f32::EPSILON);
    }

    #[test]
    fn lighten_shifts_rgb_and_clamps_leaving_alpha_untouched() {
        let base = [0.5, 0.5, 0.5, 0.9];
        let lighter = lighten(base, 0.4);
        assert!((lighter[0] - 0.9).abs() < f32::EPSILON);
        assert!((lighter[3] - 0.9).abs() < f32::EPSILON);

        let clamped = lighten(base, 10.0);
        assert!((clamped[0] - 1.0).abs() < f32::EPSILON);

        let darker = lighten(base, -10.0);
        assert!(darker[0].abs() < f32::EPSILON);
    }

    fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
        a[0] < b[2] - 0.01 && b[0] < a[2] - 0.01 && a[1] < b[3] - 0.01 && b[1] < a[3] - 0.01
    }

    #[test]
    fn a_button_row_grows_the_pill_to_hold_it() {
        let plain = buttons_output(Vec::new(), false, SETTLED_ANIM, 1.0);
        let with = buttons_output(vec![button_input("OK", 40.0)], false, SETTLED_ANIM, 1.0);
        assert!(with.pill.height > plain.pill.height);
        let last = with.button_rects_logical[0];
        assert!(
            last[3] <= with.hit_rect_logical[3] - PILL_PAD_Y_PTS + 0.01,
            "the bottom padding is kept"
        );
    }

    #[test]
    fn many_long_buttons_wrap_onto_rows_without_overlap_or_overflow() {
        for ppp in [1.0_f32, 2.0] {
            let buttons = (0..8).map(|i| button_input(&format!("long {i}"), 120.0 * ppp));
            let out = buttons_output(buttons.collect(), true, SETTLED_ANIM, ppp);

            let rects = &out.button_rects_logical;
            assert_eq!(rects.len(), 8);
            let mut rows: Vec<f32> = rects.iter().map(|r| r[1]).collect();
            rows.dedup_by(|a, b| (*a - *b).abs() < 0.01);
            assert!(
                rows.len() >= 2,
                "ppp={ppp}: expected wrapping, got {rows:?}"
            );
            assert!(
                out.pill.width <= MAX_PILL_WIDTH_PTS.mul_add(ppp, 0.01),
                "ppp={ppp}: pill width {}",
                out.pill.width
            );
            for (i, rect) in rects.iter().enumerate() {
                assert!(
                    rect_inside(*rect, out.hit_rect_logical),
                    "ppp={ppp}: {rect:?}"
                );
                for other in &rects[i + 1..] {
                    assert!(
                        !rects_overlap(*rect, *other),
                        "ppp={ppp}: {rect:?} {other:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn dismiss_rect_is_inside_the_pill_and_contains_the_close_glyph_centre() {
        for ppp in [1.0_f32, 2.0] {
            for has_detail in [false, true] {
                let geom = ToastGeometry {
                    content_rect: test_window_rect(),
                    active_pane_rect: None,
                };
                let visuals = egui::Visuals::dark();
                let input = test_input(0, 100.0, has_detail, SETTLED_ANIM);
                let out = layout_toasts(
                    std::slice::from_ref(&input),
                    ToastPosition::TopRightStack,
                    geom,
                    &visuals,
                    ppp,
                );
                let hit = out[0].hit_rect_logical;
                let dismiss = out[0].dismiss_rect_logical;
                assert!(dismiss[0] >= hit[0] && dismiss[1] >= hit[1]);
                assert!(dismiss[2] <= hit[2] && dismiss[3] <= hit[3]);
                assert!((dismiss[2] - dismiss[0] - CLOSE_SIZE_PTS).abs() < 0.01);
                assert!((dismiss[3] - dismiss[1] - CLOSE_SIZE_PTS).abs() < 0.01);

                // The close glyph run is the last one; its centre (physical
                // px) converted to logical must be inside the dismiss rect.
                let run = out[0].runs.last().unwrap();
                assert_eq!(run.text, ChromeIcon::Close.glyph());
                let centre = egui::pos2(
                    (run.origin_x + input.close.width / 2.0) / ppp,
                    (run.baseline_y - input.close.ascent + input.close.height / 2.0) / ppp,
                );
                assert!(
                    rect_contains(dismiss, centre),
                    "ppp={ppp} detail={has_detail}: {centre:?} not in {dismiss:?}"
                );
            }
        }
    }

    #[test]
    fn dismiss_rect_stays_inside_hit_rect_mid_animation() {
        let geom = ToastGeometry {
            content_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, ENTRY_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let hit = out[0].hit_rect_logical;
        let dismiss = out[0].dismiss_rect_logical;
        assert!(dismiss[0] >= hit[0] && dismiss[1] >= hit[1]);
        assert!(dismiss[2] <= hit[2] && dismiss[3] <= hit[3]);
    }
}
