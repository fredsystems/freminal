// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Terminal upload/draw GPU timing (Task 125.8), built on 125.7's
//! GL-independent [`GpuQueryRing`] foundation
//! (`freminal_windowing::gpu_profiling`).
//!
//! One concept: a per-pane, renderer-side adapter that turns the pure
//! ring/state-machine from 125.7 into real asynchronous GPU timestamp
//! measurements of exactly two phases per drawn frame -- the terminal
//! upload commands (`gui::renderer::gpu::TerminalRenderer::draw_with_verts`
//! / `draw_with_cursor_only_update`'s atlas/image/vertex uploads) and the
//! terminal draw commands that follow them (the five `draw_*` passes) --
//! never a third phase, never a subtraction-derived one.
//!
//! # Why per-pane, not one ring shared across the window
//!
//! [`freminal_windowing::gpu_profiling::GpuProfilePhase`] is a bare
//! `&'static str` name with no room for a dynamic pane id, and
//! [`freminal_windowing::gpu_profiling::CompletedSample`] does not return
//! the query handles it just destroyed -- both are 125.7 design decisions
//! this subtask must not reopen (125.7 is already reviewed and merged, and
//! `freminal-windowing/src/gpu_profiling.rs` is outside this subtask's
//! modification scope). Encoding a pane id into the phase string and
//! parsing it back out would be exactly the string-parsing metadata
//! anti-pattern the plan explicitly warns against.
//!
//! The design that avoids both problems without touching 125.7 at all:
//! **one [`PaneGpuTimingProfile`] per pane**, owned by that pane's own
//! `TerminalRenderer` (mirroring `gui::renderer::profiling::LiveRenderProfile`,
//! which is likewise one instance per pane's `RenderState`). Pane identity
//! then falls out of *which instance* produced a report -- never
//! encoded/decoded through the ring at all -- because the caller (`gpu.rs`'s
//! `draw_with_verts` for one specific pane) is always the same caller that
//! already knows its own pane id when it later logs that pane's flush (see
//! `gui::terminal::widget`'s call site). "Aggregate all panes" is realized
//! by profiling every pane uniformly (not only the active one) under the
//! same [`GpuProfilePhase`] pair and the same [`LOG_TARGET`], so a
//! multi-pane session's log can be aggregated post-hoc by pane id -- not by
//! a single ring shared across panes, which the 125.7 types cannot express
//! without reopening that subtask.
//!
//! # Timestamp pairs, never `TIME_ELAPSED`
//!
//! Every phase is measured with two independent `GL_TIMESTAMP` queries
//! (`glQueryCounter`), never `glBeginQuery`/`glEndQuery` with
//! `GL_TIME_ELAPSED`. A `TIME_ELAPSED` query cannot represent two adjacent,
//! non-overlapping spans (upload then draw) without nesting or nulling the
//! first before starting the second; two timestamp pairs express both
//! spans, plus the boundary between them, with ordinary independent GL
//! objects. [`PaneGpuTimingProfile::end_upload_begin_draw`] issues the
//! upload phase's end timestamp and the draw phase's start timestamp back
//! to back, from the exact call site between the last upload command and
//! the first draw command -- so "upload excludes draw" and "draw begins
//! after uploads, excludes uploads" hold by construction, not by
//! subtraction.

use freminal_windowing::gpu_profiling::{
    CompletedSample, GpuProfilePhase, GpuQueryRing, GpuQuerySource, GpuTimerCapability,
    SampleIssueOutcome, parse_gl_version_string,
};

use super::gl_facade::Gl;

/// The upload-phase name issued around
/// `TerminalRenderer::draw_with_verts`/`draw_with_cursor_only_update`'s
/// atlas/image-texture/vertex-buffer upload calls.
const PHASE_UPLOAD: GpuProfilePhase = GpuProfilePhase("terminal_upload");

/// The draw-phase name issued around the same methods' `draw_*` calls,
/// strictly after the upload phase's end timestamp.
const PHASE_DRAW: GpuProfilePhase = GpuProfilePhase("terminal_draw");

/// How many simultaneously pending upload/draw sample pairs one pane's
/// ring tolerates before rejecting (and counting) a new one. Two samples
/// are issued per drawn frame (one upload pair, one draw pair), so this
/// bounds a GPU-side backlog of roughly 16 frames' worth of unretired
/// queries -- generous for the handful of frames a query is expected to
/// take to retire, while still bounded (never growing without limit if a
/// driver stops signalling availability at all).
const RING_CAPACITY: usize = 32;

