// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Pixel capture for the Phase 2 harness (Task 123, subtask 123.12).
//!
//! One concept: rendering a [`SyntheticFrame`] into a real GL context and
//! getting the resulting pixels back. This is the half of the harness that
//! answers *what* was drawn; [`super::headless`] answers *how often* and
//! *how expensively*.
//!
//! `PROFILING.md` names the absence of this as freminal's single biggest
//! methodology gap: "a regression that changes what is drawn, rather than
//! how often, is undetectable in CI."
//!
//! # Requirements
//!
//! Mesa plus a live `$DISPLAY` — in practice the Linux `default` Nix dev
//! shell, under `xvfb-run`:
//!
//! ```sh
//! xvfb-run -a cargo test -p freminal --features gl-pixel
//! ```
//!
//! Every entry point returns [`PixelHarnessError::NoContext`] rather than
//! panicking when that is unavailable, so callers can skip cleanly.
//!
//! # What is rendered
//!
//! The same [`super::headless::HeadlessRenderer`] Phase 1 drives, but
//! against `Gl::real` instead of `Gl::recording` — so this exercises the
//! true `glow` path end to end: real shader compilation, real uploads, real
//! rasterisation. That reuse is deliberate. A separate rendering path for
//! pixels would have been a second thing to keep in sync with production,
//! and it would have made a pixel result and a call-count result
//! non-comparable.

use conv2::ConvUtil;
use freminal_windowing::gl_context_offscreen::OffscreenGl;
use glow::HasContext;

use super::gl_facade::Gl;
use super::headless::{HeadlessDriverError, HeadlessRenderer, SyntheticFrame};

/// Failures from capturing a frame.
#[derive(Debug, thiserror::Error)]
pub enum PixelHarnessError {
    /// No offscreen GL context could be created — almost always a missing
    /// `$DISPLAY` (not under `xvfb-run`) or absent Mesa. Callers should
    /// skip rather than fail.
    #[error("no offscreen GL context: {0}")]
    NoContext(#[from] freminal_windowing::Error),
    /// The headless driver itself failed (font loading, GL init).
    #[error("headless driver: {0}")]
    Driver(#[from] HeadlessDriverError),
    /// The frame's computed pixel size was not usable.
    #[error("invalid frame size: {0}x{1}")]
    InvalidSize(i32, i32),
}

/// A captured RGBA8 image, top-left origin.
///
/// GL's `read_pixels` uses a bottom-left origin; [`capture`] flips rows on
/// the way out so this type matches every image format and every human
/// expectation. Doing the flip once here rather than at each comparison
/// site is what keeps golden files viewable in an ordinary image viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PixelFrame {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8, row-major from the top.
    pub rgba: Vec<u8>,
}

impl PixelFrame {
    /// The RGBA quadruple at `(x, y)`, top-left origin, or `None` if either
    /// coordinate is outside the image.
    ///
    /// `x` is bounds-checked against `width` explicitly, and that check is
    /// load-bearing rather than defensive: without it, an out-of-range `x`
    /// still lands inside `rgba` and silently resolves to a pixel on a
    /// later row — `pixel(width, 0)` would return `(0, 1)`. A wrong pixel
    /// is worse than no pixel in a comparison harness.
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

