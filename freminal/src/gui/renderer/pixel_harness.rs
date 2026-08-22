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
    /// The RGBA quadruple at `(x, y)`, top-left origin.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        let idx = (y as usize)
            .checked_mul(self.width as usize)?
            .checked_add(x as usize)?
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

/// Render `frame` into a fresh offscreen context and capture the result.
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
pub fn capture(frame: &SyntheticFrame) -> Result<PixelFrame, PixelHarnessError> {
    // The driver touches no GL in `new`, so it can be asked for the
    // viewport size before a context exists to size the pbuffer to match.
    let mut driver = HeadlessRenderer::new()?;
    let (vp_w, vp_h) = driver.viewport_px(frame);
    let (width, height) = match (u32::try_from(vp_w), u32::try_from(vp_h)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => return Err(PixelHarnessError::InvalidSize(vp_w, vp_h)),
    };

    let off = OffscreenGl::new(width, height)?;
    let gl = Gl::real(off.gl());

    driver.init(&gl)?;
    driver.draw_frame(&gl, frame);

    Ok(read_back(off.gl(), width, height))
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
    use super::super::pixel_harness::{PixelFrame, capture, renderer_string};

    /// Capture `frame`, or return `None` when no GL context exists.
    ///
    /// Skipping is deliberate: these tests need Mesa and a live `$DISPLAY`,
    /// which a developer outside the Linux dev shell will not have. A hard
    /// failure there would train people to ignore the suite. 123.13's CI job
    /// is what stops the skip path hiding a real break, because there a
    /// context is guaranteed.
    fn capture_or_skip(frame: &SyntheticFrame, what: &str) -> Option<PixelFrame> {
        match capture(frame) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!(
                    "SKIP {what}: {e}\n  (run under `xvfb-run -a cargo test -p freminal \
                     --features gl-pixel`)"
                );
                None
            }
        }
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
        let renderer = renderer_string().expect("renderer string after a successful capture");

        if update_requested() {
            write_golden(name, &captured, &renderer).expect("golden written");
            eprintln!("wrote golden {name} for renderer {renderer}");
            return;
        }

        match compare(name, &captured).expect("comparison ran") {
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