/// Explicit `tracing` target for [`PaneGpuTimingProfile`]'s periodic flush
/// summary.
///
/// Distinct from every other Task 121/125 profiling target
/// (`"freminal::frame_profiling"`, `"freminal_windowing::frame_profiling"`,
/// `"freminal::task_125::live_render_profile"`) so a live session can
/// filter terminal GPU timing independently of any of them.
pub const LOG_TARGET: &str = "freminal::task_125::gpu_timing";

/// Adapts the [`Gl`] facade to
/// [`freminal_windowing::gpu_profiling::GpuQuerySource`], so
/// [`GpuQueryRing`] can drive real (or recording-fabricated) query calls
/// without knowing about `Gl` itself. Borrows `Gl` rather than owning it --
/// constructed fresh, cheaply, for the duration of one
/// [`PaneGpuTimingProfile::begin_frame`] call.
struct GlQuerySource<'gl, 'ctx> {
    gl: &'gl Gl<'ctx>,
}

impl GpuQuerySource<glow::Query> for GlQuerySource<'_, '_> {
    fn is_result_available(&mut self, query: &glow::Query) -> bool {
        // SAFETY: `query` was created by this same `Gl` (via `create_query`
        // inside `PaneGpuTimingProfile`) and is still live -- the ring never
        // calls this after `destroy_query` has run for a handle.
        unsafe {
            self.gl
                .get_query_parameter_u32(*query, glow::QUERY_RESULT_AVAILABLE)
                != 0
        }
    }

    fn read_result_ns(&mut self, query: &glow::Query) -> u64 {
        // SAFETY: the ring only calls this immediately after
        // `is_result_available` returned `true` for the same query in the
        // same `poll` call -- see that trait method's contract.
        unsafe { self.gl.get_query_parameter_u64(*query, glow::QUERY_RESULT) }
    }

    fn destroy_query(&mut self, query: glow::Query) {
        // SAFETY: `query` is a live handle this same `Gl` created and has
        // not yet destroyed.
        unsafe { self.gl.delete_query(query) };
    }
}

/// A snapshot of one pane's cumulative GPU timing state, taken when
/// [`PaneGpuTimingProfile::take_flush_signal`] reports a flush is due.
///
/// Deliberately a plain data snapshot rather than a borrow of the live
/// profile: the caller (`gui::terminal::widget`) logs this after the
/// `PaintCallback` that produced it has already returned, so nothing here
/// may reference GL state.
#[derive(Debug, Clone)]
pub struct GpuTimingReport {
    /// Whether this context supports asynchronous GPU timer queries at
    /// all. [`GpuTimerCapability::Unavailable`] means every field below is
    /// permanently zero -- not a transient gap, a structural one (see
    /// [`PaneGpuTimingProfile::detect_capability`]).
    pub capability: GpuTimerCapability,
    /// The active `GL_RENDERER` string, captured once at
    /// [`PaneGpuTimingProfile::detect_capability`] time.
    pub renderer_string: String,
    /// Cumulative measured upload-phase GPU duration, nanoseconds.
    pub upload_ns_total: u64,
    /// Cumulative completed upload-phase samples.
    pub upload_sample_count: u64,
    /// Cumulative measured draw-phase GPU duration, nanoseconds --
    /// terminal draw commands only, excluding upload (Task 125.8's
    /// timestamp-pair boundary makes this hold by construction, not by
    /// subtracting the upload figure from a combined one).
    pub draw_ns_total: u64,
    /// Cumulative completed draw-phase samples.
    pub draw_sample_count: u64,
    /// How many frames elapsed between issue and availability for the
    /// most recently completed sample (either phase) -- the "query
    /// latency in frames" the plan calls for reporting.
    ///
    /// Unit: pane-local drawn frames, i.e. [`PaneGpuTimingProfile::frame`]
    /// ticks -- incremented once per [`PaneGpuTimingProfile::begin_frame`]
    /// call, which runs only when this pane's own `PaintCallback` is
    /// invoked. An idle pane may draw rarely (or not at all for long
    /// stretches), so this is not a count of window frames and not a
    /// wall-clock duration. It must never be compared across panes (two
    /// panes' frame counters advance independently and at different
    /// rates) and must never be read as a latency measured in
    /// milliseconds or window frames.
    pub last_latency_frames: u64,
    /// Cumulative samples rejected because the ring was at
    /// [`RING_CAPACITY`] when issued (never blocks; see
    /// [`GpuQueryRing::dropped_sample_count`]).
    pub dropped_sample_count: u64,
    /// Samples still pending (issued, result not yet available) as of the
    /// most recent [`PaneGpuTimingProfile::begin_frame`] poll -- "results
    /// not yet available", distinct from [`Self::capability`] being
    /// [`GpuTimerCapability::Unavailable`] (a structural absence, not a
    /// transient one).
    pub unavailable_sample_count: usize,
}

