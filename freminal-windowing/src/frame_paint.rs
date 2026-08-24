// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Painting one egui frame into a surface, including the decision to skip
//! the clear (124.17's skip-clear + partial-present gate).
//!
//! This is a separate module from `egui_integration` on purpose: that
//! module owns per-window state (`egui_winit::State`, the winit `Window`,
//! `FrameProfile`'s window-bound flush) and is therefore impossible to
//! drive from a test. This module owns exactly one concept -- painting a
//! single frame's shapes into a [`FrameSurface`] -- and is deliberately
//! window-free, so a test harness (124.19b's offscreen pixel harness) can
//! drive it against a pbuffer instead of a real `winit::Window`.
//!
//! [`paint_frame`] does NOT cover the buffer swap, `handle_platform_output`,
//! `pre_present_notify`, or anything else that needs a real `Window` --
//! those stay the caller's job in
//! [`EguiState::run_frame`](crate::egui_integration::EguiState::run_frame).

use crate::gl_context::GlState;

/// Whether the surface can present a damaged sub-region at all. A static
/// per-surface capability, probed once at surface creation
/// (`GlState::supports_partial_present`) and re-read every frame here —
/// cheap, since it is a stored `Option::is_some()` check, not a driver
/// round trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialPresentSupport {
    /// The surface can present only the damaged rectangles
    /// (`eglSwapBuffersWithDamage` is available).
    Supported,
    /// The surface must always present the whole buffer (extension absent,
    /// non-EGL backend, or an Apple platform with no EGL backend at all).
    Unsupported,
}

impl From<bool> for PartialPresentSupport {
    /// `GlState::supports_partial_present` returns a `bool` because it is a
    /// direct read of `Option::is_some()` over a probed EGL extension. It is
    /// converted to the named capability here, at the single point that
    /// reads it, so nothing downstream — [`decide_partial_present`], its
    /// tests, and the counters — ever handles a bare bool.
    ///
    /// Widening `GlState`'s own signature instead would put a present-path
    /// decision type into the GL-context module, which is the wrong home for
    /// it (`module-cohesion`), and would have to be duplicated across that
    /// function's two `cfg`-gated definitions.
    fn from(supported: bool) -> Self {
        if supported {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}

/// Why a frame did or did not take the skip-clear + partial-present path
/// (124.17). Exactly one variant per frame, so the derived
/// `frame-profiling` counters sum to the frame count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialPresentDecision {
    /// The app reported [`crate::FrameDamage::Full`].
    NotRequested,
    /// The app reported `Partial` with an empty rect list.
    RequestedWithNoRects,
    /// The app reported `Partial` with rects, but the surface cannot
    /// present a sub-region (damage extension absent, non-EGL backend, or
    /// an Apple platform).
    BlockedBySurface,
    /// The app reported `Partial` with rects and the surface supports
    /// partial present, but the back buffer does not hold the previous
    /// frame's contents (`buffer_age() != 1`).
    BlockedByBufferAge {
        /// The queried buffer age that failed the `== 1` check.
        age: u32,
    },
    /// Taken: the clear is skipped and only the damaged rects present.
    Taken,
}

/// 124.17: decide whether this frame may skip the full clear and present
/// only its damaged region. Pure and unit-testable — the single source of
/// truth the `run_frame` call site consumes for both the `partial` value it
/// has always computed and (feature-gated) the frame-profiling counters
/// that attribute every frame to exactly one [`PartialPresentDecision`].
///
/// `buffer_age` is taken **lazily** (`impl FnOnce() -> u32`, not `u32`):
/// today's gate short-circuits, so the EGL buffer-age query is never issued
/// on a `Full` frame, an empty-rect `Partial` frame, or a `Partial` frame
/// the surface cannot present a sub-region for. Taking it eagerly would add
/// a per-frame driver round trip to every frame in the program — a
/// behaviour change inside the very path this function only measures.
pub fn decide_partial_present(
    frame_damage: &crate::FrameDamage,
    support: PartialPresentSupport,
    buffer_age: impl FnOnce() -> u32,
) -> PartialPresentDecision {
    match frame_damage {
        crate::FrameDamage::Full => PartialPresentDecision::NotRequested,
        crate::FrameDamage::Partial(rects) => {
            if rects.is_empty() {
                PartialPresentDecision::RequestedWithNoRects
            } else if support == PartialPresentSupport::Unsupported {
                PartialPresentDecision::BlockedBySurface
            } else {
                let age = buffer_age();
                if age == 1 {
                    PartialPresentDecision::Taken
                } else {
                    PartialPresentDecision::BlockedByBufferAge { age }
                }
            }
        }
    }
}

/// The surface a frame is painted into, as [`paint_frame`] sees it.
///
/// Implemented by the windowed [`GlState`] in production and by the
/// offscreen pixel harness (124.19b) in tests. Deliberately does NOT cover
/// the buffer swap: swapping is window-bound (it needs
/// `Window::pre_present_notify` to run first, and the actual
/// `swap_buffers`/`swap_buffers_with_damage` call), and stays the caller's
/// job in `EguiState::run_frame`.
pub trait FrameSurface {
    /// The GL context this surface paints into. Handed to the caller's
    /// `ui_fn` so the app can issue its own GL calls (e.g. per-pane FBO
    /// paint callbacks) against the same context `egui_glow::Painter` uses.
    fn glow(&self) -> &glow::Context;

