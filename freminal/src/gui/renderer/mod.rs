// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal rendering pipeline split into focused sub-modules.
//!
//! - [`gpu`] — [`TerminalRenderer`] struct, GL init/draw/destroy, shader compilation,
//!   VAO/VBO setup, and GL upload helpers.
//! - [`shaders`] — GLSL source string constants for the four shader passes
//!   (decoration, background, foreground, image).
//! - [`vertex`] — CPU-side vertex/instance builders, `FgRenderOptions`, and helpers.
//!   Contains the full test suite for vertex generation logic.

pub mod errors;
pub mod gpu;
pub(super) mod shaders;
pub mod vertex;

pub use gpu::{TerminalRenderer, WindowPostRenderer};
pub use vertex::{
    BackgroundFrame, CURSOR_QUAD_FLOATS, FgRenderOptions, ImageDrawEntry, MatchHighlight,
    build_background_instances, build_cursor_verts_only, build_foreground_instances,
    build_image_verts,
};

use conv2::{ConvUtil, RoundToNearest};

/// What a single pane contributed to the frame just rendered (#435).
///
/// Only the **active pane** ever draws a cursor, so a cursor blink/move — or
/// a switch of which pane is active — only touches the active pane (and, on a
/// switch, the previously-active pane, which erases its cursor). Every other
/// pane whose content did not change contributes nothing to the frame. This
/// enum lets the per-window aggregation distinguish those cases:
///
/// - [`PaneFrameDamage::Full`] — the pane did a full content rebuild; the
///   frame must clear + present fully.
/// - [`PaneFrameDamage::CursorOnly`] — the pane took the cursor-only fast
///   path; the rect (if any) is the changed cursor region. `None` means the
///   pane's cursor region did not resolve to a valid rect (degenerate size);
///   the aggregator treats that as a full frame out of caution.
/// - [`PaneFrameDamage::Unchanged`] — the pane re-drew its existing vertices
///   with no change at all (the common inactive-pane case); it contributes no
///   damage and does **not** force a full frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneFrameDamage {
    /// The pane rebuilt its full content this frame.
    Full,
    /// The pane took the cursor-only fast path; carries the changed region.
    CursorOnly(Option<CursorDamage>),
    /// The pane rendered no change (reused existing vertices).
    #[default]
    Unchanged,
}

/// The changed screen region of a cursor-only frame (#435).
///
/// Coordinates are **physical framebuffer pixels with a bottom-left origin**
/// (the OpenGL / EGL convention shared by `glScissor` and
/// `eglSwapBuffersWithDamage`).
///
/// A cursor blink or a cursor move changes at most two cells (the old cursor
/// cell, now revealing its glyph, and the new cursor cell). This rect is the
/// tight bounding box of those cells; presenting only it — and skipping the
/// full-framebuffer clear — is what turns a blink from a full-scene redraw
/// into a one-region update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorDamage {
    /// X of the lower-left corner, physical pixels.
    pub x: i32,
    /// Y of the lower-left corner, physical pixels (bottom-left origin).
    pub y: i32,
    /// Width, physical pixels.
    pub width: i32,
    /// Height, physical pixels.
    pub height: i32,
}

