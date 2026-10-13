// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The measured-toast input types shared by the layout and button modules.
//!
//! `layout` builds a toast's pill and `buttons` builds its button row, and
//! both need the same measured inputs and scaled metrics. Keeping those types
//! here, below both, means neither module has to import the other's
//! internals: `layout` depends on `buttons`, `buttons` depends on nothing
//! but this module.

use super::{ToastAnim, ToastKind};
use crate::gui::icons::ChromeIcon;
use crate::gui::renderer::ToastTextMetrics;

/// One measured toast button label, input to [`layout_toasts`](super::layout::layout_toasts).
#[derive(Debug, Clone)]
pub(super) struct ToastButtonInput {
    /// The exact label that was measured, already whitespace-normalized and
    /// fitted to the maximum button width (see
    /// [`fit_text`](super::layout::fit_text)).
    pub label_text: String,
    /// The physical pixel size `label_text` was measured/rasterized at.
    pub size_px: f32,
    /// Measured `label_text`, in physical pixels.
    pub label: ToastTextMetrics,
}

/// Per-toast input to [`layout_toasts`](super::layout::layout_toasts): its
/// measured content plus its current animation phase.
///
/// Carries the exact text (`label_text`/`detail_text`, needed to build the
/// actual [`ToastTextRun`](crate::gui::renderer::ToastTextRun)s), the
/// physical pixel size each was measured at
/// (`label_size_px`/`detail_size_px`/`icon_size_px` — needed so the run
/// handed to the text renderer later rasterizes at *exactly* the size
/// [`ToastTextMetrics`] was measured at; any mismatch would desync the
/// pill's width from the glyphs actually drawn), and each string's
/// pre-measured [`ToastTextMetrics`] (needed for layout math). The caller
/// measures text exactly once (via `ToastTextRenderer::measure`) and the
/// layout function never re-shapes it.
#[derive(Debug, Clone)]
pub(super) struct ToastLayoutInput {
    /// Stable id, matching `Toast::id`, used to key the returned outputs
    /// back to their source toast (e.g. for hit-testing dismiss clicks).
    pub id: u64,
    /// The toast's severity — drives color and icon.
    pub kind: ToastKind,
    /// The glyph drawn in the icon slot — the toast's icon override, or the
    /// kind's own icon. [`Self::icon`] (below) is its measurement.
    pub icon_glyph: ChromeIcon,
    /// The exact `"Kind: title"` label string that was measured — already
    /// whitespace-normalized and, if it would otherwise overflow the pill,
    /// truncated with a trailing ellipsis (see
    /// [`fit_text`](super::layout::fit_text)). This is *the* string handed
    /// to the emitted text run, so it is guaranteed to fit the width
    /// [`Self::label`] (below) reports.
    pub label_text: String,
    /// The physical pixel size `label_text` was measured/rasterized at.
    pub label_size_px: f32,
    /// Measured `label_text`, in **physical** pixels (see [`ToastTextMetrics`]).
    pub label: ToastTextMetrics,
    /// The exact detail string that was measured — already
    /// whitespace-normalized and fitted the same way as [`Self::label_text`].
    /// Empty when `has_detail` is `false`.
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
    /// This toast's current animation phase (see `Toast::anim`).
    pub anim: ToastAnim,
    /// Measured button labels, in order. Empty for every unkeyed toast, and
    /// then the toast's geometry is exactly that of a toast with no buttons.
    pub buttons: Vec<ToastButtonInput>,
}

/// Precomputed, `pixels_per_point`-scaled layout constants shared across
/// every toast in one layout call. Bundled into one struct so the per-toast
/// helper functions stay under the `too_many_arguments` limit.
pub(super) struct ToastLayoutMetrics {
    /// `pixels_per_point`, kept alongside the already-scaled fields below
    /// since logical-to-physical hit-rect conversion still needs it raw.
    pub ppp: f32,
    /// The pill's horizontal inner padding, scaled to physical pixels.
    pub pad_x: f32,
    /// The pill's vertical inner padding, scaled to physical pixels.
    pub pad_y: f32,
    /// The icon-to-label gap, scaled to physical pixels.
    pub icon_label_gap: f32,
    /// The dismiss square's side length, scaled to physical pixels.
    pub close_size: f32,
    /// The label-to-detail gap, scaled to physical pixels.
    pub detail_gap: f32,
    /// The theme's corner radius, scaled to physical pixels.
    pub corner_radius: f32,
    /// The neutral chrome border width (from the theme's `window_stroke`,
    /// Modern 1.0 / Retro 1.5 pt), scaled to physical pixels. Issue #433
    /// redesign: replaces the removed glow as the pill's separation from the
    /// terminal, and gives Modern/Retro a subtly different border weight.
    pub border_width: f32,
}

/// Height, physical pixels, of the label (and optional detail) block.
pub(super) fn text_block_height(input: &ToastLayoutInput, detail_gap: f32) -> f32 {
    let mut text_height = input.label.height;
    if input.has_detail {
        text_height += detail_gap + input.detail.height;
    }
    text_height
}