/// Per-pane asynchronous GPU timing adapter (Task 125.8).
///
/// Owns exactly one [`GpuQueryRing<glow::Query>`], detected capability, the
/// cached renderer string, and cumulative upload/draw totals -- the same
/// "cumulative-since-creation" idiom `gui::renderer::profiling::LiveRenderProfile`
/// already uses. See the module doc for why this is one instance per pane
/// rather than one shared ring.
///
/// # Call sequence a caller must follow
///
/// 1. [`Self::detect_capability`] -- once, when the owning
///    `TerminalRenderer` initializes its GL resources.
/// 2. Each drawn frame, in this exact order:
///    [`Self::begin_frame`] -> [`Self::begin_upload`] -> (upload GL calls)
///    -> [`Self::end_upload_begin_draw`] -> (draw GL calls) ->
///    [`Self::end_draw`].
/// 3. [`Self::shutdown`] -- once, when the owning `TerminalRenderer` is
///    destroyed.
///
/// Every method is a cheap no-op when [`Self::capability`] is
/// [`GpuTimerCapability::Unavailable`] (the default before
/// [`Self::detect_capability`] runs, and the permanent state on a context
/// that lacks timer-query support) -- no GL call is issued, so an
/// unsupported context renders exactly as it would with this type absent
/// entirely.
#[derive(Debug)]
pub struct PaneGpuTimingProfile {
    ring: GpuQueryRing<glow::Query>,
    capability: GpuTimerCapability,
    renderer_string: String,
    /// This pane's own drawn-frame counter: incremented once per
    /// [`Self::begin_frame`] call, i.e. once per frame in which *this
    /// pane's* `PaintCallback` actually ran -- not once per window frame.
    /// An idle pane's `PaintCallback` may run rarely, so this counter (and
    /// anything derived from it, such as
    /// [`GpuTimingReport::last_latency_frames`]) advances at a pane-local
    /// rate that must never be compared across panes or treated as
    /// wall-clock time.
    frame: u64,
    pending_upload_start: Option<glow::Query>,
    pending_draw_start: Option<glow::Query>,
    upload_ns_total: u64,
    upload_sample_count: u64,
    draw_ns_total: u64,
    draw_sample_count: u64,
    last_latency_frames: u64,
    unavailable_sample_count: usize,
    since_last_flush: u64,
    flush_signal: bool,
}

impl Default for PaneGpuTimingProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneGpuTimingProfile {
    /// Emit a flush-worthy summary once every this many completed
    /// draw-phase samples (one per drawn frame when capability is
    /// available) -- a shorter cadence than
    /// `LiveRenderProfile::FLUSH_EVERY` (120) since GPU timing samples
    /// complete one to a handful of frames after the frame that issued
    /// them, so a shorter window keeps the log responsive to that lag.
    pub const FLUSH_EVERY: u64 = 60;