impl CursorDamage {
    /// Build the damage bounding box for a cursor-only frame.
    ///
    /// Inputs are all in the terminal's own coordinate space:
    /// - `viewport_left`, `viewport_bottom_from_top`: the terminal viewport's
    ///   top-left corner in physical framebuffer pixels, measured from the
    ///   **top-left** of the framebuffer (egui's origin convention).
    /// - `fb_height`: the full framebuffer height in physical pixels, used to
    ///   flip the Y axis into GL's bottom-left origin.
    /// - `cursor_cells`: each `(px_x, px_y, w, h)` is a changed cell in
    ///   physical pixels **relative to the viewport top-left**, top-left
    ///   origin (matching `cursor_pixel_pos` / cell dimensions in `show`).
    ///   Usually one entry (blink) or two (a move: old + new cell).
    ///
    /// Returns `None` when there are no changed cells (nothing to present).
    #[must_use]
    pub fn from_cursor_cells(
        viewport_left: f32,
        viewport_top: f32,
        fb_height: i32,
        cursor_cells: &[(f32, f32, f32, f32)],
    ) -> Option<Self> {
        // 1px safety pad applied outward on every side before clamping (see
        // below). Declared here to satisfy `items_after_statements`.
        const PAD: i32 = 1;

        if cursor_cells.is_empty() {
            return None;
        }

        // Union the cells in top-left-origin framebuffer pixels first.
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for &(cx, cy, cw, ch) in cursor_cells {
            let left = viewport_left + cx;
            let top = viewport_top + cy;
            min_x = min_x.min(left);
            min_y = min_y.min(top);
            max_x = max_x.max(left + cw);
            max_y = max_y.max(top + ch);
        }

        // Round outward to whole pixels, then expand by a 1px safety pad on
        // every side. The values are already integral (floor/ceil), so the
        // rounding scheme only resolves the trait. The pad absorbs two sources
        // of sub-pixel error without risk: (a) the framebuffer height fed in is
        // reconstructed from egui's logical screen rect times the (possibly
        // fractional) scale factor, which can be off by up to a pixel; (b) a
        // cursor at a fractional cell origin. Over-presenting a slightly larger
        // region is always correct — every pixel in it is the freshly-drawn,
        // correct pixel — whereas under-covering leaves stale pixels. This also
        // makes the (unclamped-on-the-right) X edge robust.
        let px_min_x: i32 = min_x.floor().approx_as_by::<i32, RoundToNearest>().ok()? - PAD;
        let px_min_y: i32 = min_y.floor().approx_as_by::<i32, RoundToNearest>().ok()? - PAD;
        let px_max_x: i32 = max_x.ceil().approx_as_by::<i32, RoundToNearest>().ok()? + PAD;
        let px_max_y: i32 = max_y.ceil().approx_as_by::<i32, RoundToNearest>().ok()? + PAD;

        // Clamp to the framebuffer so we never emit a negative-origin or
        // out-of-bounds rect (a cursor at row 0 / col 0, or sub-pixel slop).
        let top_clamped = px_min_y.max(0);
        let bottom_clamped = px_max_y.min(fb_height);
        let height = bottom_clamped - top_clamped;
        let left_clamped = px_min_x.max(0);
        let width = px_max_x - left_clamped;
        if width <= 0 || height <= 0 {
            return None;
        }

        // Flip Y from top-left origin to GL/EGL bottom-left origin.
        let y = fb_height - bottom_clamped;

        Some(Self {
            x: left_clamped,
            y,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod cursor_damage_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::CursorDamage;

    // All expectations below include the 1px safety pad applied outward on
    // every side before clamping (see `from_cursor_cells`).

    #[test]
    fn single_cell_blink_flips_y_to_bottom_left_origin() {
        // Framebuffer 800x600. Terminal viewport at top-left (0,0). A cell of
        // 10x20 px at column 0, row 0 (top-left corner of the viewport).
        let d = CursorDamage::from_cursor_cells(0.0, 0.0, 600, &[(0.0, 0.0, 10.0, 20.0)])
            .expect("one cell yields damage");
        // x: [-1,11] clamped-left to [0,11] -> x=0, width=11.
        assert_eq!(d.x, 0);
        assert_eq!(d.width, 11);
        // top: -1 clamped to 0; bottom: 21 -> height 21; y = 600 - 21 = 579.
        assert_eq!(d.height, 21);
        assert_eq!(d.y, 579);
    }

    #[test]
    fn viewport_offset_is_added() {
        // Viewport starts 100px right, 50px down (e.g. a gutter + menu bar).
        let d = CursorDamage::from_cursor_cells(100.0, 50.0, 600, &[(30.0, 40.0, 10.0, 20.0)])
            .expect("damage");
        // Cell top-left in fb coords: x=130, top=90, bottom=110; +/-1 pad.
        assert_eq!(d.x, 129);
        assert_eq!(d.width, 12);
        assert_eq!(d.height, 22);
        // bottom 110 + pad 1 = 111; y = 600 - 111 = 489.
        assert_eq!(d.y, 489);
    }

    #[test]
    fn cursor_move_unions_old_and_new_cells() {
        // Old cell at col 0, new cell at col 5 (both row 0), 10x20 cells.
        let d = CursorDamage::from_cursor_cells(
            0.0,
            0.0,
            600,
            &[(0.0, 0.0, 10.0, 20.0), (50.0, 0.0, 10.0, 20.0)],
        )
        .expect("damage");
        // Union x[0,60] -> pad [-1,61] -> clamp-left [0,61] -> width 61.
        assert_eq!(d.x, 0);
        assert_eq!(d.width, 61);
        assert_eq!(d.height, 21);
        assert_eq!(d.y, 579);
    }

    #[test]
    fn subpixel_edges_round_outward() {
        // A cell at a fractional origin must not clip its edge pixels.
        let d = CursorDamage::from_cursor_cells(0.0, 0.0, 600, &[(0.5, 0.5, 10.0, 20.0)])
            .expect("damage");
        // left floor 0 -1 = -1 -> clamp 0; right ceil 11 +1 = 12 -> width 12.
        assert_eq!(d.x, 0);
        assert_eq!(d.width, 12);
        // top floor 0 -1 -> 0; bottom ceil 21 +1 = 22 -> height 22.
        assert_eq!(d.height, 22);
        assert_eq!(d.y, 600 - 22);
    }

    #[test]
    fn empty_cells_yield_no_damage() {
        assert_eq!(CursorDamage::from_cursor_cells(0.0, 0.0, 600, &[]), None);
    }

    #[test]
    fn cell_clamped_to_framebuffer_bounds() {
        // A cell partly below the framebuffer bottom must clamp, not emit a
        // negative-origin or oversized rect.
        let d = CursorDamage::from_cursor_cells(0.0, 590.0, 600, &[(0.0, 0.0, 10.0, 20.0)])
            .expect("damage");
        // top 590 - 1 = 589; bottom 610 + 1 = 611 clamped to 600 -> height 11;
        // y = 600 - 600 = 0.
        assert_eq!(d.height, 11);
        assert_eq!(d.y, 0);
    }
}