    /// Count of pixels differing from `other` by more than `channel_bound`
    /// in any channel, or `None` if the two differ in dimensions.
    #[must_use]
    pub fn differing_pixels(&self, other: &Self, channel_bound: u8) -> Option<usize> {
        if self.width != other.width || self.height != other.height {
            return None;
        }
        let differing = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(other.rgba.as_chunks::<4>().0.iter())
            .filter(|(a, b)| {
                a.iter()
                    .zip(b.iter())
                    .any(|(x, y)| x.abs_diff(*y) > channel_bound)
            })
            .count();
        Some(differing)
    }
}

/// Shared setup for every pixel-capture entry point below: size the
/// viewport from `frame`, create a fresh offscreen GL context, wrap it in
/// a [`Gl`] just long enough to run `driver.init`, and hand back the
/// initialized driver alongside the still-live offscreen context.
///
/// This returns `(HeadlessRenderer, OffscreenGl, u32, u32)` rather than an
/// already-wrapped [`Gl`], because `Gl::real` borrows the [`OffscreenGl`]
/// it wraps (`Gl::real(off.gl())`) — a `Gl` tied to a local `off` cannot
/// outlive this function's return. Callers construct their own
/// `Gl::real(off.gl())` immediately after calling this, once `off` lives in
/// their own stack frame.
///
/// A new context per call is deliberate: it makes each capture independent,
/// so a test cannot be polluted by GL state a previous one left behind, and
/// it removes any ordering dependency between tests. Context creation under
/// llvmpipe is cheap enough that this is not worth optimising.
///
/// # Errors
///
/// [`PixelHarnessError::NoContext`] when no GL context is available (skip,
/// do not fail); [`PixelHarnessError::Driver`] for font or GL-init
/// failures; [`PixelHarnessError::InvalidSize`] for a degenerate frame.
fn setup_capture(
    frame: &SyntheticFrame,
) -> Result<(HeadlessRenderer, OffscreenGl, u32, u32), PixelHarnessError> {
    // The driver touches no GL in `new`, so it can be asked for the
    // viewport size before a context exists to size the pbuffer to match.
    let mut driver = HeadlessRenderer::new()?;
    let (vp_w, vp_h) = driver.viewport_px(frame);
    let (width, height) = match (u32::try_from(vp_w), u32::try_from(vp_h)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err(PixelHarnessError::InvalidSize(vp_w, vp_h)),
    };

    let off = OffscreenGl::new(width, height)?;
    {
        let gl = Gl::real(off.gl());
        driver.init(&gl)?;
    }

    Ok((driver, off, width, height))
}

/// Render `frame` into a fresh offscreen context and capture the result.
///
/// # Errors
///
/// [`PixelHarnessError::NoContext`] when no GL context is available (skip,
/// do not fail); [`PixelHarnessError::Driver`] for font or GL-init
/// failures; [`PixelHarnessError::InvalidSize`] for a degenerate frame.
pub fn capture(frame: &SyntheticFrame) -> Result<PixelFrame, PixelHarnessError> {
    let (mut driver, off, width, height) = setup_capture(frame)?;
    let gl = Gl::real(off.gl());

    driver.draw_frame(&gl, frame);

    Ok(read_back(off.gl(), width, height))
}

/// Render `frame` once, then `cursor_only_frames` further **cursor-only**
/// frames on the same renderer, and capture the last one.
///
/// Added by subtask 124.7. [`capture`] draws exactly one frame into a fresh
/// renderer, which cannot see any defect that only appears once a GPU buffer
/// is *reused* across frames — and buffer reuse is precisely what 124.7
/// introduced by letting a small decoration payload skip the orphan when the
/// slot's existing allocation is already large enough. The first upload into
/// each `deco_vbo` slot still orphans (it has to, to size the storage), so a
/// single-frame capture takes only the unchanged path.
///
/// Three cursor-only frames is the smallest count that exercises the gate on
/// both double-buffer slots and then again on the first, which is where a
/// stale-allocation bug would show.
///
/// # Errors
///
/// As [`capture`].
pub fn capture_after_cursor_only_frames(
    frame: &SyntheticFrame,
    cursor_only_frames: usize,
) -> Result<PixelFrame, PixelHarnessError> {
    let (mut driver, off, width, height) = setup_capture(frame)?;
    let gl = Gl::real(off.gl());

    driver.draw_frame(&gl, frame);
    for _ in 0..cursor_only_frames {
        driver.draw_cursor_only(&gl, frame);
    }

    Ok(read_back(off.gl(), width, height))
}

/// Draw `first` into a fresh offscreen context (unscissored), capture it,
/// then draw `second` on top with `SCISSOR_TEST` restricted to `scissor`.
///
/// The second draw lands on the SAME framebuffer -- no clear in between,
/// matching the production paint callback, where the clear is the
/// windowing layer's job and never this renderer's.
///
/// `scissor` is `(x, y, width, height)` in physical framebuffer pixels,
/// bottom-left origin, the same convention as `glScissor` and
/// [`freminal_windowing::DamageRect`].
///
/// Added for 124.23: proving the fix is exactly comparing the returned
/// `(before, after)` outside `scissor` -- any pixel that differs there is a
/// pixel the scissored second draw was not supposed to be able to touch.
///
/// # Errors
///
/// As [`capture`].
pub fn capture_scissored_overdraw(
    first: &SyntheticFrame,
    second: &SyntheticFrame,
    scissor: (i32, i32, i32, i32),
) -> Result<(PixelFrame, PixelFrame), PixelHarnessError> {
    let (mut driver, off, width, height) = setup_capture(first)?;
    let gl = Gl::real(off.gl());

    driver.draw_frame(&gl, first);
    let before = read_back(off.gl(), width, height);

    let (x, y, w, h) = scissor;
    unsafe {
        off.gl().enable(glow::SCISSOR_TEST);
        off.gl().scissor(x, y, w, h);
    }
    driver.draw_frame(&gl, second);
    unsafe {
        off.gl().disable(glow::SCISSOR_TEST);
    }

    let after = read_back(off.gl(), width, height);
    Ok((before, after))
}

/// Draw `frame` once with `SCISSOR_TEST` enabled and set to exactly the
/// full viewport, then capture the result.
///
/// Used only to prove the [`freminal_windowing::PresentRegion::Full`]
/// no-op property (124.23): a scissor box covering the entire viewport
/// clips nothing, so this must be pixel-identical to [`capture`], which
/// never touches `SCISSOR_TEST` at all -- the exact case the full-draw
/// paint arm takes today, before and after the 124.23 fix, in the
/// single-pane case.
///
/// # Errors
///
/// As [`capture`].
pub fn capture_with_full_viewport_scissor(
    frame: &SyntheticFrame,
) -> Result<PixelFrame, PixelHarnessError> {
    let (mut driver, off, width, height) = setup_capture(frame)?;
    let gl = Gl::real(off.gl());
    let vp_w = width.value_as::<i32>().unwrap_or(0);
    let vp_h = height.value_as::<i32>().unwrap_or(0);

    unsafe {
        off.gl().enable(glow::SCISSOR_TEST);
        off.gl().scissor(0, 0, vp_w, vp_h);
    }
    driver.draw_frame(&gl, frame);
    unsafe {
        off.gl().disable(glow::SCISSOR_TEST);
    }

    Ok(read_back(off.gl(), width, height))
}

/// Draw `left` and `right` as two panes side by side into ONE framebuffer
/// and capture the whole thing.
///
/// Added for 127.8. This mirrors how the application draws a split tab:
/// every pane renders into the shared window framebuffer through its own
/// GL viewport, set by the caller before the pane's draw (in production
/// the egui paint callback; here this function). The renderer itself
/// never sets a viewport -- it only passes the pane's pixel size to its
/// shaders as `u_viewport_size` -- so offsetting the GL viewport is all it
/// takes to place a second pane, and the same [`HeadlessRenderer`] (shared
/// atlas, shared buffers) draws both, as one `TerminalRenderer` does for a
/// window's panes.
///
/// Both frames must have identical pixel dimensions. The result is
/// `2 * pane_width` wide: `left` occupies `x in [0, pane_width)` and
/// `right` occupies `x in [pane_width, 2 * pane_width)`.
///
/// # Errors
///
/// As [`capture`], plus [`PixelHarnessError::InvalidSize`] when the two
/// frames differ in size.
pub fn capture_side_by_side(
    left: &SyntheticFrame,
    right: &SyntheticFrame,
) -> Result<PixelFrame, PixelHarnessError> {
    let mut driver = HeadlessRenderer::new()?;
    let (pane_w, pane_h) = driver.viewport_px(left);
    if driver.viewport_px(right) != (pane_w, pane_h) {
        let (rw, rh) = driver.viewport_px(right);
        return Err(PixelHarnessError::InvalidSize(rw, rh));
    }
    let (total_w, fb_height) = match (u32::try_from(pane_w), u32::try_from(pane_h)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w.saturating_mul(2), h),
        _ => return Err(PixelHarnessError::InvalidSize(pane_w, pane_h)),
    };

    let off = OffscreenGl::new(total_w, fb_height)?;
    let gl = Gl::real(off.gl());
    driver.init(&gl)?;

    unsafe {
        off.gl().viewport(0, 0, pane_w, pane_h);
    }
    driver.draw_frame(&gl, left);
    unsafe {
        off.gl().viewport(pane_w, 0, pane_w, pane_h);
    }
    driver.draw_frame(&gl, right);

