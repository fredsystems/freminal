// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Offscreen egui-level pixel harness for [`crate::frame_paint::paint_frame`]
//! (Task 124.19b).
//!
//! One concept: driving `paint_frame` -- the real egui `run_ui` /
//! tessellate / paint sequence, not just raw GL calls -- against an offscreen
//! pbuffer, then reading the resulting pixels back. This exists because the
//! Task 123 Phase 2 harness (`freminal/src/gui/renderer/pixel_harness.rs`)
//! cannot see this class of bug at all: it drives `HeadlessRenderer` directly
//! against `glow`, and never constructs an `egui::Context` or calls
//! `paint_frame`, so it never paints an egui `CentralPanel` (or any other
//! egui-level chrome) in the first place. This harness is test infrastructure
//! only -- it has no place in a production build, which is why it is gated
//! identically to [`crate::gl_context_offscreen`], the module it is built on.
//!
//! # Requirements
//!
//! Same as [`crate::gl_context_offscreen`]: Mesa plus a live `$DISPLAY`, in
//! practice the Linux `gl-pixel` Nix dev shell, under `xvfb-run`:
//!
//! ```sh
//! xvfb-run -a cargo test -p freminal-windowing --features gl-offscreen
//! ```

use std::cell::Cell;
use std::sync::Arc;

use conv2::ConvUtil;
use glow::HasContext;

use crate::frame_paint::{
    DamageHistory, FrameSurface, PaintFrameRequest, PartialPresentDecision, PartialPresentSupport,
    paint_frame,
};
use crate::gl_context_offscreen::OffscreenGl;

/// Failures constructing a [`HarnessDriver`].
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// No offscreen GL context could be created at all -- the routine,
    /// expected-to-skip case (missing Mesa, no `$DISPLAY`, not running under
    /// `xvfb-run`). Callers should skip rather than fail, exactly as
    /// [`OffscreenGl::new`]'s own callers do.
    #[error("no offscreen GL context: {0}")]
    NoContext(#[from] crate::Error),
    /// `egui_glow::Painter::new` itself failed against a pbuffer context
    /// that WAS created successfully. Unlike [`Self::NoContext`], this is
    /// not a condition this subtask's stop conditions permit working
    /// around -- a caller encountering it should treat it as a hard
    /// failure, not a skip.
    #[error("egui_glow painter creation failed against the offscreen context: {0}")]
    PainterCreation(String),
}

/// A [`FrameSurface`] over an [`OffscreenGl`] pbuffer, with caller-chosen
/// partial-present support and buffer age.
///
/// This is the load-bearing piece of the harness: a real windowed
/// [`crate::gl_context::GlState`] can never report `buffer_age() == 1` on
/// its very first frame (a fresh surface's back buffer holds nothing), so a
/// test cannot reach [`PartialPresentDecision::Taken`] by driving a real
/// surface through only two frames. This type lets a test simply declare
/// the surface state [`decide_partial_present`](crate::frame_paint::decide_partial_present)
/// needs, independent of how many frames have actually been painted.
///
/// `age` is a [`Cell<u32>`], not a plain field: a 124.18 test needs
/// different frames within the SAME `run_frames` sequence to report
/// different ages (e.g. a `Full` baseline frame, then a `Partial` frame at
/// `age == 1` to seed history, then a `Partial` frame at `age == 2` to
/// exercise the union) -- see [`Self::set_age`].
///
/// Also records whether [`FrameSurface::clear_to`] was called via a
/// [`Cell<bool>`] -- the harness runs entirely on one thread, so a `Cell`
/// is sufficient and avoids the ceremony of a `Mutex` for a single flag.
pub struct HarnessSurface {
    off: OffscreenGl,
    support: PartialPresentSupport,
    age: Cell<u32>,
    cleared: Cell<bool>,
}

impl HarnessSurface {
    /// Wrap `off` as a [`FrameSurface`] that reports `support` and, until
    /// changed via [`Self::set_age`], `age` for every frame painted through
    /// it (see the type doc for why this can't be probed from `off`
    /// itself). Per `freminal-state-representation`, this takes the named
    /// [`PartialPresentSupport`] rather than a bare `bool`.
    #[must_use]
    pub const fn new(off: OffscreenGl, support: PartialPresentSupport, age: u32) -> Self {
        Self {
            off,
            support,
            age: Cell::new(age),
            cleared: Cell::new(false),
        }
    }

    /// Change the buffer age this surface reports to subsequent frames.
    /// Lets one [`HarnessDriver`] paint a sequence where different frames
    /// need different ages (see the type doc).
    pub fn set_age(&self, age: u32) {
        self.age.set(age);
    }