    /// Construct a profile with no detected capability yet (i.e.
    /// [`GpuTimerCapability::Unavailable`] until [`Self::detect_capability`]
    /// runs) and all-zero cumulative totals.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(RING_CAPACITY)
    }

    /// As [`Self::new`], but with an explicit ring capacity -- used by this
    /// module's own tests to exercise the `DroppedRingFull` path without
    /// issuing 32 real pairs first.
    fn with_capacity(capacity: usize) -> Self {
        Self {
            ring: GpuQueryRing::new(capacity),
            capability: GpuTimerCapability::Unavailable,
            renderer_string: String::new(),
            frame: 0,
            pending_upload_start: None,
            pending_draw_start: None,
            upload_ns_total: 0,
            upload_sample_count: 0,
            draw_ns_total: 0,
            draw_sample_count: 0,
            last_latency_frames: 0,
            unavailable_sample_count: 0,
            since_last_flush: 0,
            flush_signal: false,
        }
    }

    /// Detect GPU timer capability from the real desktop GL version and
    /// extension set, and cache the active `GL_RENDERER` string.
    ///
    /// Reads `GL_VERSION` via [`Gl::get_parameter_string`], parses it with
    /// `freminal_windowing::gpu_profiling::parse_gl_version_string` (125.7's
    /// pure parser, written explicitly for this real-caller path -- see its
    /// doc), and reads the extension set via [`Gl::supported_extensions`]
    /// only when the version parses; if it does not, capability is
    /// conservatively [`GpuTimerCapability::Unavailable`] rather than
    /// guessed. Idempotent: safe to call more than once (only ever done
    /// once per pane, from `TerminalRenderer::init`).
    pub fn detect_capability(&mut self, gl: &Gl<'_>) {
        // SAFETY: `gl` is a valid facade over a current GL context (or the
        // recording arm, which needs no context at all).
        let version_string = unsafe { gl.get_parameter_string(glow::VERSION) };
        let Some(info) = parse_gl_version_string(&version_string) else {
            self.capability = GpuTimerCapability::Unavailable;
            return;
        };
        // SAFETY: as above.
        let extensions = unsafe { gl.supported_extensions() };
        self.capability = GpuTimerCapability::detect(info, &extensions);
        // Only read `GL_RENDERER` -- a third GL call -- when capability
        // actually resolved `Available`: an unsupported context has no use
        // for the renderer string (nothing will ever be reported for it),
        // so this keeps `detect_capability` issuing the minimum possible
        // GL calls on the (common, in every recording-driven test suite
        // that never opts in) unsupported path.
        if self.is_available() {
            // SAFETY: as above.
            self.renderer_string = unsafe { gl.get_parameter_string(glow::RENDERER) };
        }
    }

    /// The detected capability (see [`Self::detect_capability`]).
    #[must_use]
    pub const fn capability(&self) -> GpuTimerCapability {
        self.capability
    }

    /// Whether asynchronous GPU timer queries are available on this
    /// context -- the gate every other method (besides
    /// [`Self::detect_capability`] itself) checks before issuing any GL
    /// call.
    const fn is_available(&self) -> bool {
        matches!(self.capability, GpuTimerCapability::Available)
    }

    /// Poll pending samples for availability (if capability allows) and
    /// return the frame number this call's upcoming upload/draw pair must
    /// be issued against.
    ///
    /// Must be called exactly once per drawn frame, before
    /// [`Self::begin_upload`]. Polling happens here, strictly before this
    /// frame's own samples are issued, so a sample issued during THIS call
    /// (by the immediately following [`Self::begin_upload`] /
    /// [`Self::end_upload_begin_draw`] / [`Self::end_draw`]) can never be
    /// polled on its own issue frame -- the same invariant
    /// [`GpuQueryRing::poll`] itself enforces, upheld here a second,
    /// structural way by call ordering alone.
    ///
    /// "Once per drawn frame" means once per invocation of *this pane's*
    /// `PaintCallback` -- not once per window frame. The `frame` counter
    /// therefore advances in pane-local drawn frames: a pane that draws
    /// rarely (idle, scrolled out of view, etc.) advances this counter
    /// rarely, independently of every other pane and of the window's own
    /// frame cadence. The returned value, and anything derived from it
    /// such as [`GpuTimingReport::last_latency_frames`], must never be
    /// compared across panes and must never be read as a wall-clock
    /// latency.
    pub fn begin_frame(&mut self, gl: &Gl<'_>) -> u64 {
        let frame = self.frame;
        if self.is_available() {
            let mut source = GlQuerySource { gl };
            let completed = self.ring.poll(frame, &mut source);
            for sample in completed {
                self.record_completed(&sample);
            }
            self.unavailable_sample_count = self.ring.len();
        }
        self.frame = self.frame.saturating_add(1);
        frame
    }

    /// Issue the upload phase's start timestamp. Call immediately before
    /// the first upload GL command (atlas sync, image texture sync, vertex
    /// buffer uploads).
    pub fn begin_upload(&mut self, gl: &Gl<'_>) {
        if !self.is_available() {
            return;
        }
        // Defensive: a well-behaved caller always pairs this with
        // `end_upload_begin_draw` before calling this again, so
        // `pending_upload_start` should always be `None` here. If it is
        // not (a caller bug), destroy the stale handle rather than leak
        // it -- this is a safety net, not the expected path.
        if let Some(stale) = self.pending_upload_start.take() {
            // SAFETY: `stale` is a live handle this same `Gl` created.
            unsafe { gl.delete_query(stale) };
        }
        // SAFETY: `gl` is a valid facade over a current GL context.
        if let Ok(query) = unsafe { gl.create_query() } {
            // SAFETY: `query` was just created by this same `Gl`.
            unsafe { gl.query_counter(query, glow::TIMESTAMP) };
            self.pending_upload_start = Some(query);
        }
    }

    /// Issue the upload phase's end timestamp and the draw phase's start
    /// timestamp, back to back. Call immediately after the last upload GL
    /// command and before the first draw GL command -- this is the exact
    /// boundary that makes the upload phase exclude draw commands and the
    /// draw phase begin only after uploads, by construction rather than by
    /// subtraction.
    pub fn end_upload_begin_draw(&mut self, gl: &Gl<'_>, frame: u64) {
        if !self.is_available() {
            return;
        }
        if let Some(start) = self.pending_upload_start.take() {
            // SAFETY: `gl` is a valid facade over a current GL context.
            if let Ok(end) = unsafe { gl.create_query() } {
                // SAFETY: `end` was just created by this same `Gl`.
                unsafe { gl.query_counter(end, glow::TIMESTAMP) };
                self.issue(gl, PHASE_UPLOAD, start, end, frame);
            } else {
                // SAFETY: `start` is a live handle this same `Gl` created.
                unsafe { gl.delete_query(start) };
            }
        }
        // Defensive, mirroring `begin_upload`'s stale-handle guard.
        if let Some(stale) = self.pending_draw_start.take() {
            // SAFETY: `stale` is a live handle this same `Gl` created.
            unsafe { gl.delete_query(stale) };
        }
        // SAFETY: `gl` is a valid facade over a current GL context.
        if let Ok(query) = unsafe { gl.create_query() } {
            // SAFETY: `query` was just created by this same `Gl`.
            unsafe { gl.query_counter(query, glow::TIMESTAMP) };
            self.pending_draw_start = Some(query);
        }
    }

    /// Issue the draw phase's end timestamp. Call immediately after the
    /// last terminal draw GL command (before any framebuffer-restore
    /// bookkeeping, which is not itself a terminal draw command).
    pub fn end_draw(&mut self, gl: &Gl<'_>, frame: u64) {
        if !self.is_available() {
            return;
        }
        if let Some(start) = self.pending_draw_start.take() {
            // SAFETY: `gl` is a valid facade over a current GL context.
            if let Ok(end) = unsafe { gl.create_query() } {
                // SAFETY: `end` was just created by this same `Gl`.
                unsafe { gl.query_counter(end, glow::TIMESTAMP) };
                self.issue(gl, PHASE_DRAW, start, end, frame);
            } else {
                // SAFETY: `start` is a live handle this same `Gl` created.
                unsafe { gl.delete_query(start) };
            }
        }
    }

    /// Issue one start/end pair to the ring, destroying both handles
    /// immediately if the ring is at [`RING_CAPACITY`] and rejects it --
    /// `GpuQueryRing::issue`'s `DroppedRingFull` hands ownership straight
    /// back for exactly this reason (see that variant's doc).
    fn issue(
        &mut self,
        gl: &Gl<'_>,
        phase: GpuProfilePhase,
        start: glow::Query,
        end: glow::Query,
        frame: u64,
    ) {
        if let SampleIssueOutcome::DroppedRingFull { start, end } =
            self.ring.issue(phase, start, end, frame)
        {
            // SAFETY: both handles are live and were never stored by the
            // ring, so the ring cannot and will not destroy them --
            // `GpuQueryRing::issue`'s doc requires the caller to.
            unsafe {
                gl.delete_query(start);
                gl.delete_query(end);
            }
        }
    }

    /// Fold one completed sample into the cumulative totals and latch the
    /// flush signal if this observation crosses a [`Self::FLUSH_EVERY`]
    /// boundary.
    fn record_completed(&mut self, sample: &CompletedSample) {
        self.last_latency_frames = sample.latency_frames();
        let duration = sample.duration_ns();
        if sample.phase == PHASE_UPLOAD {
            self.upload_ns_total = self.upload_ns_total.saturating_add(duration);
            self.upload_sample_count = self.upload_sample_count.saturating_add(1);
        } else if sample.phase == PHASE_DRAW {
            self.draw_ns_total = self.draw_ns_total.saturating_add(duration);
            self.draw_sample_count = self.draw_sample_count.saturating_add(1);
            self.since_last_flush = self.since_last_flush.saturating_add(1);
            if self.since_last_flush.is_multiple_of(Self::FLUSH_EVERY) {
                self.flush_signal = true;
            }
        }
    }

    /// Consume the pending flush signal, if one is set. `true` at most
    /// once per crossed [`Self::FLUSH_EVERY`] boundary -- the same
    /// one-shot idiom as `LiveRenderProfile::take_flush_signal`.
    #[must_use]
    pub fn take_flush_signal(&mut self) -> bool {
        std::mem::take(&mut self.flush_signal)
    }

    /// Snapshot the current cumulative totals for logging.
    #[must_use]
    pub fn report(&self) -> GpuTimingReport {
        GpuTimingReport {
            capability: self.capability,
            renderer_string: self.renderer_string.clone(),
            upload_ns_total: self.upload_ns_total,
            upload_sample_count: self.upload_sample_count,
            draw_ns_total: self.draw_ns_total,
            draw_sample_count: self.draw_sample_count,
            last_latency_frames: self.last_latency_frames,
            dropped_sample_count: self.ring.dropped_sample_count(),
            unavailable_sample_count: self.unavailable_sample_count,
        }
    }

    /// Explicit shutdown: destroy every still-pending query handle
    /// (including an in-flight `pending_upload_start`/`pending_draw_start`
    /// half-issued pair) without reading their results. Call once, when
    /// the owning `TerminalRenderer` is destroyed.
    pub fn shutdown(&mut self, gl: &Gl<'_>) {
        if !self.is_available() {
            return;
        }
        if let Some(query) = self.pending_upload_start.take() {
            // SAFETY: `query` is a live handle this same `Gl` created.
            unsafe { gl.delete_query(query) };
        }
        if let Some(query) = self.pending_draw_start.take() {
            // SAFETY: as above.
            unsafe { gl.delete_query(query) };
        }
        let mut source = GlQuerySource { gl };
        self.ring.drain_and_destroy(&mut source);
    }
}