    /// Whether this surface can present a damaged sub-region at all — see
    /// [`PartialPresentSupport`].
    fn partial_present_support(&self) -> PartialPresentSupport;

    /// Age of the current back buffer. See `GlState::buffer_age`'s doc for
    /// the exact meaning of the returned value (`1` == "safe to treat as
    /// last frame").
    ///
    /// Deliberately NOT named `buffer_age`, to avoid colliding with
    /// `GlState`'s inherent method of that name — see [`Self::clear_to`].
    fn back_buffer_age(&self) -> u32;

    /// Clear the framebuffer to `color`.
    ///
    /// Deliberately NOT named `clear`. The impl for [`GlState`] delegates to
    /// that type's *inherent* `clear`/`buffer_age`, and those inherent
    /// methods now have no other callers in the crate. Were the names to
    /// collide, a same-named trait method would still compile (inherent
    /// methods win resolution) — but deleting the inherent one, which a
    /// dead-code sweep would be entitled to do since nothing else calls it,
    /// would silently rebind the call to this trait method and recurse
    /// forever. Distinct names make that deletion a compile error instead.
    /// Nothing in `cargo test --all` drives `GlState`, so the recursion
    /// would not be caught before shipping.
    fn clear_to(&self, color: [f32; 4]);
}

impl FrameSurface for GlState {
    fn glow(&self) -> &glow::Context {
        &self.glow_context
    }

    fn partial_present_support(&self) -> PartialPresentSupport {
        // `self.supports_partial_present()` resolves to `GlState`'s own
        // inherent method (inherent methods always take priority over a
        // same-named trait method), so this is a plain delegation, not
        // recursion into this trait method.
        PartialPresentSupport::from(self.supports_partial_present())
    }

    fn back_buffer_age(&self) -> u32 {
        self.buffer_age()
    }