    Ok(read_back(off.gl(), total_w, fb_height))
}

/// The GL renderer string of a freshly-created offscreen context.
///
/// Reported alongside any pixel result: a golden image is only meaningful
/// relative to the rasteriser that produced it, and llvmpipe's output can
/// shift between Mesa releases.
///
/// # Errors
///
/// [`PixelHarnessError::NoContext`] when no GL context is available.
pub fn renderer_string() -> Result<String, PixelHarnessError> {
    Ok(OffscreenGl::new(1, 1)?.renderer())
}

/// Read the current framebuffer back as RGBA8, flipping to a top-left
/// origin.
fn read_back(gl: &glow::Context, width: u32, height: u32) -> PixelFrame {
    let w = width.value_as::<usize>().unwrap_or(0);
    let h = height.value_as::<usize>().unwrap_or(0);
    let row_bytes = w.saturating_mul(4);
    let mut bottom_up = vec![0u8; row_bytes.saturating_mul(h)];

    unsafe {
        // `finish` rather than `flush`: readback must observe a completed
        // frame, and llvmpipe is asynchronous enough that flush alone can
        // return a partially-rendered buffer.
        gl.finish();
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            width.value_as::<i32>().unwrap_or(0),
            height.value_as::<i32>().unwrap_or(0),
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut bottom_up)),
        );
    }

    let mut rgba = Vec::with_capacity(bottom_up.len());
    for row in bottom_up.chunks_exact(row_bytes).rev() {
        rgba.extend_from_slice(row);
    }

    PixelFrame {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::headless::{CursorPresence, SyntheticFrame, ToastPresence};
    use super::super::pixel_golden::{
        GoldenComparison, compare, recorded_renderer, update_requested, write_golden,
    };
    use super::super::pixel_harness::{PixelFrame, PixelHarnessError, capture, renderer_string};

    /// Capture `frame`, returning `None` **only** when no GL context exists
    /// and we are not running in CI.
    ///
    /// Two distinctions matter here, and an earlier version of this helper
    /// collapsed both:
    ///
    /// 1. **Only [`PixelHarnessError::NoContext`] is skippable.** A `Driver`
    ///    error (font loading, shader compilation, GL init) or an
    ///    `InvalidSize` is a genuine failure, and treating it as "skip"
    ///    meant a broken renderer would report success. Those now panic.
    /// 2. **Where a context is *promised*, a missing one is a hard
    ///    failure.** 123.13's job guarantees Mesa and Xvfb, so a missing
    ///    context there is a broken runner and must fail loudly rather than
    ///    produce a false green.
    ///
    /// The second condition keys on `FREMINAL_REQUIRE_GL`, which only the
    /// `gl-pixel` workflow sets — **not** on `CI`. An earlier revision used
    /// `CI`, which broke the ordinary `Test ubuntu-latest` job: GitHub sets
    /// `CI=true` on every job, and `cargo xtask test` passes
    /// `--all-features`, so these tests run there too — on a runner that
    /// has neither Mesa nor Xvfb and was never meant to. "In CI" and "in
    /// the job that provides a GL stack" are different predicates, and
    /// conflating them turned a correct skip into six spurious failures.
    ///
    /// Everywhere else a missing context still skips, because a developer
    /// on macOS or outside the Linux dev shell cannot have one, and a suite
    /// that always fails for them is a suite they learn to ignore.
    pub(super) fn capture_or_skip(frame: &SyntheticFrame, what: &str) -> Option<PixelFrame> {
        match capture(frame) {
            Ok(f) => Some(f),
            Err(PixelHarnessError::NoContext(e)) => {
                assert!(
                    !gl_context_required(),
                    "{what}: no offscreen GL context ({e}) despite \
                     FREMINAL_REQUIRE_GL being set -- that variable means \
                     this environment promised Mesa and Xvfb, so this is a \
                     broken runner, not a reason to skip"
                );
                eprintln!(
                    "SKIP {what}: no GL context ({e})\n  (run under `xvfb-run -a \
                     cargo test -p freminal --features gl-pixel`)"
                );
                None
            }
            Err(other) => panic!("{what}: pixel harness failed: {other}"),
        }
    }

    /// Whether this environment has promised a working offscreen GL stack.
    ///
    /// Set only by `.github/workflows/gl-pixel.yml`. Deliberately not `CI`:
    /// every GitHub Actions job sets that, including ones with no GL stack
    /// that still run these tests via `cargo xtask test --all-features`.
    pub(super) fn gl_context_required() -> bool {
        std::env::var("FREMINAL_REQUIRE_GL").is_ok_and(|v| v != "0" && !v.is_empty())
    }

    /// `pixel` must reject out-of-range coordinates rather than silently
    /// returning a pixel from elsewhere in the image.
    ///
    /// Needs no GL context — it constructs a `PixelFrame` directly, so it
    /// runs everywhere. Regression test for the bug where `x` was not
    /// checked against `width`, so `pixel(width, 0)` returned `(0, 1)`.
    #[test]
    fn pixel_rejects_out_of_range_coordinates() {
        // 2x2, each pixel tagged with a distinct red channel.
        let frame = PixelFrame {
            width: 2,
            height: 2,
            rgba: vec![
                10, 0, 0, 255, // (0,0)
                20, 0, 0, 255, // (1,0)
                30, 0, 0, 255, // (0,1)
                40, 0, 0, 255, // (1,1)
            ],
        };

        assert_eq!(frame.pixel(0, 0), Some([10, 0, 0, 255]));
        assert_eq!(frame.pixel(1, 0), Some([20, 0, 0, 255]));
        assert_eq!(frame.pixel(0, 1), Some([30, 0, 0, 255]));
        assert_eq!(frame.pixel(1, 1), Some([40, 0, 0, 255]));

        // The bug: `x == width` used to wrap onto the next row and return
        // (0, 1) -- a plausible-looking but wrong pixel.
        assert_eq!(frame.pixel(2, 0), None, "x == width must be rejected");
        assert_eq!(frame.pixel(99, 0), None);
        assert_eq!(frame.pixel(0, 2), None, "y == height must be rejected");
        assert_eq!(frame.pixel(2, 2), None);
    }

    /// The stability check 123.12 requires **before** golden comparison is
    /// trusted at all: the same frame captured twice, on the same machine,
    /// must be bit-identical.
    ///
    /// This is what justifies the exact tolerance in `pixel_golden`'s policy.
    /// If it ever fails, the tolerance is not what should change — the
    /// nondeterminism is the bug, per `flaky-tests-are-bugs`.
    #[test]
    fn repeated_capture_is_bit_identical() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(first) = capture_or_skip(&frame, "stability") else {
            return;
        };
        let second = capture(&frame).expect("second capture after a successful first");

        assert_eq!(
            first.differing_pixels(&second, 0),
            Some(0),
            "two captures of the same frame must be bit-identical; if this \
             fails the rasteriser is nondeterministic and the tolerance is \
             NOT the thing to change"
        );
    }

    /// A rendered frame must actually contain rendered text.
    ///
    /// Guards the failure mode where the harness "passes" because it is
    /// capturing an empty framebuffer — which would make every golden
    /// comparison vacuously stable and the whole of Phase 2 worthless.
    #[test]
    fn a_captured_frame_contains_rendered_content() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(captured) = capture_or_skip(&frame, "content") else {
            return;
        };

        let lit = captured
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] | p[1] | p[2] != 0)
            .count();

        assert!(
            lit > 1000,
            "expected a substantial number of non-black pixels from a 40x10 \
             grid of text, got {lit}"
        );
    }

    /// Changing what is drawn must change the pixels — the property that
    /// makes golden comparison meaningful.
    #[test]
    fn visual_changes_are_visible_in_the_capture() {
        let base = SyntheticFrame::new(40, 10);
        let Some(shown) = capture_or_skip(&base, "cursor") else {
            return;
        };
        let hidden = capture(&base.with_cursor(CursorPresence::Hidden)).expect("hidden capture");
        let toasted = capture(&base.with_toast(ToastPresence::Present)).expect("toast capture");

        let cursor_delta = shown.differing_pixels(&hidden, 0).expect("same size");
        assert!(
            cursor_delta > 0,
            "hiding the cursor must change pixels, got no difference"
        );

        let toast_delta = shown.differing_pixels(&toasted, 0).expect("same size");
        assert!(
            toast_delta > cursor_delta,
            "a toast overlay must change more pixels than a cursor \
             ({toast_delta} vs {cursor_delta})"
        );
    }

    /// The golden mechanism itself, proved on one trivial scene.
    #[test]
    fn golden_round_trips_for_a_reference_frame() {
        let name = "reference_40x10";
        let frame = SyntheticFrame::new(40, 10);
        let Some(captured) = capture_or_skip(&frame, "golden") else {
            return;
        };
        check_or_update_golden(name, &captured);
    }

    /// Compare `captured` against the golden `name` (or regenerate it when
    /// `UPDATE_GOLDEN=1`), declining to compare across rasterisers.
    ///
    /// Shared by every golden test so the renderer-sidecar policy below
    /// lives in one place.
    pub(super) fn check_or_update_golden(name: &str, captured: &PixelFrame) {
        let renderer = renderer_string().expect("renderer string after a successful capture");

        if update_requested() {
            write_golden(name, captured, &renderer).expect("golden written");
            eprintln!("wrote golden {name} for renderer {renderer}");
            return;
        }

        // A pixel golden is only meaningful relative to the rasteriser that
        // produced it -- this module's own header says so. Before 123.C2 the
        // `default` dev shell exported `LIBGL_ALWAYS_SOFTWARE=1`, so every
        // local run was llvmpipe and matched the recorded golden by
        // accident. 123.C2 correctly confined that to the `gl-pixel` shell,
        // which left this test comparing an llvmpipe golden against whatever
        // GPU the developer has -- 9648/80000 pixels differ on a Radeon
        // 7900 XTX. Since the pre-commit hook runs `cargo test
        // --all-features`, that failure blocked EVERY commit on any machine
        // with a real GPU.
        //
        // So: decline to compare two images that were never comparable.
        // This is NOT a widened tolerance -- the comparison below is still
        // exact, at zero differing pixels. It is a guard on whether the
        // comparison means anything at all.
        //
        // The `gl_context_required()` split is load-bearing in the same way
        // it is in `capture_or_skip`. In the dedicated `gl-pixel` CI job
        // that variable is set and the renderer is pinned to llvmpipe by
        // `glPixelEnv`, so a mismatch THERE cannot be a developer's GPU --
        // it means the golden was regenerated under the wrong rasteriser,
        // which would otherwise silently skip this test in CI forever.
        // That must fail loudly, not skip.
        if let Some(recorded) = recorded_renderer(name)
            && recorded != renderer
        {
            assert!(
                !gl_context_required(),
                "golden {name} was captured under `{recorded}` but this \
                 environment renders with `{renderer}`, despite \
                 FREMINAL_REQUIRE_GL being set -- that variable means this \
                 is the pinned-llvmpipe gl-pixel job, so the golden was \
                 regenerated under the wrong rasteriser. Regenerate it \
                 under `.#gl-pixel`, not from the `default` shell."
            );
            eprintln!(
                "SKIP golden {name}: captured under `{recorded}`, this run \
                 renders with `{renderer}`.\n  A pixel golden is only \
                 meaningful against its own rasteriser. Compare it with:\n    \
                 nix develop .#gl-pixel --command xvfb-run -a cargo test -p \
                 freminal --features gl-pixel {name}"
            );
            return;
        }

        match compare(name, captured).expect("comparison ran") {
            GoldenComparison::Match => {}
            GoldenComparison::Missing { path } => {
                panic!(
                    "golden not found: {}\n  Create it with:\n    \
                     UPDATE_GOLDEN=1 xvfb-run -a cargo test -p freminal \
                     --features gl-pixel {name}",
                    path.display()
                );
            }
            GoldenComparison::Mismatch {
                path,
                differing,
                total,
            } => {
                let recorded = recorded_renderer(name).unwrap_or_else(|| "unknown".to_owned());
                panic!(
                    "{differing}/{total} pixels differ from {}\n  \
                     golden captured under: {recorded}\n  \
                     this run: {renderer}\n  \
                     If the renderers differ, this is a Mesa change, NOT a \
                     regression -- regenerate the golden and let the sidecar \
                     record the new version. Do NOT widen the tolerance.\n  \
                     Regenerate with:\n    UPDATE_GOLDEN=1 xvfb-run -a cargo \
                     test -p freminal --features gl-pixel {name}",
                    path.display()
                );
            }
            GoldenComparison::SizeMismatch { golden, actual } => {
                panic!(
                    "golden is {}x{} but the capture is {}x{} -- font metrics \
                     or grid size changed; regenerate deliberately",
                    golden.0, golden.1, actual.0, actual.1
                );
            }
        }
    }
}