    /// Whether [`FrameSurface::clear_to`] was called since the last
    /// [`Self::reset_cleared`].
    #[must_use]
    pub const fn was_cleared(&self) -> bool {
        self.cleared.get()
    }

    /// Clear the recorded flag before painting the next frame, so each
    /// frame's result reflects only that frame's own clear decision.
    pub fn reset_cleared(&self) {
        self.cleared.set(false);
    }

    /// A clone of the shared glow context, for [`egui_glow::Painter::new`].
    fn gl_arc(&self) -> Arc<glow::Context> {
        self.off.gl_arc()
    }
}

impl FrameSurface for HarnessSurface {
    fn glow(&self) -> &glow::Context {
        self.off.gl()
    }

    fn partial_present_support(&self) -> PartialPresentSupport {
        self.support
    }

    fn back_buffer_age(&self) -> u32 {
        self.age.get()
    }

    fn clear_to(&self, color: [f32; 4]) {
        self.cleared.set(true);
        // SAFETY: `self.off`'s context is current for the lifetime of
        // `self.off` (established by `OffscreenGl::new`); this harness never
        // shares a context across threads, so no other thread can have made
        // a different context current between construction and this call.
        unsafe {
            self.off
                .gl()
                .clear_color(color[0], color[1], color[2], color[3]);
            self.off.gl().clear(glow::COLOR_BUFFER_BIT);
        }
    }
}

/// A captured RGBA8 image, top-left origin.
///
/// This is the same idea as
/// `freminal::gui::renderer::pixel_harness::PixelFrame`, but that type
/// cannot be reused from here: `freminal` depends on `freminal-windowing`,
/// not the other way around, so a type defined in the `freminal` crate is
/// unreachable from this one. Deliberately minimal compared to its
/// cross-crate cousin -- no golden-file machinery, no configurable channel
/// tolerance (this subtask forbids one) -- because this harness only needs
/// exact equality/difference checks within a single test run, never a
/// cross-run golden comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readback {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8, row-major from the top.
    pub rgba: Vec<u8>,
}

impl Readback {
    /// The RGBA quadruple at `(x, y)`, top-left origin, or `None` if either
    /// coordinate is outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let width: usize = self.width.value_as().ok()?;
        let idx = y
            .value_as::<usize>()
            .ok()?
            .checked_mul(width)?
            .checked_add(x.value_as::<usize>().ok()?)?
            .checked_mul(4)?;
        let slice = self.rgba.get(idx..idx.checked_add(4)?)?;
        Some([slice[0], slice[1], slice[2], slice[3]])
    }

    /// Count of pixels that differ from `other` at all, byte for byte.
    ///
    /// Deliberately exact -- no channel tolerance parameter. Images with
    /// mismatched dimensions compare only over their common prefix (`zip`
    /// stops at the shorter iterator); every caller in this module
    /// constructs both images from the same driver at the same size, so
    /// that case does not arise in practice.
    #[must_use]
    pub fn differing_pixels(&self, other: &Self) -> usize {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(other.rgba.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count()
    }
}

/// One frame's paint closure, in the exact shape [`paint_frame`] expects.
///
/// Boxed (rather than a bare generic on [`HarnessDriver::run_frames`]) so a
/// single `Vec` can hold frames whose closures capture different local
/// state -- mirroring the Phase 2 harness's own frame-sequence idiom
/// (`capture_after_cursor_only_frames`), which likewise needs one call per
/// frame with per-frame content.
pub type FrameFn<'a> = Box<dyn FnMut(&egui::Context, &glow::Context) -> crate::FrameSignals + 'a>;

/// The result of painting one frame through [`HarnessDriver::run_frames`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessFrameResult {
    /// The framebuffer contents immediately after this frame's paint.
    pub readback: Readback,
    /// Why this frame did or did not take the skip-clear + partial-present
    /// path -- see [`PartialPresentDecision`].
    pub decision: PartialPresentDecision,
    /// Whether [`FrameSurface::clear_to`] was called for this frame.
    pub cleared: bool,
}

/// Drives [`paint_frame`] against one [`HarnessSurface`], painting a
/// caller-supplied sequence of frames with no clear or buffer swap between
/// them beyond whatever `paint_frame` itself decides.
///
/// Painting successive frames into the same pbuffer with no intervening
/// swap is exactly the "back buffer still holds the previous frame"
/// situation a real double-buffered window can only report via
/// `buffer_age() == 1` -- here it is true by construction, because there is
/// only one buffer and nothing has told the driver to treat it as stale.
pub struct HarnessDriver {
    surface: HarnessSurface,
    ctx: egui::Context,
    painter: egui_glow::Painter,
    width: u32,
    height: u32,
    /// This driver's record of recent frames' own declared damage (124.18)
    /// -- owned here, not per-`run_frames`-call, so a test can call
    /// [`Self::run_frames`] more than once (e.g. to change
    /// [`Self::set_surface_age`] between individual frames) and still have
    /// later frames see earlier ones' history.
    history: DamageHistory,
}

