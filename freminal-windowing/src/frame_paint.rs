// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Painting one egui frame into a surface, including the decision to skip
//! the clear (124.17's skip-clear + partial-present gate) and, when
//! [`crate::FrameDamage::None`] proves nothing changed at all, the decision
//! to skip shape partitioning, tessellation, the clear, AND every GL
//! primitive paint (Task 124.2). Texture-delta bookkeeping (egui 0.36's
//! drop-bomb contract) still runs unconditionally either way.
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

use std::collections::VecDeque;

use conv2::ConvUtil;

use crate::gl_context::GlState;
use crate::{DamageRect, FrameDamage};

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
    /// The app reported [`crate::FrameDamage::None`] (Task 124.2): nothing
    /// changed at all, so nothing is presented this frame — no clear, no
    /// GL primitive paint, no `pre_present_notify`, no buffer swap.
    /// Deliberately distinct from [`Self::NotRequested`] (which DOES
    /// present, just fully): a frame reaching this variant never queries
    /// buffer age and is never recorded into [`DamageHistory`], since no
    /// swap happens for it to describe.
    NoPresentation,
    /// The app reported [`crate::FrameDamage::Full`].
    NotRequested,
    /// The app reported `Partial` with an empty rect list.
    RequestedWithNoRects,
    /// The app reported `Partial` with rects, but the surface cannot
    /// present a sub-region (damage extension absent, non-EGL backend, or
    /// an Apple platform).
    BlockedBySurface,
    /// The app reported `Partial` with rects and the surface supports
    /// partial present, but no safe redraw region could be reconstructed
    /// for the queried buffer age — either the age is `0` (buffer contents
    /// unknown), or reconstructing it would require more history than
    /// [`DamageHistory`] retains or has recorded yet (see
    /// [`DamageHistory::redraw_region`]).
    BlockedByBufferAge {
        /// The queried buffer age that could not be safely reconstructed.
        age: u32,
    },
    /// Taken: the clear is skipped and only `region` presents.
    Taken {
        /// The queried buffer age this decision was taken for (`1` in the
        /// common single-generation case, `> 1` when [`DamageHistory`]
        /// reconstructed the redraw region from a stale buffer).
        age: u32,
        /// The region every unclipped primitive must be clipped to, and the
        /// region presented via `swap_buffers_with_damage`. The union of
        /// this frame's own declared damage with however many previous
        /// frames' damage the stale buffer requires (bounding-box, not
        /// multi-rect — see [`DamageHistory::redraw_region`]'s doc for why
        /// one rect is sufficient for v1).
        region: DamageRect,
    },
}

/// Bounded record of the last few *presented* frames' own declared
/// [`FrameDamage`], used by [`decide_partial_present`] to reconstruct the
/// redraw region a stale back buffer needs (124.18).
///
/// `EGL_EXT_buffer_age`'s `buffer_age() == n` means the back buffer's
/// contents are those of the frame `n` renders ago. Every pixel outside a
/// frame's own declared damage is byte-identical to the *previous* frame's
/// (that is [`FrameDamage::Partial`]'s whole contract), so the union of the
/// last `n` frames' own declared damage (this frame's, plus the previous
/// `n - 1`) is exactly the set of pixels that differ between the stale
/// buffer's contents and the frame about to be presented. A stored
/// [`FrameDamage::Full`] entry contributes "the whole surface changed" to
/// that union, since a full frame's own declared damage is, definitionally,
/// everything.
///
/// Retains [`Self::MAX_DEPTH`] entries, most-recent-last. `age` values
/// needing more history than that (or more than has been recorded since
/// the window/harness was created) are not reconstructable — see
/// [`Self::redraw_region`]'s doc for the resulting `None`.
#[derive(Debug, Default)]
pub struct DamageHistory {
    entries: VecDeque<FrameDamage>,
}

impl DamageHistory {
    /// How many past frames' own damage this history retains before
    /// evicting the oldest. 124.17's GPU re-take measured a conventionally
    /// double-buffered surface reporting `buffer_age() == 2` in steady
    /// state (never `1`, never `3` or higher, across ~250 queries in 21
    /// flush windows) — retaining 3 leaves headroom for one extra stale
    /// generation (e.g. triple buffering, or a transient stall) before
    /// falling back to a full frame.
    pub const MAX_DEPTH: usize = 3;

    /// A fresh, empty history — every window or test harness driver starts
    /// with none recorded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `damage` as the most recently presented frame's own declared
    /// damage, evicting the oldest entry once more than [`Self::MAX_DEPTH`]
    /// are held. Call once per frame that actually presents — regardless of
    /// which [`PartialPresentDecision`] a presenting frame reached, even one
    /// that fell back to a full present, since every presented frame's own
    /// damage is needed to reconstruct a LATER frame's union.
    ///
    /// Task 124.2: do NOT call this for a frame whose decision was
    /// [`PartialPresentDecision::NoPresentation`] — that frame never swaps,
    /// so it is not a "recently presented frame" this history describes at
    /// all, and recording it would let a later frame's buffer-age
    /// reconstruction assume a swap that never happened. See
    /// `ResolvedPartialPresent::resolve`'s call site.
    pub fn push(&mut self, damage: FrameDamage) {
        self.entries.push_back(damage);
        while self.entries.len() > Self::MAX_DEPTH {
            self.entries.pop_front();
        }
    }

    /// The redraw region required to safely treat a back buffer that is
    /// `age` frames stale as up to date, given `current`'s own declared
    /// damage for the frame about to present. `size_px` materialises a
    /// whole-surface [`DamageRect`] for any [`FrameDamage::Full`] entry
    /// folded into the union (this frame's own or a historical one).
    ///
    /// Returns `None` — the caller falls back to a full frame, unchanged
    /// from the pre-124.18 gate — when:
    /// - `age == 0`: the buffer's contents are unknown, never reconstructable.
    /// - `age` requires more of the previous `age - 1` frames than
    ///   [`Self::MAX_DEPTH`] retains, or than have been recorded yet (e.g.
    ///   the first few frames after window creation).
    #[must_use]
    pub fn redraw_region(
        &self,
        current: &FrameDamage,
        age: u32,
        size_px: [u32; 2],
    ) -> Option<DamageRect> {
        if age == 0 {
            return None;
        }
        // `age == 1` needs zero previous frames (just this one's own
        // damage); `age == n` needs the previous `n - 1`.
        let needed: usize = age.checked_sub(1)?.value_as().ok()?;
        if needed > Self::MAX_DEPTH || needed > self.entries.len() {
            return None;
        }
        let mut union = damage_bbox(current, size_px)?;
        // `entries` is most-recent-last; the previous `needed` frames are
        // the last `needed` entries, walked newest-first (order does not
        // matter for a commutative union, but this reads as "closest
        // history first").
        for entry in self.entries.iter().rev().take(needed) {
            if let Some(bbox) = damage_bbox(entry, size_px) {
                union = union_rect(union, bbox);
            }
        }
        Some(union)
    }
}