#[cfg(test)]
mod wasted_work_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::headless::SyntheticFrame;
    use super::{capture, capture_after_cursor_only_frames};
    // Reuse the tests module's variant-aware guard rather than duplicating
    // it: an "any error is a skip" guard is exactly the bug this file was
    // reviewed for.

    /// The pixel-level proof that a repaint of unchanged state is **wasted
    /// work** — the finding Phase 1 could price but could not demonstrate.
    ///
    /// Phase 1 established that a frame in which nothing changed still costs
    /// a full clear and a full present (`frame_damage.rs`'s `Unchanged` ->
    /// `Full` fallback: no damage rect is pushed, so `rects.is_empty()`
    /// returns `Full`). What it could not show is that the *output* of that
    /// repaint is identical — only that the calls were made.
    ///
    /// This closes that. Rendering the same unchanged state twice produces
    /// byte-identical pixels. So the ~52 GL calls and ~200 KB of uploads a
    /// no-change frame pays produce, provably, not one different pixel.
    ///
    /// That is the empirical case for a third `FrameDamage` state meaning
    /// "nothing changed, present nothing" — see Task 124.2. Without a
    /// pixel harness this could only be argued from the code; with one it
    /// is measured.
    #[test]
    fn repainting_unchanged_state_produces_identical_pixels() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(first) = super::tests::capture_or_skip(&frame, "unchanged-repaint") else {
            return;
        };
        let second = capture(&frame).expect("second capture");

        assert_eq!(
            first.differing_pixels(&second, 0),
            Some(0),
            "a full repaint of unchanged state must produce identical \
             pixels; if it does not, the renderer is nondeterministic and \
             that is a far larger bug than the wasted work"
        );
    }

    /// The default background is never drawn, and the harness can see it.
    ///
    /// Task 34 established that the renderer deliberately skips cells with
    /// `DefaultBackground`, leaving those pixels untouched so the window's
    /// background (and any transparency) shows through. That has been an
    /// architectural claim with no direct verification.
    ///
    /// Measured on a 40x10 grid of dense text: **56,157 of 80,000 pixels
    /// (70%) are left fully transparent**, and only 1,762 are fully opaque.
    /// The skip is real and is doing most of the work of the frame.
    ///
    /// Worth pinning because it constrains Task 124: any change to damage
    /// tracking or partial presents has to preserve this. A "cheaper" path
    /// that started clearing to an opaque colour would silently break
    /// background transparency, and no call-count test would notice.
    #[test]
    fn default_background_cells_are_left_untouched() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(captured) = super::tests::capture_or_skip(&frame, "default-background") else {
            return;
        };

        let pixels = captured.rgba.as_chunks::<4>().0;
        let total = pixels.len();
        let transparent = pixels.iter().filter(|p| p[3] == 0).count();

        assert!(
            transparent * 2 > total,
            "most of a text frame should be untouched default background, \
             got {transparent} transparent of {total}"
        );
    }

    /// Subtask 124.7's correctness guard: reusing a decoration-buffer
    /// allocation instead of orphaning it must not change a single pixel.
    ///
    /// 124.7 lets `upload_deco_verts` skip the orphan when the payload is
    /// small and the slot is already big enough. The failure mode of getting
    /// that wrong is **silent visual corruption** — the issue #432 class —
    /// and no call-count test can see it, which is exactly why this subtask
    /// waited for Phase 2.
    ///
    /// The comparison is a cursor-only frame reached by reuse against the
    /// same state reached without it. `capture` draws one frame into a fresh
    /// renderer, so every upload in it orphans; `capture_after_cursor_only_frames`
    /// drives further cursor-only frames, and from the second onward the
    /// decoration upload takes the gated no-orphan path. Identical pixels at
    /// a channel bound of zero is the only acceptable result.
    #[test]
    fn reusing_a_decoration_allocation_changes_no_pixels() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(orphaned) = super::tests::capture_or_skip(&frame, "deco-orphan-baseline") else {
            return;
        };
        let reused =
            capture_after_cursor_only_frames(&frame, 3).expect("reused-allocation capture");

        assert_eq!(
            orphaned.differing_pixels(&reused, 0),
            Some(0),
            "skipping the decoration buffer's orphan must be invisible; any \
             difference here is the silent-corruption class 124.7 was \
             deliberately held back for a pixel harness to rule out"
        );
    }
}