impl HarnessDriver {
    /// Create a driver over a fresh `width` x `height` offscreen surface
    /// that reports `support` and `age` for every frame painted through it.
    ///
    /// # Errors
    ///
    /// [`HarnessError::NoContext`] when no offscreen GL context is
    /// available (skip, do not fail -- see the type doc);
    /// [`HarnessError::PainterCreation`] if `egui_glow::Painter::new` fails
    /// against a context that WAS created successfully (a hard failure,
    /// not a skip).
    pub fn new(
        width: u32,
        height: u32,
        support: PartialPresentSupport,
        age: u32,
    ) -> Result<Self, HarnessError> {
        let off = OffscreenGl::new(width, height)?;
        let surface = HarnessSurface::new(off, support, age);

        let ctx = egui::Context::default();
        // Pin the scale factor so a readback is deterministic regardless of
        // whatever DPI the host environment would otherwise report through
        // `RawInput` -- this subtask forbids a comparison tolerance, so
        // nothing here may depend on host-specific scaling.
        ctx.set_pixels_per_point(1.0);

        let painter = egui_glow::Painter::new(surface.gl_arc(), "", None, false)
            .map_err(|e| HarnessError::PainterCreation(format!("{e}")))?;

        Ok(Self {
            surface,
            ctx,
            painter,
            width,
            height,
            history: DamageHistory::new(),
        })
    }

    /// Change the buffer age reported to frames painted from now on -- see
    /// [`HarnessSurface::set_age`]. Lets a test paint a sequence across
    /// multiple [`Self::run_frames`] calls where different frames need
    /// different ages (e.g. seed one frame of history at `age == 1`, then
    /// exercise a union at `age == 2`), without which every frame in a
    /// driver's lifetime would be stuck at the age passed to [`Self::new`].
    pub fn set_surface_age(&self, age: u32) {
        self.surface.set_age(age);
    }

    /// Paint each of `frames` in order into the same surface, returning one
    /// [`HarnessFrameResult`] per frame in the order painted.
    pub fn run_frames(&mut self, frames: Vec<FrameFn<'_>>) -> Vec<HarnessFrameResult> {
        let width_pts = self.width.approx_as::<f32>().unwrap_or(0.0);
        let height_pts = self.height.approx_as::<f32>().unwrap_or(0.0);
        let screen_rect =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(width_pts, height_pts));

        let mut results = Vec::with_capacity(frames.len());
        for ui_fn in frames {
            self.surface.reset_cleared();

            let raw_input = egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            };
            let request = PaintFrameRequest {
                size_px: [self.width, self.height],
                raw_input,
                // Deliberately distinct from every colour a test paints, so
                // a stray clear -- one that `was_cleared()` failed to
                // catch, or an unwanted extra clear -- would show up as an
                // obviously wrong pixel rather than blending in.
                clear_color: [0.0, 0.0, 0.0, 1.0],
                present_flag: None,
                damage_history: &mut self.history,
            };

            let output = paint_frame(&self.surface, &self.ctx, &mut self.painter, request, ui_fn);
            let cleared = self.surface.was_cleared();
            let readback = self.read_back();