#[cfg(test)]
impl DamageHistory {
    /// Test-only: number of entries currently retained. Lets a test assert
    /// the eviction cap directly, independent of [`Self::redraw_region`]'s
    /// own `needed > Self::MAX_DEPTH` bound (which limits how far back a
    /// query looks regardless of how many entries are actually stored, and
    /// so cannot by itself distinguish "eviction works" from "eviction is a
    /// no-op but nothing ever asks past `MAX_DEPTH` anyway").
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The bounding box a [`FrameDamage`] contributes to a union: the whole
/// surface for [`FrameDamage::Full`], the bounding box of the rects for
/// [`FrameDamage::Partial`] (`None` for an empty rect list — an
/// empty-`Partial` frame changed nothing, so it must not force a wider
/// union than its neighbours warrant), and `None` for [`FrameDamage::None`]
/// for the same reason — a frame that changed nothing contributes nothing
/// to the union.
///
/// [`FrameDamage::None`] is never actually recorded into [`DamageHistory`]
/// (see [`DamageHistory::push`]'s doc), and [`decide_partial_present`]
/// short-circuits before ever reaching a call into
/// [`DamageHistory::redraw_region`] with it as `current` either — so this
/// arm exists only for exhaustiveness, not because a real `None` value is
/// expected to reach it.
fn damage_bbox(damage: &FrameDamage, size_px: [u32; 2]) -> Option<DamageRect> {
    match damage {
        FrameDamage::None => None,
        FrameDamage::Full => whole_surface_rect(size_px),
        FrameDamage::Partial(rects) => bbox_of_rects(rects),
    }
}

/// A [`DamageRect`] covering the entire surface at `size_px`, or `None` if
/// the physical size cannot be represented losslessly as `i32` (never
/// expected for a real window/framebuffer, but this module never panics
/// production code on a conversion — see `freminal-numeric-conversions`).
fn whole_surface_rect(size_px: [u32; 2]) -> Option<DamageRect> {
    let width: i32 = size_px[0].value_as().ok()?;
    let height: i32 = size_px[1].value_as().ok()?;
    Some(DamageRect {
        x: 0,
        y: 0,
        width,
        height,
    })
}

/// The bounding box of `rects`, or `None` if `rects` is empty.
fn bbox_of_rects(rects: &[DamageRect]) -> Option<DamageRect> {
    rects.iter().copied().reduce(union_rect)
}

/// The smallest [`DamageRect`] containing both `a` and `b`.
fn union_rect(a: DamageRect, b: DamageRect) -> DamageRect {
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

/// Convert a [`DamageRect`] (physical framebuffer pixels, bottom-left
/// origin) to an [`egui::Rect`] (logical points, top-left origin), for
/// intersecting against a [`egui::ClippedPrimitive::clip_rect`] (124.18).
///
/// `surface_height_px` is needed for the vertical flip: `DamageRect`
/// measures `y` up from the bottom, `egui::Rect` measures down from the
/// top. Returns `None` if any dimension cannot be losslessly converted to
/// `f32` — never expected for a real window, but this module never panics
/// production code on a conversion.
fn damage_rect_to_egui_rect(
    region: DamageRect,
    pixels_per_point: f32,
    surface_height_px: u32,
) -> Option<egui::Rect> {
    let ppp = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    let height_px: f32 = surface_height_px.approx_as().ok()?;
    let x: f32 = region.x.approx_as().ok()?;
    let y: f32 = region.y.approx_as().ok()?;
    let width: f32 = region.width.approx_as().ok()?;
    let height: f32 = region.height.approx_as().ok()?;

    let min_x = x / ppp;
    let max_x = (x + width) / ppp;
    // Flip: `DamageRect`'s `y` is measured up from the bottom; egui's
    // `Rect` is measured down from the top.
    let top_px = height_px - (y + height);
    let bottom_px = height_px - y;
    let min_y = top_px / ppp;
    let max_y = bottom_px / ppp;

    Some(egui::Rect::from_min_max(
        egui::Pos2::new(min_x, min_y),
        egui::Pos2::new(max_x, max_y),
    ))
}

/// 124.17/124.18: decide whether this frame may skip the full clear and
/// present only a redraw region reconstructed from `history` plus this
/// frame's own damage. Pure and unit-testable — the single source of truth
/// the `run_frame` call site consumes for both the `partial` value it has
/// always computed and (feature-gated) the frame-profiling counters that
/// attribute every frame to exactly one [`PartialPresentDecision`].
///
/// `buffer_age` is taken **lazily** (`impl FnOnce() -> u32`, not `u32`):
/// the gate short-circuits, so the EGL buffer-age query is never issued on
/// a `None` frame (Task 124.2), a `Full` frame, an empty-rect `Partial`
/// frame, or a `Partial` frame the surface cannot present a sub-region for.
/// Taking it eagerly would add a per-frame driver round trip to every frame
/// in the program — a behaviour change inside the very path this function
/// only measures.
///
/// Does NOT record `frame_damage` into `history` — the caller does that
/// (see [`DamageHistory::push`]'s doc for why: this frame's own damage must
/// still be visible to a LATER frame's reconstruction even when this
/// frame's own decision fell back to a full present, and why a
/// [`PartialPresentDecision::NoPresentation`] frame must NOT be recorded at
/// all).
pub fn decide_partial_present(
    frame_damage: &FrameDamage,
    support: PartialPresentSupport,
    history: &DamageHistory,
    size_px: [u32; 2],
    buffer_age: impl FnOnce() -> u32,
) -> PartialPresentDecision {
    match frame_damage {
        FrameDamage::None => PartialPresentDecision::NoPresentation,
        FrameDamage::Full => PartialPresentDecision::NotRequested,
        FrameDamage::Partial(rects) => {
            if rects.is_empty() {
                PartialPresentDecision::RequestedWithNoRects
            } else if support == PartialPresentSupport::Unsupported {
                PartialPresentDecision::BlockedBySurface
            } else {
                let age = buffer_age();
                history.redraw_region(frame_damage, age, size_px).map_or(
                    PartialPresentDecision::BlockedByBufferAge { age },
                    |region| PartialPresentDecision::Taken { age, region },
                )
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
    /// the exact meaning of the returned value (`0` == unusable; `n >= 1`
    /// == reconstructable via [`DamageHistory`]).
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

    /// Clear only `region` of the framebuffer to `color`, leaving every
    /// pixel outside it untouched (124.20).
    ///
    /// Used on a `Taken` frame instead of [`Self::clear_to`]: a partial
    /// present's whole promise is that everything outside `region` is
    /// unchanged from the previous frame, so a full clear there would
    /// erase content the redraw never repaints — but skipping the clear
    /// *inside* `region` entirely (124.17/124.18's original behaviour) is
    /// also wrong, because a `DefaultBackground` terminal cell or a
    /// `background_opacity < 1.0` chrome fill deliberately paints no
    /// opaque quad there and needs the clear color underneath it, not
    /// whatever a previous, unrelated frame left behind.
    ///
    /// `region` is in physical framebuffer pixels, bottom-left origin —
    /// the same convention [`crate::DamageRect`] documents and `glScissor`
    /// uses directly, so no coordinate flip is needed.
    ///
    /// Deliberately NOT named `clear_scissored`, for the same reason
    /// [`Self::clear_to`] is not named `clear` — see that method's doc.
    fn clear_region_to(&self, color: [f32; 4], region: DamageRect);
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

    fn clear_region_to(&self, color: [f32; 4], region: DamageRect) {
        self.clear_scissored(color, region);
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
    /// Shared cell through which the authoritative [`crate::PresentRegion`]
    /// is published for this frame, mirroring
    /// `crate::App::present_partial_flag`.
    pub present_flag: Option<&'a std::sync::Arc<std::sync::Mutex<crate::PresentRegion>>>,
    /// This window's (or test harness driver's) record of recent frames'
    /// own declared damage, consumed and updated by [`paint_frame`] to
    /// reconstruct the redraw region a stale back buffer needs — see
    /// [`DamageHistory`] (124.18). Owned by the caller (`EguiState` in
    /// production, `HarnessDriver` in the offscreen test harness) because
    /// it must persist across frames, which this deliberately window-free,
    /// per-frame function cannot do itself.
    pub damage_history: &'a mut DamageHistory,
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
    /// band-vs-head/tail — see the call site comment for why. Exactly
    /// [`Duration::ZERO`](std::time::Duration::ZERO) when this frame's
    /// presentation was [`crate::FrameDamage::None`] (Task 124.2): no
    /// tessellation phase ran to measure.
    pub tessellate: std::time::Duration,
    /// Wall-clock time across the three `paint_primitives` calls (head,
    /// band, tail), summed. Exactly
    /// [`Duration::ZERO`](std::time::Duration::ZERO) when this frame's
    /// presentation was [`crate::FrameDamage::None`] (Task 124.2): no
    /// `paint_primitives` call ran to measure.
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
    /// What this frame decided to actually do to the framebuffer and swap
    /// chain (Task 124.2). Replaces the earlier `partial: Option<DamageRect>`
    /// — a bare `None` there meant "the full clear+paint+swap path was
    /// taken", which became ambiguous with the newly possible "nothing was
    /// presented at all" case once [`crate::FrameDamage::None`] exists.
    /// [`EguiState::run_frame`](crate::egui_integration::EguiState::run_frame)
    /// matches this to decide whether to skip `pre_present_notify` and the
    /// swap entirely, swap the whole surface, or swap only a damaged region.
    pub presentation: FramePresentation,
    /// Why this frame did or did not take the skip-clear + partial-present
    /// path.
    ///
    /// Gated because its consumers are the caller's frame-profiling counters
    /// and (as of 124.19b) the offscreen frame-paint harness's tests; in a
    /// default build nothing reads it, so it is compiled out rather than
    /// carrying a dead-code suppression.
    ///
    /// 124.19b's offscreen pixel harness (`frame_paint_harness.rs`) is that
    /// second consumer, which is why this `cfg` is
    /// `any(feature = "frame-profiling", feature = "gl-offscreen")` rather
    /// than `frame-profiling` alone. Assert on this rather than on
    /// `presentation` directly -- a failure reporting
    /// `BlockedByBufferAge { age: 0 }` names the cause, where
    /// `FramePresentation::Full` only says "the full path was taken", not
    /// why.
    #[cfg(any(feature = "frame-profiling", feature = "gl-offscreen"))]
    pub decision: PartialPresentDecision,
    /// The delay the app itself requested via `ctx.request_repaint_after`
    /// during this frame's `update()`, if any.
    pub terminal_requested_delay: Option<std::time::Duration>,
    /// Frame-profiling-only phase timings — see [`PaintFrameProfiling`].
    #[cfg(feature = "frame-profiling")]
    pub profiling: PaintFrameProfiling,
}

/// What [`paint_frame`] decided to actually do with the framebuffer and
/// swap chain this frame (Task 124.2) — the resolution of
/// [`PartialPresentDecision`] into the three concrete actions
/// [`EguiState::run_frame`](crate::egui_integration::EguiState::run_frame)
/// can take.
///
/// Replaces the pre-124.2 `Option<DamageRect>` (`None` meant "clear the
/// whole surface, paint everything unclipped, swap fully" — the ordinary
/// `Full` case). That encoding stopped being sound once a frame could
/// legitimately do NOTHING: a bare `None` could no longer say whether the
/// caller should still clear+paint+swap fully, or skip all three. Per
/// `freminal-state-representation`, the fix is a named domain enum, not a
/// second bool bolted onto the existing `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePresentation {
    /// Nothing changed this frame ([`crate::FrameDamage::None`]): no clear,
    /// no GL primitive paint, no `pre_present_notify`, no buffer swap. Not
    /// a presented frame — must not be recorded into [`DamageHistory`] (see
    /// that type's doc).
    None,
    /// The whole surface was cleared and must be redrawn and presented via
    /// a full `swap_buffers`.
    Full,
    /// Only this region was cleared/redrawn; present via
    /// `swap_buffers_with_damage` restricted to it. The same region every
    /// unclipped primitive was clipped to before painting (see
    /// [`DamageHistory::redraw_region`]'s doc for why a single bounding box
    /// is sufficient for v1).
    Partial(DamageRect),
}

/// Everything [`paint_frame`] needs from the 124.17/124.18/124.2
/// skip-clear-plus-partial-present gate for one frame: the decision itself,
/// the [`FramePresentation`] it resolves to, and the redraw region already
/// converted to an egui clip rect (`None` unless `presentation` is
/// [`FramePresentation::Partial`]).
///
/// Extracted out of [`paint_frame`] purely to keep that function under
/// clippy's `too_many_lines` threshold, per `freminal-extend-or-extract` —
/// not because this logic is reusable elsewhere.
struct ResolvedPartialPresent {
    /// Only read by [`Self::reported_decision`], which is `cfg`-gated
    /// identically -- so this field is too, rather than existing unread
    /// (and warning as dead code) in a default build.
    #[cfg(any(feature = "frame-profiling", feature = "gl-offscreen"))]
    decision: PartialPresentDecision,
    presentation: FramePresentation,
    redraw_clip: Option<egui::Rect>,
}

impl ResolvedPartialPresent {
    /// Resolve the gate for one frame, recording `frame_damage` into
    /// `history` as a side effect for every decision EXCEPT
    /// [`PartialPresentDecision::NoPresentation`] (Task 124.2) — see
    /// [`DamageHistory::push`]'s doc for why a frame that never presents
    /// must not be recorded as if it had.
    fn resolve<S: FrameSurface>(
        surface: &S,
        frame_damage: FrameDamage,
        history: &mut DamageHistory,
        size_px: [u32; 2],
        pixels_per_point: f32,
    ) -> Self {
        let decision = decide_partial_present(
            &frame_damage,
            surface.partial_present_support(),
            history,
            size_px,
            || surface.back_buffer_age(),
        );
        if !matches!(decision, PartialPresentDecision::NoPresentation) {
            history.push(frame_damage);
        }

        // `partial` and `redraw_clip` are downgraded to `None` together if
        // the region can't be converted to an egui clip rect: painting
        // unclipped chrome while having already decided to skip the clear
        // is exactly the defect 124.18 fixed, so there is no safe way to
        // have one without the other.
        let region_from_decision = match decision {
            PartialPresentDecision::Taken { region, .. } => Some(region),
            _ => None,
        };
        let redraw_clip = region_from_decision
            .and_then(|region| damage_rect_to_egui_rect(region, pixels_per_point, size_px[1]));
        let partial = redraw_clip.and(region_from_decision);

        let presentation = if matches!(decision, PartialPresentDecision::NoPresentation) {
            FramePresentation::None
        } else {
            partial.map_or(FramePresentation::Full, FramePresentation::Partial)
        };

        Self {
            #[cfg(any(feature = "frame-profiling", feature = "gl-offscreen"))]
            decision,
            presentation,
            redraw_clip,
        }
    }

    /// The decision to actually report to callers (frame-profiling
    /// counters, the offscreen harness): stays truthful about what
    /// happened even when [`Self::resolve`] downgraded `presentation` back
    /// to [`FramePresentation::Full`] after a clip-conversion failure -- a
    /// `Taken` decision would be a lie in that case, since this frame took
    /// the full-clear path exactly as if `redraw_region` itself had
    /// returned `None`. [`FramePresentation::None`] is never downgraded --
    /// [`PartialPresentDecision::NoPresentation`] is always accurate.
    #[cfg(any(feature = "frame-profiling", feature = "gl-offscreen"))]
    const fn reported_decision(&self) -> PartialPresentDecision {
        if matches!(self.presentation, FramePresentation::Partial(_)) {
            self.decision
        } else if let PartialPresentDecision::Taken { age, .. } = self.decision {
            PartialPresentDecision::BlockedByBufferAge { age }
        } else {
            self.decision
        }
    }
}

/// Clear the framebuffer for a presenting (`Full`/`Partial`) frame, clip
/// every primitive to the redraw region (a no-op on `Full`, since
/// `redraw_clip` is `None` then), and publish the authoritative
/// [`crate::PresentRegion`] before the paint callbacks run.
///
/// Only ever called when `presentation` is NOT [`FramePresentation::None`]
/// -- [`paint_frame`] gates the call on `should_paint`. Extracted purely to
/// keep that function under clippy's `too_many_lines` threshold, per
/// `freminal-extend-or-extract`; the three primitive lists are bundled into
/// one `[&mut Vec<_>; 3]` array parameter (rather than three positional
/// parameters) to stay under `too_many_arguments` as well.
fn clear_clip_and_publish<S: FrameSurface>(
    surface: &S,
    clear_color: [f32; 4],
    presentation: FramePresentation,
    redraw_clip: Option<egui::Rect>,
    present_flag: Option<&std::sync::Arc<std::sync::Mutex<crate::PresentRegion>>>,
    primitives: [&mut Vec<egui::ClippedPrimitive>; 3],
) {
    // 124.20: a `Partial` frame's redraw region must be cleared, not
    // skipped -- skipping it entirely left a `DefaultBackground` terminal
    // cell or a `background_opacity < 1.0` chrome fill (both of which
    // deliberately paint no opaque quad) blending against whatever a
    // stale, unrelated previous frame left in the framebuffer instead of
    // against `clear_color`. Clearing the WHOLE surface on a `Partial`
    // frame would be just as wrong the other way -- it would erase content
    // outside `region` that this frame never repaints -- so the clear is
    // confined to exactly the region already computed for clipping below.
    // `FramePresentation::None` never reaches this function (the caller's
    // `should_paint` gate), so the fallback-to-`clear_to` half of this
    // match is reached only by `Full`.
    match presentation {
        FramePresentation::Partial(region) => surface.clear_region_to(clear_color, region),
        FramePresentation::Full | FramePresentation::None => {
            surface.clear_to(clear_color);
        }
    }

    // 124.18: a partial present means only `redraw_clip`'s pixels may
    // change, so every unclipped primitive -- head, band, and tail alike --
    // must be clipped to it, or the always-opaque `CentralPanel` fill
    // (painted in "head" every frame regardless of declared damage) erases
    // everything outside it. Intersecting each primitive's own `clip_rect`
    // is sufficient: a `Primitive::Callback`'s `clip_rect` becomes the GL
    // scissor `egui_glow::Painter::paint_primitives` sets before invoking
    // the callback, and `set_clip_rect` clamps `max` to `>= min` (verified
    // against egui_glow 0.36.1), so a primitive fully outside the region
    // scissors to zero area and draws nothing -- no separate zero-size
    // check needed here. `redraw_clip` is `None` whenever `presentation`
    // is `Full`, so this is a no-op on a full frame.
    if let Some(clip) = redraw_clip {
        for primitives in primitives {
            for clipped in primitives.iter_mut() {
                clipped.clip_rect = clipped.clip_rect.intersect(clip);
            }
        }
    }

    // Publish the authoritative region BEFORE the paint callbacks run (they
    // execute inside the `paint_primitives` calls the caller makes right
    // after this returns), so any callback that scissors to the damage
    // region reads the same value that decided whether the clear was
    // skipped and whether the egui primitives were clipped. Same-thread
    // lock immediately before the reads -> an uncontended `Mutex` is
    // sufficient, no atomics needed. Not published at all on a `None`
    // frame (this function is never called for one): no paint callback
    // runs to read it that frame, and a stale published value from the
    // last real frame is exactly what a callback that never runs should
    // keep seeing.
    if let Some(flag) = present_flag {
        let region = match presentation {
            FramePresentation::Partial(region) => crate::PresentRegion::Region(region),
            FramePresentation::Full | FramePresentation::None => crate::PresentRegion::Full,
        };
        let mut guard = flag
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = region;
    }
}

/// The three tessellated primitive lists for one frame's head/band/tail
/// paint-order split (see [`tessellate_head_band_tail`]'s doc), plus the
/// wall-clock time spent tessellating all three (feature-gated).
struct TessellatedFrame {
    head: Vec<egui::ClippedPrimitive>,
    band: Vec<egui::ClippedPrimitive>,
    tail: Vec<egui::ClippedPrimitive>,
    /// Only ever read by [`paint_frame`]'s `tessellate_elapsed`, which is
    /// itself `cfg`-gated identically -- so this field is too, rather than
    /// existing unread (and warning as dead code) in a default build.
    #[cfg(feature = "frame-profiling")]
    elapsed: std::time::Duration,
}

/// Slice `shapes` into head (chrome painted before the terminal band —
/// e.g. the `CentralPanel` background fill, menu bar, tab bar), band
/// (terminal content, rebuilt every frame), and tail (chrome painted after
/// the band — overlays, borders, modals) by `band_range`, then tessellate
/// each slice separately. `LayerId::background()`'s `PaintList` drains
/// first into `full_output.shapes`, and the band occupies a contiguous
/// range within it (see `App::take_terminal_band_range`), so `band_range`
/// is valid as an index range into `shapes` directly.
///
/// Clamp defensively: an app that reports a range referring to a shape
/// count larger than what actually drained (e.g. stale state) must never
/// panic on the slice below. `start` is clamped to the shape count; `end`
/// is then clamped to `[start, shape count]`, so `start <= end <=
/// shapes.len()` always holds.
///
/// Only ever called when [`paint_frame`]'s `should_paint` is `true` (Task
/// 124.2) -- extracted purely to keep that function under clippy's
/// `too_many_lines` threshold, per `freminal-extend-or-extract`, not
/// because this logic is reusable elsewhere.
fn tessellate_head_band_tail(
    ctx: &egui::Context,
    shapes: &[egui::epaint::ClippedShape],
    band_range: std::ops::Range<usize>,
    pixels_per_point: f32,
) -> TessellatedFrame {
    let start = band_range.start.min(shapes.len());
    let end = band_range.end.clamp(start, shapes.len());
    let band_shapes: Vec<egui::epaint::ClippedShape> = shapes[start..end].to_vec();

    // Task 121 frame-profiling harness: time the ENTIRE tessellation phase
    // as one span (band tessellation plus head/tail tessellation) rather
    // than splitting them into separate counters -- one total, sampled
    // per-frame, is the more meaningful number to threshold/alert on.
    #[cfg(feature = "frame-profiling")]
    let tessellate_start = std::time::Instant::now();

    // The band is tessellated from this frame's own shapes -- the
    // terminal band is rebuilt every frame.
    let band = ctx.tessellate(band_shapes, pixels_per_point);

    // Head/tail: re-tessellated from this frame's own shapes every frame.
    let head_shapes: Vec<egui::epaint::ClippedShape> = shapes[..start].to_vec();
    let tail_shapes: Vec<egui::epaint::ClippedShape> = shapes[end..].to_vec();
    let head = ctx.tessellate(head_shapes, pixels_per_point);
    let tail = ctx.tessellate(tail_shapes, pixels_per_point);

    TessellatedFrame {
        head,
        band,
        tail,
        #[cfg(feature = "frame-profiling")]
        elapsed: tessellate_start.elapsed(),
    }
}

/// Paint one egui frame into `surface`.
///
/// Runs the app's `ui_fn`, decides whether this frame may skip the clear
/// (124.17) or skip tessellation, clearing, and painting entirely (124.2),
/// then -- unless skipped -- tessellates and paints the head/band/tail
/// split. Always manages egui's texture deltas, whether or not this frame
/// painted anything. Deliberately does not touch anything window-bound
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
        damage_history,
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

    // Decide whether this frame may skip the full clear and present only a
    // reconstructed redraw region -- or skip presenting at all (Task
    // 124.2). Resolved HERE, immediately once `pixels_per_point` is known
    // and BEFORE any shape partitioning or tessellation runs: a
    // `FrameDamage::None` frame has nothing to tessellate or paint, so
    // doing that work first and only then discovering it was unnecessary
    // would contradict the entire point of this subtask -- proving a
    // skipped frame costs nothing at the paint layer, not merely that its
    // GL calls are skipped after the CPU work to feed them already ran.
    //
    // For a non-`None` frame this is a two-part gate:
    //   1. The app reports the frame as `Partial` (only the listed rects
    //      changed; everything else is identical to the previous frame).
    //   2. `DamageHistory` can reconstruct a safe redraw region for the
    //      queried buffer age (see [`DamageHistory::redraw_region`]), and
    //      the surface can present a sub-region.
    // If either fails we fall back to the always-correct full path:
    // clear + full paint + full swap.
    //
    // 124.17/124.18/124.2: the gate itself lives in `decide_partial_present`
    // (pure, unit-tested), and its resolution into a `FramePresentation` /
    // clip-rect pair lives in `ResolvedPartialPresent::resolve` (extracted
    // solely to keep this function under clippy's `too_many_lines`
    // threshold) -- see that function's doc for why `buffer_age` stays a
    // lazy closure here rather than a plain `u32`.
    let resolved = ResolvedPartialPresent::resolve(
        surface,
        frame_damage,
        damage_history,
        size_px,
        pixels_per_point,
    );
    let presentation = resolved.presentation;
    let redraw_clip = resolved.redraw_clip;

    // Task 124.2: `FramePresentation::None` means nothing changed this
    // frame at all -- skip shape partitioning, tessellation, the clear,
    // and every GL primitive paint entirely. The egui UI pass above still
    // ran (so scheduling, platform output, and the app's own damage
    // computation stay correct), and the texture-delta drains below still
    // run unconditionally (painter bookkeeping and egui 0.36's drop-bomb
    // contract, not framebuffer paint -- see the comment at those
    // drains), but no CPU work that exists only to feed a GL paint call
    // may run either.
    let should_paint = !matches!(presentation, FramePresentation::None);

    // Task 121 frame-profiling harness: time the ENTIRE tessellation
    // phase as one span (band tessellation plus head/tail tessellation)
    // rather than splitting them into separate counters -- one total,
    // sampled per-frame, is the more meaningful number to
    // threshold/alert on. Initialized to exactly `Duration::ZERO` and left
    // untouched when `should_paint` is `false` (Task 124.2) -- there is no
    // tessellation phase to measure on a skipped frame, so this is never
    // an `Instant::elapsed()` taken around empty work.
    #[cfg(feature = "frame-profiling")]
    let mut tessellate_elapsed = std::time::Duration::ZERO;

    // ── 3-way paint-order split ──────────────────────────
    //
    // Only runs when `should_paint` (Task 124.2): a `None` frame has
    // nothing to slice or tessellate. See [`tessellate_head_band_tail`]'s
    // doc for the slicing/clamping contract. `full_output.shapes` is taken
    // via `std::mem::take` rather than moved out of the field directly, so
    // `full_output`'s other fields (`platform_output`, `viewport_output`,
    // `textures_delta`) stay fully usable below regardless of which branch
    // ran, and a `None` frame's shapes are simply left in place (and
    // dropped, unread, with `full_output` at the end of this function).
    let (mut head_primitives, mut band_primitives, mut tail_primitives) = if should_paint {
        let shapes = std::mem::take(&mut full_output.shapes);
        let tessellated = tessellate_head_band_tail(ctx, &shapes, band_range, pixels_per_point);
        #[cfg(feature = "frame-profiling")]
        {
            tessellate_elapsed = tessellated.elapsed;
        }
        (tessellated.head, tessellated.band, tessellated.tail)
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    if should_paint {
        clear_clip_and_publish(
            surface,
            clear_color,
            presentation,
            redraw_clip,
            present_flag,
            [
                &mut head_primitives,
                &mut band_primitives,
                &mut tail_primitives,
            ],
        );
    }

    // Texture bookkeeping -- runs UNCONDITIONALLY, regardless of
    // `should_paint`. This is painter-state management (which textures the
    // `egui_glow::Painter` atlas knows about) and egui 0.36's
    // `TexturesDelta` drop-bomb contract, not framebuffer paint: it must
    // stay correct even on a `None` frame that skips shape partitioning,
    // tessellation, and every GL primitive paint, per Task 124.2's mandate
    // not to mislabel this bookkeeping as the work that IS skipped.
    //
    // Paint order when `should_paint`: set all textures, then three
    // `paint_primitives` calls in head -> band -> tail order, then free all
    // textures. This is exactly what `paint_and_update_textures` does
    // internally (set all -> paint -> free all), just split across three
    // paint calls so the band can be painted independently of chrome.
    // Order matters: `paint_primitives` re-establishes GL state
    // (scissor/blend, unbound VBO/EBO/texture/program) independently on
    // every call, so three sequential calls over a partition of the same
    // shape list paint identically to one call over the concatenation —
    // head paints first (e.g. the `CentralPanel` background fill, which
    // must be UNDER the band), then band, then tail (overlays/borders,
    // which must be OVER the band).
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
    // Initialized to exactly `Duration::ZERO` and left untouched when
    // `should_paint` is `false` (Task 124.2) -- there is no paint phase to
    // measure on a skipped frame, so this is never an `Instant::elapsed()`
    // taken around empty work.
    #[cfg(feature = "frame-profiling")]
    let mut paint_elapsed = std::time::Duration::ZERO;
    if should_paint {
        #[cfg(feature = "frame-profiling")]
        let paint_start = std::time::Instant::now();
        painter.paint_primitives(size_px, pixels_per_point, &head_primitives);
        painter.paint_primitives(size_px, pixels_per_point, &band_primitives);
        painter.paint_primitives(size_px, pixels_per_point, &tail_primitives);
        #[cfg(feature = "frame-profiling")]
        {
            paint_elapsed = paint_start.elapsed();
        }
    }
    for id in full_output.textures_delta.free.drain() {
        painter.free_texture(id);
    }

    PaintFrameOutput {
        platform_output: full_output.platform_output,
        viewport_output: full_output.viewport_output,
        presentation,
        #[cfg(any(feature = "frame-profiling", feature = "gl-offscreen"))]
        decision: resolved.reported_decision(),
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
    use super::{
        DamageHistory, FramePresentation, FrameSurface, PartialPresentDecision,
        PartialPresentSupport, ResolvedPartialPresent, decide_partial_present,
    };
    use crate::{DamageRect, FrameDamage};

    /// Surface size shared by every test in this module -- arbitrary but
    /// fixed, so a `FrameDamage::Full` history entry's whole-surface
    /// contribution is a known, exact value.
    const SIZE_PX: [u32; 2] = [640, 480];

    fn a_rect() -> DamageRect {
        DamageRect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    /// A 10x10 rect at `x`, `y == 0` -- used to build non-overlapping rects
    /// at known offsets so a union's exact bounding box is easy to predict.
    fn rect_at(x: i32) -> DamageRect {
        DamageRect {
            x,
            y: 0,
            width: 10,
            height: 10,
        }
    }

    /// A [`FrameSurface`] that reports fixed [`PartialPresentSupport`]/age
    /// and panics if anything ever calls into GL through it -- sufficient
    /// for exercising [`ResolvedPartialPresent::resolve`] directly (Task
    /// 124.2's history-bookkeeping tests below), since that function never
    /// touches `glow()`/`clear_to()`/`clear_region_to()` itself (only
    /// `paint_frame` does, and only when `should_paint` is true).
    struct FakeSurface {
        support: PartialPresentSupport,
        age: u32,
    }

    impl FrameSurface for FakeSurface {
        fn glow(&self) -> &glow::Context {
            unreachable!("FakeSurface: resolve() must never touch GL")
        }

        fn partial_present_support(&self) -> PartialPresentSupport {
            self.support
        }

        fn back_buffer_age(&self) -> u32 {
            self.age
        }

        fn clear_to(&self, _color: [f32; 4]) {
            unreachable!("FakeSurface: resolve() must never paint")
        }

        fn clear_region_to(&self, _color: [f32; 4], _region: DamageRect) {
            unreachable!("FakeSurface: resolve() must never paint")
        }
    }

    #[test]
    fn not_requested_on_full_damage_and_never_queries_buffer_age() {
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::Full,
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || panic!("buffer_age() must not be queried on a Full frame"),
        );
        assert_eq!(decision, PartialPresentDecision::NotRequested);
    }

    // ── Task 124.2: `FrameDamage::None` ──────────────────────────────────

    #[test]
    fn no_presentation_on_none_damage_and_never_queries_buffer_age() {
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::None,
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || panic!("buffer_age() must not be queried on a None frame"),
        );
        assert_eq!(decision, PartialPresentDecision::NoPresentation);
    }

    /// Even an unsupported surface or an unqueryable history must not
    /// change a `None` frame's decision -- `FrameDamage::None` short-circuits
    /// before any of the `Partial`-only gate logic runs.
    #[test]
    fn no_presentation_on_none_damage_regardless_of_surface_support() {
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::None,
            PartialPresentSupport::Unsupported,
            &history,
            SIZE_PX,
            || panic!("buffer_age() must not be queried on a None frame"),
        );
        assert_eq!(decision, PartialPresentDecision::NoPresentation);
    }

    /// [`ResolvedPartialPresent::resolve`] must resolve a `None` decision to
    /// [`FramePresentation::None`] and must NOT record it into
    /// [`DamageHistory`] -- a `None` frame never swaps, so it is not a
    /// "recently presented frame" the history describes (see
    /// `DamageHistory::push`'s doc).
    #[test]
    fn resolve_on_none_damage_yields_framepresentation_none_and_does_not_advance_history() {
        let mut history = DamageHistory::new();
        let surface = FakeSurface {
            support: PartialPresentSupport::Supported,
            age: 1,
        };
        let resolved = ResolvedPartialPresent::resolve(
            &surface,
            FrameDamage::None,
            &mut history,
            SIZE_PX,
            1.0,
        );
        assert_eq!(resolved.presentation, FramePresentation::None);
        assert_eq!(resolved.redraw_clip, None);
        assert_eq!(
            history.len(),
            0,
            "a None-decision frame must not be pushed into DamageHistory"
        );
    }

    /// A `Full` or `Partial` decision, by contrast, DOES advance
    /// [`DamageHistory`] -- the contrast case that proves the test above
    /// is pinning `None`'s special-casing specifically, not history's
    /// `push` being broken in general.
    #[test]
    fn resolve_on_full_damage_advances_history() {
        let mut history = DamageHistory::new();
        let surface = FakeSurface {
            support: PartialPresentSupport::Supported,
            age: 1,
        };
        let resolved = ResolvedPartialPresent::resolve(
            &surface,
            FrameDamage::Full,
            &mut history,
            SIZE_PX,
            1.0,
        );
        assert_eq!(resolved.presentation, FramePresentation::Full);
        assert_eq!(
            history.len(),
            1,
            "a Full-decision frame DOES present and must be recorded"
        );
    }

    /// A `None` frame interleaved between two presenting frames must be
    /// invisible to a LATER frame's buffer-age reconstruction -- if a
    /// `None` frame were mistakenly pushed, it would occupy a slot in
    /// `DamageHistory`'s bounded lookback and silently displace a real
    /// presented frame's damage from the window a later `age` query needs.
    #[test]
    fn a_none_frame_interleaved_between_presenting_frames_does_not_shift_the_history_window() {
        let mut history = DamageHistory::new();
        let surface = FakeSurface {
            support: PartialPresentSupport::Supported,
            age: 1,
        };

        // Frame 1: Full baseline -- always presents, always recorded.
        ResolvedPartialPresent::resolve(&surface, FrameDamage::Full, &mut history, SIZE_PX, 1.0);
        // Frame 2: Partial, presents (age == 1 needs no history), recorded
        // with its own rect at x == 20.
        ResolvedPartialPresent::resolve(
            &surface,
            FrameDamage::Partial(vec![rect_at(20)]),
            &mut history,
            SIZE_PX,
            1.0,
        );
        // Frame 3: None -- must NOT be recorded.
        ResolvedPartialPresent::resolve(&surface, FrameDamage::None, &mut history, SIZE_PX, 1.0);

        assert_eq!(
            history.len(),
            2,
            "the None frame must not have added a third entry"
        );

        // Frame 4: Partial at age == 2 -- needs exactly the immediately
        // previous PRESENTED frame's damage (frame 2's `rect_at(20)`), not
        // the interleaved None frame's (nonexistent) contribution. Had the
        // None frame been incorrectly pushed, `entries` would hold three
        // entries with the (bogus) None one newest, so an age-2 query's
        // "one previous frame" would consume THAT entry instead --
        // `damage_bbox(&FrameDamage::None, ..)` contributes nothing (see
        // that function's doc), so the union would silently OMIT frame 2's
        // real rect entirely, never even reaching back far enough to touch
        // frame 1's whole-surface `Full` contribution.
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(
            history.redraw_region(&current, 2, SIZE_PX),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 30,
                height: 10,
            }),
            "age == 2 after a None frame must union with frame 2's own rect, \
             exactly as if the None frame had never been painted at all"
        );
    }

    #[test]
    fn requested_with_no_rects_on_empty_partial_and_never_queries_buffer_age() {
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::Partial(Vec::new()),
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || panic!("buffer_age() must not be queried when the rect list is empty"),
        );
        assert_eq!(decision, PartialPresentDecision::RequestedWithNoRects);
    }

    #[test]
    fn blocked_by_surface_when_unsupported_and_never_queries_buffer_age() {
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Unsupported,
            &history,
            SIZE_PX,
            || panic!("buffer_age() must not be queried when the surface can't present partially"),
        );
        assert_eq!(decision, PartialPresentDecision::BlockedBySurface);
    }