/// Subtask 124.23's own tests: the full-draw paint arm now scissors to the
/// windowing-published `PresentRegion`, exactly as the cursor-only arm
/// already did. See `widget.rs`'s `draw_scissored_to_present_region` for
/// the production change.
///
/// Note on what this module deliberately does NOT attempt: the plan's
/// third, best-effort test -- reproducing a pane's semi-transparent quads
/// blending against stale (rather than cleared) pixels at
/// `background_opacity < 1.0` -- is not expressible here. This renderer
/// (`gpu.rs`'s `draw_with_verts` / `draw_with_cursor_only_update`) issues
/// no `glClear` call at all, in this file or in `headless.rs`; the clear
/// that the defect skips happens in the windowing layer, entirely outside
/// what `HeadlessRenderer` drives. There is consequently no "clear
/// happened" vs "clear was skipped" distinction this harness can even
/// pose, let alone assert on. Confirming that would require instrumenting
/// `freminal-windowing`, which is outside this subtask's scope. Separately,
/// `HeadlessRenderer::draw_frame` (`headless.rs`) hardcodes `bg_opacity` to
/// `1.0` and exposes no way to vary it from `SyntheticFrame` -- the knob
/// the reproduction needs also lives in a file outside this subtask's
/// scope (`freminal/src/gui/terminal/widget.rs` and this file only).
#[cfg(test)]
mod present_region_scissor_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::headless::{CursorPresence, SyntheticFrame};
    use super::{capture, capture_scissored_overdraw, capture_with_full_viewport_scissor};

    /// The core property 124.23 delivers: a scissored draw writes no
    /// pixels outside the scissor rect, even when the geometry it is asked
    /// to draw would otherwise have changed pixels there.
    ///
    /// The synthetic cursor is always drawn at cell (0, 0) (`headless.rs`'s
    /// `synthetic_cursor`: `col: 0.0, row: 0.0`), so hiding it changes pixels only
    /// within that top-left cell's column -- `first`'s cursor-shown draw
    /// and `second`'s cursor-hidden draw are pixel-identical everywhere
    /// else. That is confirmed here directly (the "control" check) rather
    /// than assumed, so a stale assumption about cursor placement fails
    /// loudly instead of silently making the main assertion vacuous.
    ///
    /// The scissored second draw is then restricted to every column
    /// *except* the leftmost one -- deliberately excluding the one region
    /// that would otherwise change. If the scissor mechanism confines the
    /// draw as it must, the leftmost column is left exactly as the first
    /// (cursor-shown) draw left it, even though the second draw's own data
    /// says the cursor should be gone there.
    #[test]
    fn scissored_full_draw_writes_no_pixels_outside_the_scissor_rect() {
        let cols = 20;
        let rows = 6;
        let shown = SyntheticFrame::new(cols, rows);
        let hidden = shown.with_cursor(CursorPresence::Hidden);

        let Some(shown_alone) = super::tests::capture_or_skip(&shown, "scissor-mechanism-control")
        else {
            return;
        };
        let hidden_alone = capture(&hidden).expect("unscissored control capture");

        let cols_u32 = u32::try_from(cols).expect("cols fits u32");
        let cell_w = shown_alone.width / cols_u32;
        assert!(cell_w > 0, "cell width must be positive, got {cell_w}");

        // Control: the cursor-visibility difference must be real, and must
        // land within the leftmost `cell_w` columns. Without this, the
        // "outside the scissor" region below could trivially match
        // regardless of whether the scissor mechanism works at all.
        let mut control_differs = false;
        'control: for y in 0..shown_alone.height {
            for x in 0..cell_w {
                if shown_alone.pixel(x, y) != hidden_alone.pixel(x, y) {
                    control_differs = true;
                    break 'control;
                }
            }
        }
        assert!(
            control_differs,
            "expected the cursor-visibility difference to land in the \
             leftmost column (cell (0, 0)); either the driver changed or \
             this test's assumption about cursor placement is stale"
        );

        let vp_w = i32::try_from(shown_alone.width).expect("width fits i32");
        let vp_h = i32::try_from(shown_alone.height).expect("height fits i32");
        let cell_w_i = i32::try_from(cell_w).expect("cell_w fits i32");

        // Allowed (scissored) region: every column except the leftmost.
        let (before, after) =
            capture_scissored_overdraw(&shown, &hidden, (cell_w_i, 0, vp_w - cell_w_i, vp_h))
                .expect("scissored overdraw capture");

        for y in 0..before.height {
            for x in 0..cell_w {
                assert_eq!(
                    before.pixel(x, y),
                    after.pixel(x, y),
                    "pixel ({x}, {y}) is outside the scissor rect and must \
                     be left untouched by the second (scissored) draw, but \
                     it changed -- the scissor failed to confine the write"
                );
            }
        }
    }

    /// Pins the scissor **convention** the 124.23 helper relies on: a box
    /// covering the entire viewport, expressed in physical framebuffer
    /// pixels with a bottom-left origin, must clip nothing.
    ///
    /// Read what this does and does not establish, because the distinction
    /// matters. `PresentRegion::Full` makes
    /// `draw_scissored_to_present_region` touch no GL state at all — it does
    /// **not** set a full-viewport scissor — so that arm's no-op property is
    /// structural and has nothing observable for a pixel test to compare.
    /// This test is therefore *not* a direct test of that arm.
    ///
    /// What it does catch is a wrong convention in the other arm. If the
    /// helper's rect were interpreted in logical points, or with a top-left
    /// origin, or off by the viewport offset, then a box nominally covering
    /// the whole viewport would clip part of it and this would fail. That is
    /// the assumption `PresentRegion::Region` silently depends on, and it is
    /// otherwise only exercised by geometry that happens to look plausible.
    #[test]
    fn a_full_viewport_scissor_box_clips_nothing() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(baseline) =
            super::tests::capture_or_skip(&frame, "full-viewport-scissor-convention")
        else {
            return;
        };
        let scissored =
            capture_with_full_viewport_scissor(&frame).expect("full-viewport-scissor capture");

        assert_eq!(
            baseline.differing_pixels(&scissored, 0),
            Some(0),
            "a scissor box covering the entire viewport must be \
             pixel-identical to no scissor at all -- if this fails, the \
             rect convention the PresentRegion scissor uses (physical \
             pixels, bottom-left origin) is wrong"
        );
    }
}