            results.push(HarnessFrameResult {
                readback,
                decision: output.decision,
                cleared,
            });
        }
        results
    }

    /// Read the current framebuffer back as RGBA8, flipping to a top-left
    /// origin. Mirrors
    /// `freminal::gui::renderer::pixel_harness::read_back` exactly (see
    /// [`Readback`]'s doc for why that function cannot be called from
    /// here).
    fn read_back(&self) -> Readback {
        let gl = self.surface.glow();
        let w = self.width.value_as::<usize>().unwrap_or(0);
        let h = self.height.value_as::<usize>().unwrap_or(0);
        let row_bytes = w.saturating_mul(4);
        let mut bottom_up = vec![0u8; row_bytes.saturating_mul(h)];

        unsafe {
            // `finish` rather than `flush`: readback must observe a
            // completed frame, and llvmpipe is asynchronous enough that
            // flush alone can return a partially-rendered buffer.
            gl.finish();
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.read_pixels(
                0,
                0,
                self.width.value_as::<i32>().unwrap_or(0),
                self.height.value_as::<i32>().unwrap_or(0),
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bottom_up)),
            );
        }

        let mut rgba = Vec::with_capacity(bottom_up.len());
        for row in bottom_up.chunks_exact(row_bytes).rev() {
            rgba.extend_from_slice(row);
        }

        Readback {
            width: self.width,
            height: self.height,
            rgba,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use conv2::ConvUtil;
    use egui::{Color32, LayerId, Pos2, Rect, pos2, vec2};

    use super::{FrameFn, HarnessDriver, HarnessError};
    use crate::frame_paint::{PartialPresentDecision, PartialPresentSupport};
    use crate::{DamageRect, FrameDamage, FrameSignals};

    /// Canvas size for every test in this module. Kept small: the harness
    /// paints solid rects, not text, so there is nothing to gain from a
    /// larger surface, and llvmpipe context creation dominates runtime
    /// regardless of pbuffer size.
    const CANVAS: u32 = 64;

    /// Whether this environment has promised a working offscreen GL stack.
    /// Set only by the `gl-pixel` Nix shell / its CI job -- see
    /// `gl_context_offscreen.rs`'s identical guard for the full rationale
    /// (in particular, why this is NOT keyed on `CI`).
    fn gl_context_required() -> bool {
        std::env::var("FREMINAL_REQUIRE_GL").is_ok_and(|v| v != "0" && !v.is_empty())
    }

    /// Construct a [`HarnessDriver`], skipping (not failing) when no GL
    /// context is available -- unless `FREMINAL_REQUIRE_GL` says this
    /// environment promised one, in which case that is a broken runner and
    /// must fail loudly. Mirrors
    /// `freminal::gui::renderer::pixel_harness::tests::capture_or_skip`
    /// exactly, including the CI-vs-`FREMINAL_REQUIRE_GL` distinction
    /// recorded there (an earlier revision keyed the wrong variable and
    /// produced six spurious CI failures).
    ///
    /// A [`HarnessError::PainterCreation`] is never skipped: per this
    /// subtask's stop conditions, that failure is a genuine finding to
    /// report, not a condition to route around.
    fn new_driver_or_skip(
        support: PartialPresentSupport,
        age: u32,
        what: &str,
    ) -> Option<HarnessDriver> {
        match HarnessDriver::new(CANVAS, CANVAS, support, age) {
            Ok(driver) => Some(driver),
            Err(HarnessError::NoContext(e)) => {
                assert!(
                    !gl_context_required(),
                    "{what}: no offscreen GL context ({e}) despite \
                     FREMINAL_REQUIRE_GL being set -- that variable means \
                     this environment promised Mesa and Xvfb, so this is a \
                     broken runner, not a reason to skip"
                );
                eprintln!(
                    "SKIP {what}: no GL context ({e})\n  (run under `xvfb-run \
                     -a cargo test -p freminal-windowing --features \
                     gl-offscreen`)"
                );
                None
            }
            Err(other) => panic!("{what}: harness failed: {other}"),
        }
    }

    // ── Scene geometry, shared by all three tests ───────────────────────
    //
    // The marker and the declared damage region are placed far apart (top
    // area vs. bottom-right area of the 64x64 canvas) so neither could ever
    // overlap the other by construction. Every sampled pixel is inset at
    // least 5px from its rect's edges, well clear of egui's tessellator
    // feathering (~1px), so exact comparisons need no tolerance.

    /// The full-canvas opaque background fill -- stands in for the
    /// `CentralPanel` fill from the established-fact background: opaque at
    /// the default `bg_opacity = 1.0`, painted every frame regardless of
    /// declared damage, because egui is immediate-mode.
    fn background_fill_rect() -> Rect {
        let side: f32 = CANVAS.approx_as::<f32>().unwrap_or(0.0);
        Rect::from_min_size(Pos2::ZERO, vec2(side, side))
    }
    fn background_color() -> Color32 {
        Color32::from_rgb(30, 30, 30)
    }

    /// Stand-in for "content the previous frame already drew, which a
    /// partial present promises to leave untouched" -- painted only on the
    /// baseline frame.
    fn marker_rect() -> Rect {
        Rect::from_min_size(pos2(40.0, 4.0), vec2(16.0, 16.0))
    }
    fn marker_color() -> Color32 {
        Color32::from_rgb(200, 0, 0)
    }
    /// Sample point well inside `marker_rect`'s interior.
    const MARKER_SAMPLE: (u32, u32) = (48, 12);

    /// The region the second frame declares as damaged, and paints new
    /// content into.
    fn damage_paint_rect() -> Rect {
        Rect::from_min_size(pos2(4.0, 40.0), vec2(16.0, 16.0))
    }
    fn damage_paint_color() -> Color32 {
        Color32::from_rgb(0, 200, 0)
    }
    /// Sample point well inside `damage_paint_rect`'s interior.
    const DAMAGE_SAMPLE: (u32, u32) = (12, 48);

    /// The `DamageRect` this test *declares* for the second frame, in
    /// physical pixels with a bottom-left origin (EGL convention -- see
    /// `DamageRect`'s own doc) matching `damage_paint_rect`, which is
    /// specified in egui's top-left points. With `pixels_per_point == 1.0`
    /// (pinned by `HarnessDriver::new`), points and physical pixels
    /// coincide, so only the vertical flip is needed: `y_bl = CANVAS -
    /// (top_y + height)`.
    ///
    /// Since 124.18 this value IS what every unclipped primitive gets
    /// clipped to on a `Taken` frame (at `age == 1`, with no history, the
    /// reconstructed region is exactly this rect). It is built to
    /// genuinely correspond to `damage_paint_rect`, so this test reads as
    /// a realistic app's damage report rather than an arbitrary
    /// placeholder.
    fn declared_damage_rect() -> DamageRect {
        let canvas: i32 = CANVAS.value_as().unwrap();
        DamageRect {
            x: 4,
            y: canvas - (40 + 16),
            width: 16,
            height: 16,
        }
    }

    // ── Additional geometry for the age == 2 union test ─────────────────
    //
    // A third, distinct region ("prev") models content a PREVIOUS frame
    // changed, which the CURRENT frame's own declared damage does not
    // include (because, per `FrameDamage::Partial`'s contract, nothing
    // changed there relative to the previous frame) but which
    // `DamageHistory`'s union must still fold in when the back buffer is 2
    // frames stale. Placed apart from the marker (top) and
    // `damage_paint_rect` (bottom-left) so all three can never overlap.

    fn prev_damage_rect() -> Rect {
        Rect::from_min_size(pos2(44.0, 44.0), vec2(16.0, 16.0))
    }
    fn prev_damage_color() -> Color32 {
        Color32::from_rgb(0, 0, 200)
    }
    /// Sample point well inside `prev_damage_rect`'s interior.
    const PREV_SAMPLE: (u32, u32) = (52, 52);

    /// `prev_damage_rect`'s `DamageRect`, by the same construction as
    /// [`declared_damage_rect`].
    fn declared_prev_rect() -> DamageRect {
        let canvas: i32 = CANVAS.value_as().unwrap();
        DamageRect {
            x: 44,
            y: canvas - (44 + 16),
            width: 16,
            height: 16,
        }
    }

    /// The exact bounding-box union of [`declared_damage_rect`] and
    /// [`declared_prev_rect`], computed by hand (not by calling production
    /// code) so the test pins an independently-derived expected value.
    fn declared_damage_and_prev_union() -> DamageRect {
        let a = declared_damage_rect();
        let b = declared_prev_rect();
        let min_x = a.x.min(b.x);
        let min_y = a.y.min(b.y);
        let max_x = (a.x + a.width).max(b.x + b.width);
        let max_y = (a.y + a.height).max(b.y + b.height);
        DamageRect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    }

    /// Frame 1: the baseline. Paints the opaque background fill plus the
    /// marker, and reports `Full` damage (as an app's very first frame
    /// always must).
    fn baseline_frame() -> FrameFn<'static> {
        Box::new(|ctx: &egui::Context, _gl: &glow::Context| {
            let painter = ctx.layer_painter(LayerId::background());
            // Shape 0: the chrome fill -- "head" once split by `band_range`.
            painter.rect_filled(background_fill_rect(), 0.0, background_color());
            // Shape 1: the marker -- reported as "tail" below.
            painter.rect_filled(marker_rect(), 0.0, marker_color());
            FrameSignals {
                frame_damage: FrameDamage::Full,
                // Shape 0 (the fill) is "head"; shape 1 (the marker) is
                // "tail" -- mirroring the real split, where the
                // `CentralPanel` background fill is painted before
                // everything else.
                band_range: 1..1,
                terminal_requested_delay: None,
            }
        })
    }

    /// Frame 2 (defect-triggering): repaints the SAME opaque background
    /// fill (egui repaints it every frame; declaring `Partial` damage does
    /// not exempt an app's own immediate-mode chrome from being re-emitted
    /// into `full_output.shapes`), paints new content in the declared
    /// damage region, and does NOT repaint the marker. Reports `Partial`
    /// with one non-empty rect.
    fn partial_frame_without_marker() -> FrameFn<'static> {
        Box::new(|ctx: &egui::Context, _gl: &glow::Context| {
            let painter = ctx.layer_painter(LayerId::background());
            painter.rect_filled(background_fill_rect(), 0.0, background_color());
            painter.rect_filled(damage_paint_rect(), 0.0, damage_paint_color());
            FrameSignals {
                frame_damage: FrameDamage::Partial(vec![declared_damage_rect()]),
                band_range: 1..1,
                terminal_requested_delay: None,
            }
        })
    }

    /// Frame 2's determinism-control variant: identical to
    /// [`partial_frame_without_marker`] except it ALSO repaints the marker
    /// (still reporting `Partial`).
    fn partial_frame_with_marker() -> FrameFn<'static> {
        Box::new(|ctx: &egui::Context, _gl: &glow::Context| {
            let painter = ctx.layer_painter(LayerId::background());
            painter.rect_filled(background_fill_rect(), 0.0, background_color());
            painter.rect_filled(marker_rect(), 0.0, marker_color());
            painter.rect_filled(damage_paint_rect(), 0.0, damage_paint_color());
            FrameSignals {
                frame_damage: FrameDamage::Partial(vec![declared_damage_rect()]),
                band_range: 1..1,
                terminal_requested_delay: None,
            }
        })
    }

    /// Frame 2 of the age == 2 sequence: paints background + new content in
    /// `prev_damage_rect`, and declares THAT rect as `Partial` damage. Does
    /// NOT repaint the marker. This is "the previous frame" from the age
    /// == 2 frame's point of view -- its own declared damage must still be
    /// folded into a LATER frame's union when that later frame lands on a
    /// back buffer stale enough to have missed this one.
    fn partial_frame_prev_damage() -> FrameFn<'static> {
        Box::new(|ctx: &egui::Context, _gl: &glow::Context| {
            let painter = ctx.layer_painter(LayerId::background());
            painter.rect_filled(background_fill_rect(), 0.0, background_color());
            painter.rect_filled(prev_damage_rect(), 0.0, prev_damage_color());
            FrameSignals {
                frame_damage: FrameDamage::Partial(vec![declared_prev_rect()]),
                band_range: 1..1,
                terminal_requested_delay: None,
            }
        })
    }

    /// Frame 3 of the age == 2 sequence. Repaints background +
    /// `prev_damage_rect` (the SAME content [`partial_frame_prev_damage`]
    /// painted -- egui is immediate-mode, so a frame re-emits its whole
    /// current visual state every pass, not just what changed) + NEW
    /// content in `damage_paint_rect`. Declares `Partial` with ONLY its own
    /// new rect (`declared_damage_rect`) -- per `FrameDamage::Partial`'s
    /// contract, this is a truthful declaration precisely because nothing
    /// changed in `prev_damage_rect` relative to frame 2. At `age == 2`,
    /// `DamageHistory` must fold frame 2's OWN declared damage
    /// (`declared_prev_rect`) into this frame's redraw region even though
    /// this frame's own declaration never mentions it.
    fn partial_frame_age2_union() -> FrameFn<'static> {
        Box::new(|ctx: &egui::Context, _gl: &glow::Context| {
            let painter = ctx.layer_painter(LayerId::background());
            painter.rect_filled(background_fill_rect(), 0.0, background_color());
            painter.rect_filled(prev_damage_rect(), 0.0, prev_damage_color());
            painter.rect_filled(damage_paint_rect(), 0.0, damage_paint_color());
            FrameSignals {
                frame_damage: FrameDamage::Partial(vec![declared_damage_rect()]),
                band_range: 1..1,
                terminal_requested_delay: None,
            }
        })
    }

    /// **Pins the fix (124.18).** A taken partial present must leave every
    /// pixel outside the declared damage region byte-identical to the
    /// previous frame -- in particular `marker_rect`, which the declared
    /// damage region never touches and which frame 2 never repaints.
    ///
    /// Before 124.18, this test asserted the OPPOSITE on purpose (assertion
    /// 3 below was `assert_ne!`, with a comment explaining that a taken
    /// partial present painted the unclipped opaque chrome fill over the
    /// whole surface, wiping the marker): `paint_frame` painted head, band,
    /// and tail unclipped even when the clear was skipped, so the always
    /// present `CentralPanel`-equivalent background fill erased everything
    /// outside the declared rect. 124.18 makes `paint_frame` intersect
    /// every primitive's `clip_rect` with the redraw region before
    /// painting on a `Taken` frame, which is what turns this from a defect
    /// pin into a fix pin. This inversion is mandatory, not a rewrite of
    /// convenience -- see `PLAN_124_RENDER_EFFICIENCY.md`'s 124.18.
    #[test]
    fn a_taken_partial_present_preserves_pixels_outside_the_damage_region() {
        let Some(mut driver) = new_driver_or_skip(
            PartialPresentSupport::Supported,
            1,
            "partial-present-clipping",
        ) else {
            return;
        };

        let results = driver.run_frames(vec![baseline_frame(), partial_frame_without_marker()]);
        assert_eq!(results.len(), 2, "sanity: both frames painted");
        let baseline = &results[0];
        let taken = &results[1];

        // 1. The path we're testing actually fired. Without this, the test
        // proves nothing about `Taken` specifically -- a `BlockedBy*`
        // outcome would also paint everything again (via the full-clear
        // path) and could make assertion 3 pass for an unrelated reason.
        // At `age == 1` with no prior history, the reconstructed region is
        // exactly this frame's own declared rect.
        assert_eq!(
            taken.decision,
            PartialPresentDecision::Taken {
                age: 1,
                region: declared_damage_rect(),
            },
            "frame 2 must take the skip-clear + partial-present path for \
             this test to mean anything; got {:?}",
            taken.decision
        );

        // 2. The clear really was skipped.
        assert!(
            !taken.cleared,
            "a Taken frame must not have cleared the framebuffer"
        );

        // 3. THE FIX: the marker -- outside the declared damage region, and
        // never repainted by frame 2 -- is byte-identical to the baseline.
        // Before 124.18, `paint_frame`'s unclipped head/band/tail paint
        // overwrote it with the frame's own (identical-looking, but freshly
        // painted) chrome fill even though the clear was skipped.
        let (mx, my) = MARKER_SAMPLE;
        let baseline_pixel = baseline
            .readback
            .pixel(mx, my)
            .expect("marker pixel present in baseline readback");
        let after_pixel = taken
            .readback
            .pixel(mx, my)
            .expect("marker pixel present in frame-2 readback");
        assert_eq!(
            baseline_pixel, after_pixel,
            "a Taken frame must leave every pixel outside the declared \
             damage region byte-identical to the previous frame -- if this \
             fails, `paint_frame` is once again painting unclipped chrome \
             over a skipped clear"
        );

        // Diagnostic only (not asserted on): how much of the WHOLE surface
        // changed, not just the sampled marker pixel. Because the
        // background fill is byte-identical between the two frames (same
        // shape, same colour, deterministic paint -- see the sibling
        // determinism-control test) and is now clipped to the declared
        // damage region, the only pixels that differ from the baseline are
        // those genuinely inside the damage rect (256px = 16x16).
        let total_diff = baseline.readback.differing_pixels(&taken.readback);
        eprintln!(
            "partial-present-clipping: {total_diff}/{} pixels differ between \
             baseline and the Taken frame (declared damage rect is only \
             16x16 = 256 px)",
            CANVAS * CANVAS
        );
    }

    /// Determinism control for the test above. Without this, a passing
    /// defect test could just as easily mean the harness's own paint or
    /// readback path is nondeterministic between frames, rather than that
    /// the fill genuinely overwrote the marker. Here frame 2 DOES repaint
    /// the marker (still declaring `Partial`), so if painting the exact
    /// same shape at the exact same size, colour and position on two
    /// separate frames is deterministic, the sampled pixel must come back
    /// byte-identical both times.
    #[test]
    fn painting_the_marker_again_reproduces_it_exactly() {
        let Some(mut driver) =
            new_driver_or_skip(PartialPresentSupport::Supported, 1, "determinism-control")
        else {
            return;
        };

        let results = driver.run_frames(vec![baseline_frame(), partial_frame_with_marker()]);
        let baseline = &results[0];
        let repainted = &results[1];

        assert_eq!(
            repainted.decision,
            PartialPresentDecision::Taken {
                age: 1,
                region: declared_damage_rect(),
            },
            "sanity: same gate as the clipping test"
        );

        let (mx, my) = MARKER_SAMPLE;
        let baseline_pixel = baseline.readback.pixel(mx, my).expect("baseline marker");
        let repainted_pixel = repainted.readback.pixel(mx, my).expect("repainted marker");
        assert_eq!(
            baseline_pixel, repainted_pixel,
            "repainting the identical marker shape must reproduce it \
             byte-identically; if this fails, the rasteriser or the harness \
             is nondeterministic, which is a far larger bug than 124.18 -- \
             and would make the defect test above meaningless"
        );
    }

    /// Guards against a future "fix" that clips everything away (trivially
    /// making the clipping test above pass without actually repainting the
    /// damage region). On a `Taken` frame, the pixels INSIDE the declared
    /// damage region must reflect the new content that frame painted
    /// there.
    #[test]
    fn the_damage_region_itself_is_repainted_on_a_taken_frame() {
        let Some(mut driver) = new_driver_or_skip(
            PartialPresentSupport::Supported,
            1,
            "damage-region-repainted",
        ) else {
            return;
        };

        let results = driver.run_frames(vec![baseline_frame(), partial_frame_without_marker()]);
        let taken = &results[1];
        assert_eq!(
            taken.decision,
            PartialPresentDecision::Taken {
                age: 1,
                region: declared_damage_rect(),
            }
        );

        let (dx, dy) = DAMAGE_SAMPLE;
        let pixel = taken
            .readback
            .pixel(dx, dy)
            .expect("damage-region pixel present in frame-2 readback");
        let expected = damage_paint_color().to_srgba_unmultiplied();
        assert_eq!(
            pixel, expected,
            "the declared damage region must show the new content painted \
             into it this frame"
        );
    }

    /// **This is the case the whole subtask exists for (124.18).** Before
    /// this change, `age == 2` -- the common real-hardware case (124.17's
    /// GPU re-take measured a conventionally double-buffered surface
    /// reporting exactly this in steady state) -- was unconditionally
    /// `BlockedByBufferAge`, so partial present never fired on any shipped
    /// build. After this change it must be `Taken`, reconstructed as the
    /// union of this frame's own declared damage with the immediately
    /// previous frame's (`DamageHistory`).
    ///
    /// Sequence: a `Full` baseline (frame 1) establishes the marker; a
    /// `Partial` frame at `age == 1` (frame 2, no history contribution)
    /// declares and paints `prev_damage_rect`; a `Partial` frame at `age ==
    /// 2` (frame 3) declares ONLY its own new rect (`damage_paint_rect`)
    /// but -- being immediate-mode -- also repaints `prev_damage_rect`
    /// with the SAME content frame 2 established. Frame 3's own decision
    /// must fold frame 2's declared rect into its redraw region even
    /// though frame 3 never mentions it.
    #[test]
    fn a_frame_at_age_two_unions_with_the_immediately_previous_frames_damage() {
        let Some(mut driver) =
            new_driver_or_skip(PartialPresentSupport::Supported, 1, "age-two-union")
        else {
            return;
        };

        let baseline = driver.run_frames(vec![baseline_frame()])[0].clone();

        driver.set_surface_age(1);
        let frame2 = driver.run_frames(vec![partial_frame_prev_damage()])[0].clone();
        // Sanity: frame 2 itself took the narrow, no-history path -- if
        // this doesn't hold the rest of the test doesn't mean what it
        // claims to.
        assert_eq!(
            frame2.decision,
            PartialPresentDecision::Taken {
                age: 1,
                region: declared_prev_rect(),
            },
            "sanity: frame 2 must be Taken with its own rect only"
        );

        driver.set_surface_age(2);
        let taken = driver.run_frames(vec![partial_frame_age2_union()])[0].clone();

        // 1. The decision itself: `Taken`, reconstructed as the union of
        // frame 3's own rect with frame 2's -- NOT `BlockedByBufferAge`
        // (the pre-124.18 behaviour) and NOT narrowed to frame 3's own
        // rect alone (which would silently drop frame 2's contribution).
        assert_eq!(
            taken.decision,
            PartialPresentDecision::Taken {
                age: 2,
                region: declared_damage_and_prev_union(),
            }
        );

        // 2. The clear was skipped.
        assert!(
            !taken.cleared,
            "a Taken frame must not have cleared the framebuffer"
        );

        // 3. Preserves pixels outside the union: the marker was painted
        // only on the baseline and lies outside the union of
        // `prev_damage_rect`/`damage_paint_rect` on both axes checked
        // together (see the geometry comment above the rect helpers). If
        // the union were too permissive (e.g. a bug that always resolves
        // to the whole surface), this would fail exactly as the original
        // unclipped-chrome defect did.
        let (mx, my) = MARKER_SAMPLE;
        let baseline_pixel = baseline.readback.pixel(mx, my).expect("baseline marker");
        let after_pixel = taken.readback.pixel(mx, my).expect("frame-3 marker sample");
        assert_eq!(
            baseline_pixel, after_pixel,
            "a pixel outside the age-2 union must survive byte-identical"
        );

        // 4. Frame 3's OWN new content lands correctly. This is the
        // discriminating check against a coordinate-conversion bug (e.g. a
        // wrong physical<->points Y-flip): such a bug could shift the
        // applied clip away from where the union actually sits even if the
        // union's dimensions were computed correctly, which would leave
        // this sample showing stale (pre-frame-3) content instead.
        let (dx, dy) = DAMAGE_SAMPLE;
        let pixel = taken
            .readback
            .pixel(dx, dy)
            .expect("damage-region pixel present in frame-3 readback");
        assert_eq!(pixel, damage_paint_color().to_srgba_unmultiplied());

        // 5. Correctly repaints content that changed in the PREVIOUS
        // frame: frame 3 does not declare `prev_damage_rect` as its own
        // damage, but its repaint of it (immediate-mode) must still be
        // allowed to land, since `DamageHistory`'s union is exactly what
        // makes that safe on a 2-frames-stale buffer.
        let (px, py) = PREV_SAMPLE;
        let pixel = taken
            .readback
            .pixel(px, py)
            .expect("prev-damage pixel present in frame-3 readback");
        assert_eq!(pixel, prev_damage_color().to_srgba_unmultiplied());
    }
}