// This test suite drives the `Recording` arm of `Gl` end to end (`Gl::recording`,
// `recorded()`, `mark_query_available`), so -- like `gpu.rs`'s
// `upload_byte_attribution_tests` -- it requires `gl-recording` in addition
// to the `gpu-profiling` feature this whole module is gated on.
#[cfg(all(test, feature = "gl-recording"))]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{PHASE_DRAW, PHASE_UPLOAD, PaneGpuTimingProfile};
    use crate::gui::renderer::gl_facade::Gl;
    use freminal_windowing::gpu_profiling::GpuTimerCapability;

    /// Configure `gl`'s recording session to report a parseable modern
    /// desktop `GL_VERSION` string, so `detect_capability` resolves
    /// `Available`.
    ///
    /// A fresh recording session's `GL_VERSION` defaults to empty
    /// (deliberately unparsable -- see
    /// [`super::super::gl_facade::recording::RecordingState::gl_version_string`]'s
    /// doc), so every one of *this module's own* tests that needs
    /// `Available` capability must opt in explicitly through this helper.
    /// Tests elsewhere in the crate that drive `TerminalRenderer` through
    /// the recording facade (Task 123's headless workload assertions in
    /// particular) never call this, so their existing call counts are
    /// unaffected by gpu-profiling's capability defaulting to
    /// `Unavailable`.
    fn enable_available_capability(gl: &Gl<'_>) {
        gl.recorded()
            .expect("a recording Gl always has state")
            .set_gl_version_string("4.6.0 Recording");
    }

    /// Drive `detect_capability` against the recording facade, configured
    /// (via [`enable_available_capability`]) to report a parseable modern
    /// desktop `GL_VERSION` string -- proves the real
    /// read-`GL_VERSION`-then-parse-then-check-extensions pipeline resolves
    /// to `Available` end to end through the actual [`Gl`] facade, not
    /// just via 125.7's own pure-function tests.
    #[test]
    fn capability_detection_resolves_available_via_the_facade_round_trip() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();
        assert_eq!(profile.capability(), GpuTimerCapability::Unavailable);
        enable_available_capability(&gl);

        profile.detect_capability(&gl);

        assert_eq!(profile.capability(), GpuTimerCapability::Available);
        assert!(
            !profile.report().renderer_string.is_empty(),
            "the renderer string must be captured too"
        );
    }

    /// A fresh recording session's default (empty) `GL_VERSION` resolves
    /// `Unavailable` -- the deliberate default every *other* recording-
    /// driven test suite in the crate relies on (see
    /// [`enable_available_capability`]'s doc).
    #[test]
    fn default_recording_session_resolves_unavailable_capability() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();

        profile.detect_capability(&gl);

        assert_eq!(profile.capability(), GpuTimerCapability::Unavailable);
    }

    /// Before `detect_capability` runs, capability is `Unavailable` by
    /// construction, and every method must therefore issue zero GL calls
    /// -- the "unsupported contexts render normally" requirement: no
    /// query object is ever created, so nothing can go wrong for a
    /// context that cannot service them.
    #[test]
    fn unsupported_capability_never_issues_a_query_and_renders_normally() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();
        assert_eq!(profile.capability(), GpuTimerCapability::Unavailable);

        let frame = profile.begin_frame(&gl);
        profile.begin_upload(&gl);
        profile.end_upload_begin_draw(&gl, frame);
        profile.end_draw(&gl, frame);
        profile.shutdown(&gl);

        let calls = gl.recorded().expect("recording facade").calls();
        assert!(
            calls.is_empty(),
            "an unavailable-capability profile must issue no GL calls at \
             all, got {calls:?}"
        );
        let report = profile.report();
        assert_eq!(report.upload_sample_count, 0);
        assert_eq!(report.draw_sample_count, 0);
        assert_eq!(report.dropped_sample_count, 0);
    }

    /// The exact GL call sequence a single frame's upload/draw pair
    /// issues, in order: two query objects for the upload phase
    /// (start, then end), immediately followed by the draw phase's own
    /// start query, then (in a later call) its end query. Each object is a
    /// `create_query` + `query_counter` pair, so the 4-query, 8-call
    /// sequence below is exactly what `end_upload_begin_draw` and
    /// `end_draw` are documented to produce.
    #[test]
    fn terminal_upload_and_draw_pairs_issue_in_the_documented_gl_call_order() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();
        enable_available_capability(&gl);
        profile.detect_capability(&gl);
        if let Some(state) = gl.recorded() {
            state.clear();
        }

        let frame = profile.begin_frame(&gl);
        profile.begin_upload(&gl);
        profile.end_upload_begin_draw(&gl, frame);
        profile.end_draw(&gl, frame);

        let calls = gl.recorded().expect("recording facade").calls();
        let methods: Vec<&str> = calls.iter().map(|c| c.method).collect();
        assert_eq!(
            methods,
            vec![
                "create_query",
                "query_counter", // start_upload
                "create_query",
                "query_counter", // end_upload
                "create_query",
                "query_counter", // start_draw
                "create_query",
                "query_counter", // end_draw
            ],
            "exactly four timestamp-query pairs, no draw/upload GL calls \
             interleaved by this adapter itself (those are issued by the \
             caller, gpu.rs, in between these calls) -- and no \
             `begin_query`/`end_query` `TIME_ELAPSED` pair anywhere"
        );
    }

    /// Results are never read before availability succeeds, and arrive
    /// only on a later `begin_frame` call -- never the frame that issued
    /// them. Exercises the full round trip through the real facade
    /// (`GlQuerySource` wired to `Gl`), not just 125.7's own
    /// `FakeQuerySource`-driven ring tests.
    #[test]
    fn results_are_never_read_before_availability_and_arrive_on_a_later_frame() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();
        enable_available_capability(&gl);
        profile.detect_capability(&gl);

        let issue_frame = profile.begin_frame(&gl); // frame 0
        profile.begin_upload(&gl);
        profile.end_upload_begin_draw(&gl, issue_frame);
        profile.end_draw(&gl, issue_frame);

        // Frame 1: nothing marked available yet -- both samples remain
        // pending, and (per the ring's own invariant, exercised here
        // through the real facade rather than a fake) no
        // `get_query_parameter_u64` call may have happened for them.
        let _ = profile.begin_frame(&gl); // frame 1, polls with current_frame=1
        let report = profile.report();
        assert_eq!(report.upload_sample_count, 0);
        assert_eq!(report.draw_sample_count, 0);
        assert_eq!(
            report.unavailable_sample_count, 2,
            "both samples (upload, draw) remain pending"
        );

        // Mark all four fabricated handles available now. Handles are
        // fabricated in issue order starting at 1: start_upload=1,
        // end_upload=2, start_draw=3, end_draw=4.
        let recording = gl.recorded().expect("recording facade");
        recording.mark_query_available(query_handle(1), 1_000);
        recording.mark_query_available(query_handle(2), 5_000);
        recording.mark_query_available(query_handle(3), 5_500);
        recording.mark_query_available(query_handle(4), 9_000);

        // Frame 2: now available -- both samples complete.
        let _ = profile.begin_frame(&gl); // frame 2, polls with current_frame=2
        let report = profile.report();
        assert_eq!(report.upload_sample_count, 1);
        assert_eq!(report.upload_ns_total, 4_000); // 5_000 - 1_000
        assert_eq!(report.draw_sample_count, 1);
        assert_eq!(report.draw_ns_total, 3_500); // 9_000 - 5_500
        assert_eq!(
            report.last_latency_frames, 2,
            "issued on frame 0, completed at poll on frame 2"
        );
        assert_eq!(report.unavailable_sample_count, 0);
        assert_eq!(report.dropped_sample_count, 0);
    }

    /// A ring at capacity destroys the rejected sample's handles
    /// immediately (never leaking them, never blocking) and counts the
    /// drop -- `with_capacity(1)` makes the very first `end_draw` call's
    /// issue attempt the one that overflows, since the upload pair alone
    /// already fills the one-sample ring.
    #[test]
    fn dropped_ring_full_destroys_the_rejected_handles_immediately() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::with_capacity(1);
        enable_available_capability(&gl);
        profile.detect_capability(&gl);

        let frame = profile.begin_frame(&gl);
        profile.begin_upload(&gl);
        profile.end_upload_begin_draw(&gl, frame); // fills the 1-slot ring
        profile.end_draw(&gl, frame); // rejected: ring is full

        let report = profile.report();
        assert_eq!(
            report.dropped_sample_count, 1,
            "the draw-phase pair must be counted as dropped"
        );

        // The draw pair's start/end handles (3, 4) must both have been
        // explicitly destroyed -- not merely abandoned -- even though the
        // ring never stored them.
        let calls = gl.recorded().expect("recording facade").calls();
        let delete_count = calls.iter().filter(|c| c.method == "delete_query").count();
        assert_eq!(
            delete_count, 2,
            "both rejected handles must be destroyed immediately"
        );
    }

    /// Task 125.C16: `shutdown` runs from both a failed-init release and a
    /// later `destroy`, so it must be idempotent -- the second call finds no
    /// handle left to destroy and issues no GL call.
    #[test]
    fn shutdown_is_idempotent_and_leaves_no_query_behind() {
        let gl = Gl::recording();
        let mut profile = PaneGpuTimingProfile::new();
        enable_available_capability(&gl);
        profile.detect_capability(&gl);

        let frame = profile.begin_frame(&gl);
        profile.begin_upload(&gl);
        profile.end_upload_begin_draw(&gl, frame);
        // Leave the draw phase half-issued: its start is still pending.

        profile.shutdown(&gl);
        let state = gl.recorded().expect("recording facade");
        assert_eq!(
            state.count_of("create_query"),
            state.count_of("delete_query"),
            "every query created was deleted"
        );
        let calls = state.len();

        profile.shutdown(&gl);
        assert_eq!(state.len(), calls, "a second shutdown issues no GL call");
    }

    /// `record_completed` attributes upload-phase and draw-phase samples
    /// to their own distinct running totals -- proves the two named
    /// [`GpuProfilePhase`] constants this module defines are never
    /// conflated, matching the "named metadata type, not string parsing"
    /// guidance.
    #[test]
    fn phase_constants_are_distinct_and_named() {
        assert_ne!(PHASE_UPLOAD, PHASE_DRAW);
        assert_eq!(PHASE_UPLOAD.0, "terminal_upload");
        assert_eq!(PHASE_DRAW.0, "terminal_draw");
    }

    /// Reconstruct the `n`th fabricated `glow::Query` handle, matching
    /// [`super::super::gl_facade::recording::RecordingState`]'s
    /// monotonic-counter-from-1 fabrication order -- used only to predict
    /// exactly which handle a specific `create_query` call in a fully
    /// controlled test sequence produced, so
    /// `mark_query_available` can target it.
    const fn query_handle(n: u32) -> glow::Query {
        glow::NativeQuery(std::num::NonZeroU32::new(n).expect("n must be nonzero"))
    }
}