/// Subtask 124.14's own pixel test: the plan's mandated deliverable that a
/// `Region` frame and a `Full` frame of the same state produce identical
/// pixels ("124.14"'s "correctness argument that must be verified, not
/// assumed" in `PLAN_124_RENDER_EFFICIENCY.md`).
///
/// Read the module doc above `present_region_scissor_tests` before reading
/// this: this harness drives [`HeadlessRenderer`] directly and issues no
/// `glClear` anywhere, and `bg_opacity` is hardcoded to `1.0`. That is why
/// the test below is built the way it is, rather than actually invoking
/// two different `PaneFrameDamage` variants (there is nothing in this
/// renderer that reads that type at all -- it lives in `widget.rs` and
/// `frame_damage.rs`, entirely above this harness).
///
/// What IS expressible, and what the property in the plan actually reduces
/// to at this layer: [`capture_scissored_overdraw`] already models
/// "render once (the `Full` frame), then render again on the SAME,
/// never-cleared framebuffer restricted to a scissor rect (the `Region`
/// frame's bounded present)". A `Region` frame's whole safety argument is
/// that its content is byte-identical to what a `Full` rebuild of the same
/// state would produce -- `VertexRebuild::Bounded` runs the exact same
/// full-rebuild code as `VertexRebuild::ReevaluateFullRebuild`
/// (`frame_dirty.rs`'s doc on `VertexRebuild::Bounded`), so the vertices fed
/// to the second draw are identical to the first's. Passing the *same*
/// [`SyntheticFrame`] as both `first` and `second` reproduces exactly that:
/// "this frame's rebuild, presented in full" versus "this frame's
/// identical rebuild, presented through a bounded scissor" -- of the same
/// state, per the deliverable's own wording.
#[cfg(test)]
mod region_present_matches_full_present_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::super::headless::SyntheticFrame;
    use super::capture_scissored_overdraw;

    /// A `Region` present of unchanged state must be pixel-identical,
    /// everywhere in the framebuffer (not merely inside the scissor rect),
    /// to a `Full` present of that same state.
    ///
    /// Outside the scissor rect this is nearly definitional -- the second
    /// draw cannot touch those pixels at all, so they are left exactly as
    /// the first (`Full`) draw already rendered them, which is what
    /// `scissored_full_draw_writes_no_pixels_outside_the_scissor_rect`
    /// (`present_region_scissor_tests`) already pins for a *differing*
    /// second frame. The property this test adds is the part that is NOT
    /// definitional: *inside* the scissor rect, redrawing the identical
    /// content a second time (no clear in between, and this renderer
    /// enables no `GL_BLEND` anywhere -- verified by grep, not assumed)
    /// must not perturb a single channel. If it did -- e.g. some
    /// undiscovered blending or dithering path -- a `Region` frame would
    /// be observably different from a `Full` one even though nothing in
    /// the underlying state changed, which is exactly the silent
    /// corruption class 124.14's correctness argument exists to rule out.
    ///
    /// The rect is deliberately a genuine interior sub-region (not the
    /// full viewport, which `a_full_viewport_scissor_box_clips_nothing`
    /// already covers, and not edge-aligned, so an off-by-one in either
    /// bound would still land inside the checked area) -- a stand-in for
    /// one of `build_bounded_damage`'s row-range rects.
    #[test]
    fn a_region_present_of_unchanged_state_matches_a_full_present_everywhere() {
        let frame = SyntheticFrame::new(40, 10);
        let Some(baseline) =
            super::tests::capture_or_skip(&frame, "region-present-matches-full-present")
        else {
            return;
        };

        let vp_w = i32::try_from(baseline.width).expect("viewport width fits i32");
        let vp_h = i32::try_from(baseline.height).expect("viewport height fits i32");
        assert!(
            vp_w >= 4 && vp_h >= 4,
            "viewport too small ({vp_w}x{vp_h}) to carve a non-trivial \
             interior scissor rect out of it"
        );
        // An interior quarter of the viewport, offset from every edge.
        let region = (vp_w / 4, vp_h / 4, vp_w / 2, vp_h / 2);

        // `first` and `second` are the SAME frame: this stands in for "the
        // state did not change between the Full rebuild and the Region
        // rebuild", which is precisely the precondition that makes a
        // Region frame safe to bound in production (124.14a's correctness
        // argument: outside the rects, the previous frame's pixels remain
        // valid because nothing there changed).
        let (full, region_present) = capture_scissored_overdraw(&frame, &frame, region)
            .expect("scissored overdraw capture of identical state");

        assert_eq!(
            full.differing_pixels(&region_present, 0),
            Some(0),
            "a Region frame (bounded redraw of unchanged state) must be \
             pixel-identical to a Full frame of the same state, at every \
             pixel -- any difference here means bounding the presented \
             damage silently changed what got drawn, which is the exact \
             hazard 124.14's correctness argument exists to rule out"
        );
    }
}