    fn clear_to(&self, color: [f32; 4]) {
        self.clear(color);
    }
}

/// Per-frame inputs to [`paint_frame`] that are not generic over the
/// surface or UI-closure type parameters.
///
/// Bundled into one struct solely to keep `paint_frame`'s argument count
/// under clippy's `too_many_arguments` threshold — per
/// `freminal-extend-or-extract`'s guidance to introduce a named input
/// struct rather than reach for an `#[allow]`.
pub struct PaintFrameRequest<'a> {
    /// Window inner size in physical pixels, as the caller reads it via
    /// `Window::inner_size()` before calling [`paint_frame`] — this module
    /// is deliberately window-free, so it cannot read this itself.
    pub size_px: [u32; 2],
    /// Raw input collected from `egui-winit` for this frame.
    pub raw_input: egui::RawInput,
    /// GL clear color for this window.
    pub clear_color: [f32; 4],
    /// Shared flag through which the authoritative partial-present decision
    /// is published for this frame, mirroring
    /// `crate::App::present_partial_flag`.
    pub present_flag: Option<&'a std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Frame-profiling-only phase timings and repaint-cause data collected by
/// [`paint_frame`] (Task 121 harness), returned so the caller
/// (`EguiState::run_frame`) can fold them into its own `FrameProfile`
/// accumulators. `FrameProfile` itself stays in `egui_integration.rs`,
/// since it also owns window-bound phases (`swap`) this module never sees,
/// and the periodic flush, which needs the window's `WindowId`.
#[cfg(feature = "frame-profiling")]
pub struct PaintFrameProfiling {
    /// Wall-clock time inside `ctx.run_ui(...)` (which itself calls into
    /// `App::update`).
    pub run_ui: std::time::Duration,
    /// Wall-clock time across the band tessellation plus the head/tail
    /// chrome tessellation, summed as ONE span rather than split
    /// band-vs-head/tail — see the call site comment for why.
    pub tessellate: std::time::Duration,
    /// Wall-clock time across the three `paint_primitives` calls (head,
    /// band, tail), summed.
    pub paint: std::time::Duration,
    /// `ctx.repaint_causes()` entries collected immediately after `run_ui`
    /// returns, in iteration order — see the call site comment for why
    /// this must happen there and not later.
    pub repaint_causes: Vec<egui::RepaintCause>,
}

/// Everything
/// [`EguiState::run_frame`](crate::egui_integration::EguiState::run_frame)
/// needs after [`paint_frame`] returns, to finish the window-bound tail of
/// the frame: handing platform output back to `egui-winit`, swapping
/// buffers, deriving viewport commands/repaint delay, and folding profiling
/// data.
pub struct PaintFrameOutput {
    /// Platform output to hand to `egui_winit::State::handle_platform_output`.
    pub platform_output: egui::PlatformOutput,
    /// The full per-viewport output map — the caller reads
    /// `.get(&egui::ViewportId::ROOT)` out of this exactly as it read it out
    /// of `full_output.viewport_output` before this extraction.
    pub viewport_output: egui::OrderedViewportIdMap<egui::ViewportOutput>,
    /// The damaged rects to present, if the skip-clear + partial-present
    /// path was taken this frame.
    pub partial: Option<Vec<crate::DamageRect>>,
    /// Why this frame did or did not take the skip-clear + partial-present
    /// path.
    ///
    /// Gated because its only consumer today is the caller's frame-profiling
    /// counters; in a default build nothing reads it, so it is compiled out
    /// rather than carrying a dead-code suppression.
    ///
    /// 124.19b's offscreen pixel harness will be a second consumer, and must
    /// widen this `cfg` to
    /// `any(feature = "frame-profiling", feature = "gl-offscreen")` when it
    /// lands. It should assert on this rather than on `partial.is_some()` —
    /// the two are equivalent as predicates, but a failure reporting
    /// `BlockedByBufferAge { age: 0 }` names the cause, where `None` only
    /// says "not taken".
    #[cfg(feature = "frame-profiling")]
    pub decision: PartialPresentDecision,
    /// The delay the app itself requested via `ctx.request_repaint_after`
    /// during this frame's `update()`, if any.
    pub terminal_requested_delay: Option<std::time::Duration>,
    /// Frame-profiling-only phase timings — see [`PaintFrameProfiling`].
    #[cfg(feature = "frame-profiling")]
    pub profiling: PaintFrameProfiling,
}

/// Paint one egui frame into `surface`.
///
/// Runs the app's `ui_fn`, decides whether this frame may skip the clear
/// (124.17), tessellates and paints the head/band/tail split, and manages
/// egui's texture deltas. Deliberately does not touch anything window-bound
/// (`window.inner_size()`, `handle_platform_output`, `pre_present_notify`,
/// the buffer swap, `window.id()`) — those stay in
/// [`EguiState::run_frame`](crate::egui_integration::EguiState::run_frame),
/// the caller. This split is what lets 124.19b's offscreen pixel harness
/// drive the paint path against a pbuffer without a real `winit::Window`.
pub fn paint_frame<S, F>(
    surface: &S,
    ctx: &egui::Context,
    painter: &mut egui_glow::Painter,
    request: PaintFrameRequest<'_>,
    ui_fn: F,
) -> PaintFrameOutput
where
    S: FrameSurface,
    F: FnMut(&egui::Context, &glow::Context) -> crate::FrameSignals,
{
    let PaintFrameRequest {
        size_px,
        raw_input,
        clear_color,
        present_flag,
    } = request;
    let mut ui_fn = ui_fn;

    // egui 0.35 replaced `Context::run` (closure took `&Context`) with
    // `Context::run_ui` (closure takes the root `&mut Ui`).  Our `App`
    // trait still works in terms of `&Context`; `Ui` derefs to `Context`,
    // so deref explicitly rather than relying on a silent coercion.
    //
    // The closure both runs the app's `update` and returns this frame's
    // signals (damage report, terminal-band range, and the app's own
    // requested repaint delay); we capture them here to decide the
    // clear/present path and the head/band/tail split below. Running
    // both inside the one closure avoids two simultaneous `&mut app`
    // borrows in the caller.
    let mut frame_damage = crate::FrameDamage::Full;
    let mut band_range: std::ops::Range<usize> = 0..0;
    let mut terminal_requested_delay: Option<std::time::Duration> = None;
    // Task 121 frame-profiling harness: `run_ui` itself calls into
    // `ui_fn` (and therefore `App::update`), so this timing is an upper
    // bound on freminal's own per-frame `update()` cost as observed from
    // the windowing side.
    #[cfg(feature = "frame-profiling")]
    let run_ui_start = std::time::Instant::now();
    // `mut` because the texture-delta application below drains
    // `full_output.textures_delta` in place (egui 0.36 / #8356 — see the
    // comment at the drain site).
    let mut full_output = ctx.run_ui(raw_input, |root_ui| {
        let signals = ui_fn(&*root_ui, surface.glow());
        frame_damage = signals.frame_damage;
        band_range = signals.band_range;
        terminal_requested_delay = signals.terminal_requested_delay;
    });
    #[cfg(feature = "frame-profiling")]
    let run_ui_elapsed = run_ui_start.elapsed();

    // Task 121 defect-5 harness extension: aggregate
    // `ctx.repaint_causes()` into `repaint_cause_counts` -- answers
    // "something requested an immediate, zero-delay repaint; what,
    // exactly?" (egui-internal machinery vs. one of freminal's own
    // `ctx.request_repaint*` call sites in
    // `freminal/src/gui/terminal/widget.rs`).
    //
    // Called HERE, immediately after `run_ui` returns, because
    // `repaint_causes()` returns `prev_causes` -- the PREVIOUS pass's
    // causes (`Context::begin_pass`, invoked from inside `run_ui`,
    // swaps the just-finished pass's `causes` into `prev_causes` at the
    // START of the pass that follows it). This is the *earliest* point
    // in `run_frame` where that swap has already happened for THIS
    // frame's `run_ui` call, so it captures the freshest available data
    // (the causes from one frame ago) rather than calling later in
    // `run_frame` (same data, just read later for no benefit) or before
    // `run_ui` (this frame's `begin_pass` hasn't swapped yet, so it
    // would read causes from TWO frames ago instead of one). See the
    // `repaint_cause_counts` field doc for why a one-pass lag is fine
    // for this harness's aggregate-over-120-frames use, and would not
    // be for single-frame attribution.
    #[cfg(feature = "frame-profiling")]
    let repaint_causes: Vec<egui::RepaintCause> = ctx.repaint_causes();

    // Definitive `pixels_per_point` for THIS frame — read AFTER `run_ui`
    // has processed `raw_input` via `begin_pass`, so (unlike
    // `ppp_before_run_ui` above) this always reflects a scale-factor
    // change delivered this frame. Used for all tessellation below.
    let pixels_per_point = ctx.pixels_per_point();

    // ── 3-way paint-order split ──────────────────────────
    //
    // Slice `full_output.shapes` into head (chrome painted before the
    // terminal band — e.g. the `CentralPanel` background fill, menu
    // bar, tab bar), band (terminal content, rebuilt every frame), and
    // tail (chrome painted after the band — overlays, borders, modals)
    // by `band_range`, then tessellate and paint each slice separately,
    // in that order. `LayerId::background()`'s `PaintList` drains
    // first into `full_output.shapes`, and the band occupies a
    // contiguous range within it (see `App::take_terminal_band_range`),
    // so `band_range` is valid as an index range into `full_output.shapes`
    // directly.
    //
    // Clamp defensively: an app that reports a range referring to a
    // shape count larger than what actually drained (e.g. stale state)
    // must never panic on the slice below. `start` is clamped to the
    // shape count; `end` is then clamped to `[start, shape count]`, so
    // `start <= end <= shapes.len()` always holds.
    let shapes = full_output.shapes;
    let start = band_range.start.min(shapes.len());
    let end = band_range.end.clamp(start, shapes.len());
    let band_shapes: Vec<egui::epaint::ClippedShape> = shapes[start..end].to_vec();

    // Task 121 frame-profiling harness: time the ENTIRE tessellation
    // phase as one span (band tessellation plus head/tail tessellation)
    // rather than splitting them into separate counters -- one total,
    // sampled per-frame, is the more meaningful number to
    // threshold/alert on.
    #[cfg(feature = "frame-profiling")]
    let tessellate_start = std::time::Instant::now();

    // The band is tessellated from this frame's own shapes -- the
    // terminal band is rebuilt every frame.
    let band_primitives = ctx.tessellate(band_shapes, pixels_per_point);

    // Head/tail: re-tessellated from this frame's own shapes every
    // frame.
    let head_shapes: Vec<egui::epaint::ClippedShape> = shapes[..start].to_vec();
    let tail_shapes: Vec<egui::epaint::ClippedShape> = shapes[end..].to_vec();
    let head_primitives = ctx.tessellate(head_shapes, pixels_per_point);
    let tail_primitives = ctx.tessellate(tail_shapes, pixels_per_point);
    #[cfg(feature = "frame-profiling")]
    let tessellate_elapsed = tessellate_start.elapsed();

    // Decide whether this frame may skip the full clear and present only
    // its damaged region. This is a two-part gate:
    //   1. The app reports the frame as `Partial` (only the listed rects
    //      changed; everything else is identical to the previous frame).
    //   2. The back buffer still holds the previous frame's contents
    //      (`buffer_age() == 1`), and the surface can present a sub-region.
    // If either fails we fall back to the always-correct full path:
    // clear + full paint + full swap.
    //
    // 124.17: the gate itself lives in `decide_partial_present` (pure,
    // unit-tested) so it can be measured without changing what `partial`
    // evaluates to — see that function's doc for why `buffer_age` stays
    // a lazy closure here rather than a plain `u32`.
    let partial_present_decision =
        decide_partial_present(&frame_damage, surface.partial_present_support(), || {
            surface.back_buffer_age()
        });
    let partial = match (frame_damage, partial_present_decision) {
        (crate::FrameDamage::Partial(rects), PartialPresentDecision::Taken) => Some(rects),
        _ => None,
    };

    if partial.is_none() {
        surface.clear_to(clear_color);
    }

    // Publish the authoritative decision BEFORE the paint callbacks run
    // (they execute inside the `paint_primitives` calls below), so any
    // callback that scissors to the damage region gates on the same
    // value that decided whether the clear was skipped. Same-thread store
    // immediately before the reads -> `Relaxed` is sufficient.
    if let Some(flag) = present_flag {
        flag.store(partial.is_some(), std::sync::atomic::Ordering::Relaxed);
    }

    // Paint: set all textures, then three `paint_primitives` calls in
    // head -> band -> tail order, then free all textures. This is
    // exactly what `paint_and_update_textures` does internally (set
    // all -> paint -> free all), just split across three paint calls so
    // the band can be painted independently of chrome. Order matters:
    // `paint_primitives` re-establishes GL state (scissor/blend, unbound
    // VBO/EBO/texture/program) independently on every call, so three
    // sequential calls over a partition of the same shape list paint
    // identically to one call over the concatenation — head paints
    // first (e.g. the `CentralPanel` background fill, which must be
    // UNDER the band), then band, then tail (overlays/borders, which
    // must be OVER the band).
    //
    // egui 0.36 (upstream #8356) made `TexturesDelta` a drop-bomb: it
    // `debug_assert!`s that it is empty when dropped, and upstream's
    // `paint_and_update_textures` now `drain()`s both halves rather than
    // iterating them by reference. So these two loops must drain too —
    // iterating by reference would leave the delta populated and panic in
    // debug builds at the end of the frame. Draining also matches the
    // upstream contract that each delta is applied exactly once.
    //
    // `set` is a `HashMap<TextureId, SmallVec<[ImageDelta; 1]>>` as of
    // 0.36 (it was an ordered `Vec` before). Cross-texture ordering is
    // irrelevant — each texture is uploaded independently — but the
    // per-texture order within a `SmallVec` is significant (a whole upload
    // followed by partial patches of it), and `SmallVec`'s iteration
    // preserves it.
    for (id, image_deltas) in full_output.textures_delta.set.drain() {
        for image_delta in image_deltas {
            painter.set_texture(id, &image_delta);
        }
    }
    // Task 121 frame-profiling harness: time the three `paint_primitives`
    // calls, summed (a contiguous span across all three, since nothing
    // else runs between them, is equal to summing each individually).
    #[cfg(feature = "frame-profiling")]
    let paint_start = std::time::Instant::now();
    painter.paint_primitives(size_px, pixels_per_point, &head_primitives);
    painter.paint_primitives(size_px, pixels_per_point, &band_primitives);
    painter.paint_primitives(size_px, pixels_per_point, &tail_primitives);
    #[cfg(feature = "frame-profiling")]
    let paint_elapsed = paint_start.elapsed();
    for id in full_output.textures_delta.free.drain() {
        painter.free_texture(id);
    }

    PaintFrameOutput {
        platform_output: full_output.platform_output,
        viewport_output: full_output.viewport_output,
        partial,
        #[cfg(feature = "frame-profiling")]
        decision: partial_present_decision,
        terminal_requested_delay,
        #[cfg(feature = "frame-profiling")]
        profiling: PaintFrameProfiling {
            run_ui: run_ui_elapsed,
            tessellate: tessellate_elapsed,
            paint: paint_elapsed,
            repaint_causes,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{PartialPresentDecision, PartialPresentSupport, decide_partial_present};
    use crate::{DamageRect, FrameDamage};

    fn a_rect() -> DamageRect {
        DamageRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    #[test]
    fn not_requested_on_full_damage_and_never_queries_buffer_age() {
        let decision =
            decide_partial_present(&FrameDamage::Full, PartialPresentSupport::Supported, || {
                panic!("buffer_age() must not be queried on a Full frame")
            });
        assert_eq!(decision, PartialPresentDecision::NotRequested);
    }

    #[test]
    fn requested_with_no_rects_on_empty_partial_and_never_queries_buffer_age() {
        let decision = decide_partial_present(
            &FrameDamage::Partial(Vec::new()),
            PartialPresentSupport::Supported,
            || panic!("buffer_age() must not be queried when the rect list is empty"),
        );
        assert_eq!(decision, PartialPresentDecision::RequestedWithNoRects);
    }

    #[test]
    fn blocked_by_surface_when_unsupported_and_never_queries_buffer_age() {
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Unsupported,
            || panic!("buffer_age() must not be queried when the surface can't present partially"),
        );
        assert_eq!(decision, PartialPresentDecision::BlockedBySurface);
    }

    #[test]
    fn blocked_by_buffer_age_when_age_is_not_one() {
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            || 2,
        );
        assert_eq!(
            decision,
            PartialPresentDecision::BlockedByBufferAge { age: 2 }
        );
    }

    #[test]
    fn blocked_by_buffer_age_when_age_is_zero() {
        // Age 0 means "new or unknown" -- must NOT be treated as age 1.
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            || 0,
        );
        assert_eq!(
            decision,
            PartialPresentDecision::BlockedByBufferAge { age: 0 }
        );
    }

    #[test]
    fn taken_requires_all_three_conditions_together() {
        // All three hold: nonempty rects, surface supports partial present,
        // buffer age == 1.
        let taken = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            || 1,
        );
        assert_eq!(taken, PartialPresentDecision::Taken);

        // Drop each condition in turn -- each alone must prevent `Taken`.
        assert_ne!(
            decide_partial_present(&FrameDamage::Full, PartialPresentSupport::Supported, || 1),
            PartialPresentDecision::Taken,
            "Full damage alone must block Taken"
        );
        assert_ne!(
            decide_partial_present(
                &FrameDamage::Partial(Vec::new()),
                PartialPresentSupport::Supported,
                || 1
            ),
            PartialPresentDecision::Taken,
            "an empty rect list alone must block Taken"
        );
        assert_ne!(
            decide_partial_present(
                &FrameDamage::Partial(vec![a_rect()]),
                PartialPresentSupport::Unsupported,
                || 1
            ),
            PartialPresentDecision::Taken,
            "an unsupported surface alone must block Taken"
        );
        assert_ne!(
            decide_partial_present(
                &FrameDamage::Partial(vec![a_rect()]),
                PartialPresentSupport::Supported,
                || 2
            ),
            PartialPresentDecision::Taken,
            "a buffer age other than 1 alone must block Taken"
        );
    }

    #[test]
    fn partial_present_support_from_bool() {
        assert_eq!(
            PartialPresentSupport::from(true),
            PartialPresentSupport::Supported
        );
        assert_eq!(
            PartialPresentSupport::from(false),
            PartialPresentSupport::Unsupported
        );
    }
}