    #[test]
    fn blocked_by_buffer_age_when_history_cannot_reconstruct_the_age() {
        // Age 2 needs one previous frame's damage; an empty history (e.g.
        // one of the first frames after window creation) has none yet.
        let history = DamageHistory::new();
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || 2,
        );
        assert_eq!(
            decision,
            PartialPresentDecision::BlockedByBufferAge { age: 2 }
        );
    }

    #[test]
    fn blocked_by_buffer_age_when_age_is_zero() {
        // Age 0 means "new or unknown" -- must NOT be reconstructable no
        // matter how much history is available.
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
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
        // buffer age == 1 (needs no history at all).
        let history = DamageHistory::new();
        let taken = decide_partial_present(
            &FrameDamage::Partial(vec![a_rect()]),
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || 1,
        );
        assert_eq!(
            taken,
            PartialPresentDecision::Taken {
                age: 1,
                region: a_rect()
            }
        );

        // Drop each condition in turn -- each alone must prevent `Taken`.
        assert!(!matches!(
            decide_partial_present(
                &FrameDamage::Full,
                PartialPresentSupport::Supported,
                &history,
                SIZE_PX,
                || 1
            ),
            PartialPresentDecision::Taken { .. }
        ));
        assert!(!matches!(
            decide_partial_present(
                &FrameDamage::Partial(Vec::new()),
                PartialPresentSupport::Supported,
                &history,
                SIZE_PX,
                || 1
            ),
            PartialPresentDecision::Taken { .. }
        ));
        assert!(!matches!(
            decide_partial_present(
                &FrameDamage::Partial(vec![a_rect()]),
                PartialPresentSupport::Unsupported,
                &history,
                SIZE_PX,
                || 1
            ),
            PartialPresentDecision::Taken { .. }
        ));
        assert!(
            !matches!(
                decide_partial_present(
                    &FrameDamage::Partial(vec![a_rect()]),
                    PartialPresentSupport::Supported,
                    &history,
                    SIZE_PX,
                    || 2
                ),
                PartialPresentDecision::Taken { .. }
            ),
            "an unreconstructable age (2, with no history to draw on) alone must block Taken"
        );
    }

    #[test]
    fn taken_at_age_two_unions_with_the_immediately_previous_frame() {
        // The common real-hardware case (124.17's GPU re-take): a
        // conventionally double-buffered surface reports `buffer_age() ==
        // 2` in steady state. With one frame of history, this must now be
        // `Taken`, not blocked.
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        let decision = decide_partial_present(
            &FrameDamage::Partial(vec![rect_at(0)]),
            PartialPresentSupport::Supported,
            &history,
            SIZE_PX,
            || 2,
        );
        assert_eq!(
            decision,
            PartialPresentDecision::Taken {
                age: 2,
                region: DamageRect {
                    x: 0,
                    y: 0,
                    width: 30,
                    height: 10,
                },
            }
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

    // ── `DamageHistory::redraw_region` (124.18) ──────────────────────────
    //
    // Pure arithmetic, no GL needed. Covers `age` 0, 1, 2, 3 (== `MAX_DEPTH`),
    // exactly at the retained depth, and deeper than it -- per the plan's
    // mandate that this arithmetic is fully unit-testable on its own.

    #[test]
    fn redraw_region_is_none_for_age_zero_regardless_of_history() {
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(history.redraw_region(&current, 0, SIZE_PX), None);

        // Also true of a completely empty history.
        let empty = DamageHistory::new();
        assert_eq!(empty.redraw_region(&current, 0, SIZE_PX), None);
    }

    #[test]
    fn redraw_region_at_age_one_uses_only_current_damage_ignoring_history() {
        // `age == 1` needs zero previous frames. A decoy history entry far
        // from `current` must NOT widen the result -- if it did, this
        // would come back covering `rect_at(100)` too.
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(100)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(history.redraw_region(&current, 1, SIZE_PX), Some(a_rect()));
    }

    #[test]
    fn redraw_region_at_age_two_unions_current_with_one_previous_frame() {
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(
            history.redraw_region(&current, 2, SIZE_PX),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 30,
                height: 10,
            })
        );
    }

    #[test]
    fn redraw_region_at_age_three_unions_current_with_two_previous_frames() {
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        history.push(FrameDamage::Partial(vec![rect_at(40)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(
            history.redraw_region(&current, 3, SIZE_PX),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 50,
                height: 10,
            })
        );
    }

    #[test]
    fn redraw_region_at_exactly_the_retained_depth_unions_every_entry() {
        // `age == MAX_DEPTH + 1` needs `MAX_DEPTH` previous frames -- the
        // deepest reconstructable case.
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        history.push(FrameDamage::Partial(vec![rect_at(40)]));
        history.push(FrameDamage::Partial(vec![rect_at(60)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        let age = u32::try_from(DamageHistory::MAX_DEPTH + 1).unwrap_or(u32::MAX);
        assert_eq!(
            history.redraw_region(&current, age, SIZE_PX),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 70,
                height: 10,
            })
        );
    }

    #[test]
    fn redraw_region_is_none_when_age_needs_more_than_the_retained_depth() {
        // `age == MAX_DEPTH + 2` needs `MAX_DEPTH + 1` previous frames --
        // one more than `DamageHistory` ever retains, regardless of how
        // many entries happen to be stored.
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Partial(vec![rect_at(20)]));
        history.push(FrameDamage::Partial(vec![rect_at(40)]));
        history.push(FrameDamage::Partial(vec![rect_at(60)]));
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        let age = u32::try_from(DamageHistory::MAX_DEPTH + 2).unwrap_or(u32::MAX);
        assert_eq!(history.redraw_region(&current, age, SIZE_PX), None);
    }

    #[test]
    fn redraw_region_is_none_when_history_has_not_recorded_enough_frames_yet() {
        // `age == 2` needs one previous frame -- distinct from the
        // "deeper than MAX_DEPTH" case above: this is well within the
        // retained depth, but nothing has been recorded yet (e.g. one of
        // the first frames after window creation).
        let history = DamageHistory::new();
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(history.redraw_region(&current, 2, SIZE_PX), None);
    }

    #[test]
    fn redraw_region_folds_a_full_history_entry_into_a_whole_surface_union() {
        let mut history = DamageHistory::new();
        history.push(FrameDamage::Full);
        let current = FrameDamage::Partial(vec![rect_at(0)]);
        assert_eq!(
            history.redraw_region(&current, 2, SIZE_PX),
            Some(DamageRect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            })
        );
    }

    #[test]
    fn push_evicts_the_oldest_entry_beyond_max_depth() {
        let mut history = DamageHistory::new();
        for i in 0..DamageHistory::MAX_DEPTH {
            let x = i32::try_from(i).unwrap_or(0) * 20;
            history.push(FrameDamage::Partial(vec![rect_at(x)]));
        }
        assert_eq!(history.len(), DamageHistory::MAX_DEPTH);
        history.push(FrameDamage::Partial(vec![rect_at(1000)]));
        assert_eq!(
            history.len(),
            DamageHistory::MAX_DEPTH,
            "pushing beyond MAX_DEPTH must evict the oldest entry, not grow unbounded"
        );
    }
}