/// Task 127.8: the focused pane's cursor is solid, a non-focused pane's is
/// a hollow block, and both appear in one framebuffer exactly as a split
/// tab draws them.
///
/// The scene is two panes side by side, each a 20x6 grid of the same text,
/// each with its cursor at cell (0, 0). The active (left) pane draws the
/// steady **block** DECSCUSR style so the solid-versus-hollow difference is
/// unambiguous; the inactive (right) pane draws [`CursorPresence::Hollow`].
///
/// Approach: both panes are drawn into ONE offscreen framebuffer by
/// [`capture_side_by_side`], which sets a per-pane GL viewport the way the
/// application's paint callbacks do. No composition-after-the-fact was
/// needed, because the renderer never sets a viewport itself.
#[cfg(test)]
mod unfocused_cursor_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use freminal_common::themes::CATPPUCCIN_MOCHA;

    use super::super::headless::{CursorPresence, SyntheticFrame};
    use super::{PixelFrame, capture, capture_side_by_side};

    const COLS: usize = 20;
    const ROWS: usize = 6;

    /// The theme's cursor colour as the opaque RGBA the renderer writes.
    fn cursor_rgba() -> [u8; 4] {
        let (r, g, b) = CATPPUCCIN_MOCHA.cursor;
        [r, g, b, 255]
    }

    fn active_pane() -> SyntheticFrame {
        SyntheticFrame::new(COLS, ROWS).with_cursor(CursorPresence::Shown)
    }

    fn inactive_pane() -> SyntheticFrame {
        SyntheticFrame::new(COLS, ROWS).with_cursor(CursorPresence::Hollow)
    }

    /// Both panes in one framebuffer, or `None` when this environment has
    /// no GL context (the shared guard decides skip versus fail).
    fn composite_or_skip(what: &str) -> Option<PixelFrame> {
        // The shared guard owns the skip/fail policy for a missing context;
        // probing with the active pane alone keeps that policy in one place.
        super::tests::capture_or_skip(&active_pane(), what)?;
        Some(
            capture_side_by_side(&active_pane(), &inactive_pane())
                .expect("side-by-side capture after a successful solo capture"),
        )
    }

    /// Assert that `frame` has the cursor colour at `(x, y)` and say which
    /// probe it was if not.
    fn assert_cursor_colour(frame: &PixelFrame, x: u32, y: u32, what: &str) {
        assert_eq!(
            frame.pixel(x, y),
            Some(cursor_rgba()),
            "{what} at ({x}, {y}) must be the cursor colour"
        );
    }

    /// Assert that `frame` does NOT have the cursor colour at `(x, y)`.
    fn assert_not_cursor_colour(frame: &PixelFrame, x: u32, y: u32, what: &str) {
        let px = frame.pixel(x, y);
        assert!(px.is_some(), "{what}: ({x}, {y}) is outside the image");
        assert_ne!(
            px,
            Some(cursor_rgba()),
            "{what} at ({x}, {y}) must NOT be the cursor colour"
        );
    }

    /// The focused pane's cursor is a solid block and the non-focused
    /// pane's is a hollow one: border pixels are cursor-coloured in both,
    /// but the interior is cursor-coloured only in the solid one.
    #[test]
    fn active_pane_is_solid_and_inactive_pane_is_hollow() {
        let Some(composite) = composite_or_skip("unfocused-cursor-asserts") else {
            return;
        };

        let pane_w = composite.width / 2;
        let cell_w = pane_w / u32::try_from(COLS).unwrap();
        let cell_h = composite.height / u32::try_from(ROWS).unwrap();
        assert!(
            cell_w >= 8 && cell_h >= 8,
            "cell {cell_w}x{cell_h} is too small for a border/centre probe"
        );
        let (mid_x, mid_y) = (cell_w / 2, cell_h / 2);

        // The synthetic grid puts a glyph ('a') in this very cell, and the
        // renderer draws text over the cursor without blending in this
        // harness (no GL_BLEND is enabled), so the glyph's rows -- roughly
        // the middle of the cell -- overwrite the cursor colour, including
        // the exact centre pixel and the cell's left and right edges in
        // those rows. Every probe below therefore sits in the glyph-free
        // upper quarter (`probe_y`) or bottom row, where only the cursor
        // itself has drawn.
        let probe_y = cell_h / 4;

        // Active (left) pane: a solid block fills the cell.
        assert_cursor_colour(&composite, mid_x, probe_y, "active pane upper centre");
        assert_cursor_colour(&composite, 0, probe_y, "active pane left edge");
        assert_cursor_colour(&composite, 1, probe_y, "active pane near-edge interior");
        assert_cursor_colour(&composite, mid_x, cell_h - 2, "active pane lower centre");

        // Inactive (right) pane: the same cell, offset by one pane width.
        let x0 = pane_w;
        assert_cursor_colour(&composite, x0, probe_y, "inactive pane left border");
        assert_cursor_colour(
            &composite,
            x0 + cell_w - 1,
            probe_y,
            "inactive pane right border",
        );
        assert_cursor_colour(&composite, x0 + mid_x, 0, "inactive pane top border");
        assert_cursor_colour(
            &composite,
            x0 + mid_x,
            cell_h - 1,
            "inactive pane bottom border",
        );
        assert_cursor_colour(&composite, x0, 0, "inactive pane top-left corner");
        assert_cursor_colour(
            &composite,
            x0 + cell_w - 1,
            cell_h - 1,
            "inactive pane bottom-right corner",
        );
        // The same upper-quarter row that is cursor-coloured at the border
        // is NOT cursor-coloured one pixel inside it, and neither is the
        // block's centre: that is the hollow-versus-solid difference.
        assert_not_cursor_colour(&composite, x0 + 1, probe_y, "inactive pane inside border");
        assert_not_cursor_colour(
            &composite,
            x0 + mid_x,
            probe_y,
            "inactive pane upper centre",
        );
        assert_not_cursor_colour(&composite, x0 + mid_x, mid_y, "inactive pane centre");

        // The hollow interior is not merely un-coloured by a glyph: the
        // whole interior (inside a one-pixel-thicker margin than the
        // border) must contain no cursor-coloured pixel at all.
        let margin = 3;
        for y in margin..cell_h - margin {
            for x in margin..cell_w - margin {
                assert_not_cursor_colour(&composite, x0 + x, y, "inactive pane interior");
            }
        }

        // Overall coverage: the solid block paints far more of its cell in
        // the cursor colour than the hollow one does, whatever the glyph
        // covers.
        let coverage = |x0: u32| {
            (0..cell_h)
                .flat_map(|y| (0..cell_w).map(move |x| (x0 + x, y)))
                .filter(|&(x, y)| composite.pixel(x, y) == Some(cursor_rgba()))
                .count()
        };
        let (solid, hollow) = (coverage(0), coverage(x0));
        assert!(
            hollow > 0 && solid > hollow * 2,
            "solid cursor coverage ({solid}) must exceed twice the hollow \
             cursor's ({hollow}), and the hollow cursor must draw something"
        );
    }

    /// Drawing two panes through per-pane viewports into one framebuffer
    /// must give each half exactly the pixels a lone capture of that pane
    /// would -- the property that makes this composite a faithful model of
    /// a split tab rather than a lookalike.
    #[test]
    fn each_half_of_the_composite_equals_a_solo_capture() {
        let Some(composite) = composite_or_skip("unfocused-cursor-halves") else {
            return;
        };
        let pane_w = composite.width / 2;

        for (frame, x0, what) in [
            (active_pane(), 0, "active (left)"),
            (inactive_pane(), pane_w, "inactive (right)"),
        ] {
            let solo = capture(&frame).expect("solo capture");
            assert_eq!(solo.width, pane_w, "{what}: solo width");
            assert_eq!(solo.height, composite.height, "{what}: solo height");
            for y in 0..solo.height {
                for x in 0..solo.width {
                    assert_eq!(
                        composite.pixel(x0 + x, y),
                        solo.pixel(x, y),
                        "{what} pane pixel ({x}, {y}) differs between the \
                         shared-framebuffer draw and a solo draw"
                    );
                }
            }
        }
    }

    /// The side-by-side capture is deterministic: repeated runs are
    /// bit-identical, which is what licenses the exact golden below.
    #[test]
    fn side_by_side_capture_is_bit_identical() {
        let Some(first) = composite_or_skip("unfocused-cursor-stability") else {
            return;
        };
        for _ in 0..2 {
            let again = capture_side_by_side(&active_pane(), &inactive_pane())
                .expect("repeat side-by-side capture");
            assert_eq!(
                first.differing_pixels(&again, 0),
                Some(0),
                "two side-by-side captures of the same scene must be \
                 bit-identical; the tolerance is NOT the thing to change"
            );
        }
    }

    /// The pixel golden for the same scene, so a change to either cursor's
    /// look (border thickness, colour, geometry) is a visible diff.
    #[test]
    fn solid_and_hollow_cursor_golden() {
        let Some(composite) = composite_or_skip("unfocused-cursor-golden") else {
            return;
        };
        super::tests::check_or_update_golden("cursor_active_solid_inactive_hollow", &composite);
    }
}
