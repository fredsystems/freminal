// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! egui integration: input translation and rendering via `egui-winit` and `egui_glow`.

use std::sync::Arc;

use conv2::ConvUtil;
#[cfg(feature = "frame-profiling")]
use conv2::ValueInto;
use winit::window::Window;

use crate::error::Error;
#[cfg(feature = "frame-profiling")]
use crate::frame_paint::PartialPresentDecision;
use crate::frame_paint::{DamageHistory, FramePresentation, PaintFrameRequest, paint_frame};
use crate::gl_context::GlState;
use crate::modifier_tracker::ModifierTracker;

/// Output from a single egui frame.
pub struct FrameOutput {
    /// Viewport commands emitted by the app during this frame.
    pub commands: Vec<egui::ViewportCommand>,
    /// Requested repaint delay (`Duration::MAX` = no repaint needed).
    pub repaint_delay: std::time::Duration,
    /// The delay the *app* itself asked for this frame (`App::take_terminal_requested_delay`),
    /// independent of whatever egui decided it wanted.
    ///
    /// SPIKE (Task 121): the event loop uses this to override `repaint_delay`
    /// when egui asked for an immediate repaint *solely* because of pointer
    /// events the app already classified as needing no frame. See the
    /// override at the `RedrawRequested` arm in `event_loop.rs`.
    pub app_requested_delay: Option<std::time::Duration>,
}

/// Per-window egui state.
pub struct EguiState {
    pub(crate) ctx: egui::Context,
    pub(crate) winit_state: egui_winit::State,
    pub(crate) painter: egui_glow::Painter,
    /// Live modifier state for this window. Mirrors what `egui-winit` tracks
    /// privately, because egui 0.36 removed `RawInput::modifiers` and exposes
    /// no accessor for its replacement — see [`ModifierTracker`] for why the
    /// egui-side `Context::input(|i| i.modifiers)` is not a substitute.
    modifier_tracker: ModifierTracker,
    /// Task 121 frame-profiling harness (feature-gated): per-window
    /// accumulated phase timings, flushed as a `tracing::debug!` line every
    /// [`FrameProfile::FLUSH_EVERY`] frames. See the module-level rationale
    /// on [`FrameProfile`] itself.
    #[cfg(feature = "frame-profiling")]
    frame_profile: FrameProfile,
    /// This window's record of recent frames' own declared damage, used by
    /// [`paint_frame`] to reconstruct the redraw region a stale back
    /// buffer needs (124.18). See [`DamageHistory`]'s doc.
    damage_history: DamageHistory,
}

/// Task 121 frame-profiling harness (feature-gated only -- see the
/// `frame-profiling` Cargo feature on this crate and on `freminal`, which
/// enables it here).
///
/// Accumulates, per window, wall-clock phase timings for the windowing
/// layer's own share of `run_frame` (running `ctx.run_ui`, tessellating,
/// painting, swapping buffers, and the whole frame as `phase_total`).
/// Flushed as its own `tracing::debug!` line
/// (target `freminal_windowing::frame_profiling`) every
/// [`Self::FLUSH_EVERY`] frames, tagged with this window's `window_id` (see
/// the flush call site) so a multi-window session's interleaved log lines
/// can be told apart, and correlated with `freminal`'s own frame-profiling
/// line for the SAME `window_id`.
///
/// **The two crates' counters do NOT always march in lockstep** (an earlier
/// version of this doc claimed they did -- that was false). `frame_counter`
/// here increments once per `run_frame` call, unconditionally. `freminal`'s
/// own `frames_drawn` increments once per `App::update` call, but
/// `App::update` has THREE early-return paths that record nothing and skip
/// that increment: the settings-window dispatch branch (the settings window
/// has a windowing-side [`EguiState`]/`FrameProfile` but no app-side
/// `PerWindowState`, so it can never have a `frames_drawn` counter at all),
/// the dead-window/no-`PerWindowState` cleanup branch, and the
/// no-active-pane branch. Every one of those still runs inside a
/// `run_frame` call, so `frame_counter` increments regardless. Once any of
/// these paths fires for a window, `frame_counter` and `frames_drawn` for
/// that `window_id` permanently drift apart -- there is no resync -- and a
/// drift observed between the two for the same `window_id` means later
/// cross-crate phase comparisons for that window (e.g. the `phase_total -
/// (run_ui + tessellate + paint + swap)` residual against `freminal`'s
/// `phase_app_update`) are unreliable, since the two `FLUSH_EVERY`-frame
/// windows no longer cover the same set of frames.
///
/// All sums/counters are cumulative since window creation (never reset),
/// matching the existing `freminal::gui::window::FrameStats` idiom this
/// harness was modeled on -- `frame_counter` doubles as "the frame count
/// this flush's sums/maxima cover" for a running mean.
#[cfg(feature = "frame-profiling")]
#[derive(Debug, Default)]
struct FrameProfile {
    /// Frames rendered for this window since creation. One `run_frame` call
    /// == one drawn frame.
    frame_counter: u64,
    /// Cumulative time inside `self.ctx.run_ui(...)` (which itself calls
    /// into `App::update`, so this is an upper bound on freminal's own
    /// `update()` cost as measured from the windowing side).
    run_ui_total: std::time::Duration,
    /// Largest single-frame `run_ui` duration observed.
    run_ui_max: std::time::Duration,
    /// Cumulative wall-clock time across the WHOLE of `run_frame` (Task 121
    /// defect-4 fix): from entry to just before this struct's own flush
    /// check.
    ///
    /// `phase_total` minus the sum of `run_ui_total`, `tessellate_total`,
    /// `paint_total`, and `swap_total` (for the same window over the same
    /// frame window) is the unmeasured residual -- do NOT instrument each
    /// gap individually; the point of this field is to make that residual
    /// computable (and therefore visible) rather than silently absorbed
    /// into "everything else". The main contributors to that residual, all
    /// in this file and none separately measured:
    ///
    /// - `handle_platform_output` (feeding egui's output back to `egui-winit`)
    /// - the band-shape slice-to-owned-`Vec` clone (a full allocation + copy
    ///   every frame)
    /// - `gl_state.clear(clear_color)` (the actual GPU framebuffer clear)
    /// - the texture set/free loops (texture upload/free housekeeping
    ///   around the paint calls)
    phase_total_total: std::time::Duration,
    /// Largest single-frame `phase_total` duration observed.
    phase_total_max: std::time::Duration,
    /// Cumulative tessellation time: the band tessellation plus the
    /// head/tail chrome tessellation, summed as ONE total rather than split
    /// band-vs-head/tail -- see the call site comment for why.
    tessellate_total: std::time::Duration,
    /// Largest single-frame tessellation duration observed.
    tessellate_max: std::time::Duration,
    /// Cumulative time across the three `paint_primitives` calls
    /// (head, band, tail), summed.
    paint_total: std::time::Duration,
    /// Largest single-frame paint duration observed.
    paint_max: std::time::Duration,
    /// Cumulative time in `swap_buffers`/`swap_buffers_with_damage`.
    swap_total: std::time::Duration,
    /// Largest single-frame swap duration observed.
    swap_max: std::time::Duration,

    // ── Repaint-cause aggregation (feature-gated), reset every flush
    // window like the settle-gate diagnostics above (`reset_repaint_cause_window`)
    // rather than cumulative-since-creation -- the question this answers
    // ("what is asking for an immediate repaint on THIS kind of frame,
    // right now") only makes sense as "in the last `FLUSH_EVERY` frames",
    // the same reasoning as the settle-value fields.
    /// Occurrence count per `ctx.repaint_causes()` cause, keyed on the
    /// formatted `"{trimmed_file}:{line} {reason}"` string (see
    /// [`trim_cause_file_path`]) — e.g. distinguishing an egui-internal
    /// cause (`index.crates.io-.../egui-0.35.0/src/context.rs:1234 ...`)
    /// from a freminal call site
    /// (`freminal/src/gui/terminal/widget.rs:1936 ...`) is the whole
    /// point of this map. `BTreeMap`, not `HashMap`: deterministic
    /// (alphabetical) iteration order for the flush log, and
    /// `freminal-windowing` has no hash-map dependency to gain for this.
    ///
    /// **These are the PREVIOUS pass's causes, not this frame's own** — see
    /// [`egui::Context::repaint_causes`]'s doc and the call site in
    /// `run_frame` for why: `Context::begin_pass` (called from inside
    /// `run_ui`) swaps the just-finished pass's `causes` into `prev_causes`
    /// at the START of the pass that follows it, so `repaint_causes()`
    /// always lags by exactly one `run_frame` call. That is fine for
    /// aggregate counting over a 120-frame window (the lag is invisible in
    /// the aggregate); it would matter for attributing causes to a SPECIFIC
    /// single frame, which this harness does not attempt.
    repaint_cause_counts: std::collections::BTreeMap<String, u64>,

    // ── Task 121 pointer-motion repaint-gate spike (issue: pointer motion
    // over static terminal content measured at 58fps vs. 1.95fps idle, 95%
    // of those frames changing zero pixels) ──────────────────────────────
    //
    // Cumulative since window creation, like every other plain counter on
    // this struct. Incremented from `event_loop.rs`'s `CursorMoved` arm
    // (the only place the scheduling decision is made) via the
    // `record_pointer_frame_scheduled`/`record_pointer_frame_suppressed`
    // accessors below, NOT here — these fields stay private to `FrameProfile`
    // (see those accessors' docs for why a method, not a public field).
    /// `CursorMoved` events that scheduled a repaint (either the app's
    /// [`crate::App::pointer_motion_needs_repaint`] said so, the chrome-drag
    /// latch was set, or this was the one-frame edge-detect transition after
    /// a needed -> not-needed change).
    pointer_frames_scheduled: u64,
    /// `CursorMoved` events suppressed by the Task 121 gate — i.e. events
    /// that would have scheduled a repaint before this spike (egui-winit
    /// reports `repaint: true` unconditionally for `CursorMoved`) but did
    /// not need to, per the app's own state.
    pointer_frames_suppressed: u64,

    // ── 124.17/124.2: skip-clear + partial-present path attribution ─────
    //
    // `decide_partial_present` (pure, unit-tested) attributes every frame to
    // exactly one `PartialPresentDecision`, so these six counters are
    // mutually exclusive and sum to `frame_counter`. Cumulative since window
    // creation, like every other plain counter on this struct.
    /// Frames where the app reported [`crate::FrameDamage::None`] (Task
    /// 124.2): nothing changed at all, so nothing was presented -- no
    /// clear, no GL primitive paint, no `pre_present_notify`, no buffer
    /// swap. Deliberately distinct from [`Self::present_partial_not_requested`]
    /// (which DOES present, just fully).
    present_frame_damage_none: u64,
    /// Frames where the app reported [`crate::FrameDamage::Full`].
    present_partial_not_requested: u64,
    /// Frames where the app reported `Partial` with an empty rect list.
    present_partial_no_rects: u64,
    /// Frames where the app reported `Partial` with rects, but the surface
    /// cannot present a sub-region ([`PartialPresentSupport::Unsupported`]).
    present_partial_blocked_surface: u64,
    /// Frames where the app reported `Partial` with rects, the surface
    /// supports partial present, but `buffer_age() != 1`.
    present_partial_blocked_buffer_age: u64,
    /// Frames where the skip-clear + partial-present path was actually
    /// taken.
    present_partial_taken: u64,
    /// Histogram of the queried buffer age, bucketed by `min(age, 3)` (index
    /// 3 means "3 or more"). Sampled **only** where the age was actually
    /// queried — the app requested `Partial` with rects *and* the surface
    /// supports partial present — so this histogram's total equals
    /// `present_partial_blocked_buffer_age + present_partial_taken`, **not**
    /// `frame_counter`: a frame that never reached the age query (`Full`,
    /// empty rects, or a surface that cannot present a sub-region) is not
    /// represented here at all.
    buffer_age_histogram: [u64; 4],
}

#[cfg(feature = "frame-profiling")]
impl FrameProfile {
    /// Emit a `debug` summary once every this many drawn frames -- same
    /// cadence as `freminal::gui::window::FrameStats::FLUSH_EVERY` so the
    /// two crates' log lines are easy to correlate by eye.
    const FLUSH_EVERY: u64 = 120;

    /// Percentage of `(a + b)` that `b` represents -- e.g. what fraction of
    /// `CursorMoved` events were suppressed
    /// (`pointer_frames_scheduled`/`pointer_frames_suppressed`). Pure so
    /// it's unit-testable in isolation. `0.0` when no frames have been
    /// counted yet (rather than dividing by zero).
    fn duty_cycle_pct(a: u64, b: u64) -> f64 {
        let total = a.saturating_add(b);
        if total == 0 {
            return 0.0;
        }
        // `u64 -> f64` is lossy for very large counts (beyond 2^53), but a
        // live session's frame counters never approach that range;
        // `approx_as` is the established lossy-conversion idiom in this
        // file (see `scale_factor().approx_as::<f32>()` in `new()` above).
        let b_f: f64 = conv2::ConvUtil::approx_as(b).unwrap_or(0.0);
        let total_f: f64 = conv2::ConvUtil::approx_as(total).unwrap_or(1.0);
        (b_f / total_f) * 100.0
    }

    /// Mean of a cumulative `Duration` sum over `count` samples, as a
    /// `Duration`. Returns `Duration::ZERO` for `count == 0` rather than
    /// dividing by zero. Pure so it's unit-testable in isolation.
    ///
    /// Deliberately duplicated in `freminal::gui::window::FrameStats` rather
    /// than shared (reviewed and accepted) -- keep the two in sync by eye if
    /// either changes.
    fn mean_duration(total: std::time::Duration, count: u64) -> std::time::Duration {
        if count == 0 {
            return std::time::Duration::ZERO;
        }
        let count_f: f64 = conv2::ConvUtil::approx_as(count).unwrap_or(1.0);
        total.div_f64(count_f.max(1.0))
    }

    /// Record one `ctx.repaint_causes()` entry into `repaint_cause_counts`.
    /// Called once per returned cause (a frame that pushed the same cause
    /// twice -- e.g. two `request_repaint()` calls at the same call site in
    /// one pass -- increments the count twice, matching what
    /// `egui::Context` actually recorded: `causes.push(cause)` is
    /// unconditional, with no dedup, at `context.rs:153`).
    fn record_repaint_cause(&mut self, cause: &egui::RepaintCause) {
        let key = format!(
            "{}:{} {}",
            trim_cause_file_path(cause.file),
            cause.line,
            cause.reason
        );
        let counter = self.repaint_cause_counts.entry(key).or_insert(0);
        *counter = counter.saturating_add(1);
    }

    /// Clear the repaint-cause aggregation map at the end of a flush window
    /// -- see the field doc on `repaint_cause_counts` for why this is
    /// windowed rather than cumulative-since-creation.
    fn reset_repaint_cause_window(&mut self) {
        self.repaint_cause_counts.clear();
    }

    /// 124.17: record one queried buffer age into `buffer_age_histogram`,
    /// bucketed by `min(age, 3)` (index 3 means "3 or more"). Called only
    /// from the two [`PartialPresentDecision`] variants that actually
    /// queried the age (`BlockedByBufferAge` and `Taken`) — see the field
    /// doc on `buffer_age_histogram` for why its total is therefore NOT
    /// `frame_counter`.
    fn record_buffer_age(&mut self, age: u32) {
        let bucket: usize = age.min(3).value_into().unwrap_or(3);
        self.buffer_age_histogram[bucket] = self.buffer_age_histogram[bucket].saturating_add(1);
    }
}

/// Trim a [`egui::RepaintCause::file`] path down to its last
/// `KEEP_COMPONENTS` `/`-or-`\`-separated segments.
///
/// Egui-internal causes carry the registry cache's long, absolute-ish path
/// (e.g. `.../registry/src/index.crates.io-.../egui-0.35.0/src/context.rs`);
/// keeping the last four segments retains the registry-hash and
/// crate-name+version directory components (e.g.
/// `index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/context.rs`), which is
/// exactly what distinguishes an egui-internal cause from a freminal call
/// site (`freminal/src/gui/terminal/widget.rs`) -- the entire point of this
/// instrumentation. Paths with `KEEP_COMPONENTS` segments or fewer
/// (freminal's own workspace-relative `file!()` paths typically are) pass
/// through unchanged. Pure, so directly unit-testable.
#[cfg(feature = "frame-profiling")]
fn trim_cause_file_path(file: &str) -> String {
    const KEEP_COMPONENTS: usize = 4;
    let parts: Vec<&str> = file.split(['/', '\\']).collect();
    if parts.len() <= KEEP_COMPONENTS {
        file.to_string()
    } else {
        parts[parts.len() - KEEP_COMPONENTS..].join("/")
    }
}

/// Sum of all occurrence counts in a repaint-cause aggregation map -- the
/// total number of `ctx.repaint_causes()` entries recorded this flush
/// window, for comparison against the window's `frame_counter` (e.g. 120
/// frames producing 480 causes means ~4 requests/frame). Pure, so directly
/// unit-testable.
#[cfg(feature = "frame-profiling")]
fn total_repaint_cause_count(counts: &std::collections::BTreeMap<String, u64>) -> u64 {
    counts
        .values()
        .fold(0u64, |acc, count| acc.saturating_add(*count))
}

/// The top `n` entries of a repaint-cause aggregation map, ordered by
/// occurrence count descending. `BTreeMap::iter` yields keys in ascending
/// (alphabetical) order; a *stable* sort by count descending therefore
/// leaves ties ordered by key ascending, deterministically -- no
/// `HashMap`-style iteration-order nondeterminism to fight. Pure, so
/// directly unit-testable.
#[cfg(feature = "frame-profiling")]
fn top_repaint_causes(
    counts: &std::collections::BTreeMap<String, u64>,
    n: usize,
) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = counts.iter().map(|(k, v)| (k.clone(), *v)).collect();
    entries.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    entries.truncate(n);
    entries
}

/// Format a top-N repaint-cause list (see [`top_repaint_causes`]) as a
/// single readable `tracing` field: `"{count}x {cause}"` entries joined by
/// `"; "`, or the literal `"none"` when empty (matching the
/// `format_nonzero_signal_counts` "none when empty" idiom already used by
/// this harness's `freminal`-side counterpart). Pure, so directly
/// unit-testable.
#[cfg(feature = "frame-profiling")]
fn format_repaint_causes(entries: &[(String, u64)]) -> String {
    if entries.is_empty() {
        return "none".to_string();
    }
    entries
        .iter()
        .map(|(cause, count)| format!("{count}x {cause}"))
        .collect::<Vec<_>>()
        .join("; ")
}

impl EguiState {
    /// Create egui state for a window.
    pub(crate) fn new(window: &Window, gl_state: &GlState) -> Result<Self, Error> {
        let ctx = egui::Context::default();

        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            // Scale factor is inherently a float; `approx_as` is the lossy but
            // well-defined conversion. `1.0` fallback matches the default DPI.
            Some(window.scale_factor().approx_as::<f32>().unwrap_or(1.0)),
            None,
            None,
        );

        let painter = egui_glow::Painter::new(Arc::clone(&gl_state.glow_context), "", None, false)
            .map_err(|e| Error::GlContextCreation(format!("egui painter creation failed: {e}")))?;

        Ok(Self {
            ctx,
            winit_state,
            painter,
            modifier_tracker: ModifierTracker::default(),
            #[cfg(feature = "frame-profiling")]
            frame_profile: FrameProfile::default(),
            damage_history: DamageHistory::new(),
        })
    }

    /// Collect raw input from winit for the current frame.
    pub(crate) fn take_egui_input(&mut self, window: &Window) -> egui::RawInput {
        self.winit_state.take_egui_input(window)
    }

    /// Task 121 pointer-motion repaint-gate spike: record that a
    /// `CursorMoved` event scheduled a repaint.
    ///
    /// A small accessor rather than a public `frame_profile` field/counter:
    /// `event_loop.rs` (where the scheduling decision is made) has no other
    /// reason to reach into `FrameProfile`'s internals, and this keeps the
    /// counter itself private to this module alongside every other
    /// `FrameProfile` field. Logged on the existing per-window
    /// `FLUSH_EVERY`-frame flush line in [`Self::run_frame`] — see the
    /// `pointer_frames_scheduled`/`pointer_frames_suppressed` field docs on
    /// [`FrameProfile`] for why these counters live there (rather than on
    /// `event_loop.rs`'s own `WindowState`): they are logically part of the
    /// same per-window frame-profiling harness and this reuses its existing
    /// flush cadence/`window_id` tagging instead of standing up a second one.
    #[cfg(feature = "frame-profiling")]
    pub(crate) const fn record_pointer_frame_scheduled(&mut self) {
        self.frame_profile.pointer_frames_scheduled = self
            .frame_profile
            .pointer_frames_scheduled
            .saturating_add(1);
    }

    /// Task 121 pointer-motion repaint-gate spike: record that a
    /// `CursorMoved` event was suppressed (did not schedule a repaint) by
    /// the gate. See [`Self::record_pointer_frame_scheduled`]'s doc for why
    /// this is an accessor rather than a public field.
    #[cfg(feature = "frame-profiling")]
    pub(crate) const fn record_pointer_frame_suppressed(&mut self) {
        self.frame_profile.pointer_frames_suppressed = self
            .frame_profile
            .pointer_frames_suppressed
            .saturating_add(1);
    }

    /// Run a single egui frame and paint, using pre-collected raw input.
    ///
    /// Returns [`FrameOutput`] containing viewport commands and repaint timing.
    ///
    /// The frame-paint work itself (running `ui_fn`, deciding the
    /// skip-clear + partial-present path, tessellating and painting the
    /// head/band/tail split, texture housekeeping) lives in
    /// [`paint_frame`](crate::frame_paint::paint_frame), which is
    /// deliberately window-free so it can be driven by a test harness
    /// (124.19b). This function is the thin window-bound shell around it:
    /// it reads `window.inner_size()`, hands egui's platform output back to
    /// `egui-winit`, notifies the window before presenting, swaps buffers,
    /// and folds the (feature-gated) frame-profiling data into
    /// `self.frame_profile`.
    // too_many_lines: 124.19a extracted the head/band/tail paint sequence
    // into `paint_frame`, but the bulk of what remains here is the single
    // feature-gated `tracing::debug!` flush call (one field per counter,
    // ~30 fields) -- splitting that macro invocation into a helper would
    // not reduce coupling, it would just move the same field list one
    // frame further away from the accumulators it reads. Mirrors the
    // `too_many_lines` allow this body was extracted from.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn run_frame<F>(
        &mut self,
        window: &Window,
        gl_state: &GlState,
        clear_color: [f32; 4],
        raw_input: egui::RawInput,
        present_flag: Option<&std::sync::Arc<std::sync::Mutex<crate::PresentRegion>>>,
        ui_fn: F,
    ) -> FrameOutput
    where
        F: FnMut(&egui::Context, &glow::Context) -> crate::FrameSignals,
    {
        // Task 121 frame-profiling harness (defect-4 fix): wall-clock start
        // of the WHOLE frame, captured before anything else in `run_frame`
        // runs. Its `.elapsed()` (taken just before this window's flush
        // check, near the end of this function) is `phase_total` -- see the
        // `FrameProfile::phase_total_total` doc for what its residual
        // against `run_ui + tessellate + paint + swap` exposes.
        #[cfg(feature = "frame-profiling")]
        let total_start = std::time::Instant::now();

        let size = window.inner_size();

        let paint_output = paint_frame(
            gl_state,
            &self.ctx,
            &mut self.painter,
            PaintFrameRequest {
                size_px: [size.width, size.height],
                raw_input,
                clear_color,
                present_flag,
                damage_history: &mut self.damage_history,
            },
            ui_fn,
        );

        self.winit_state
            .handle_platform_output(window, paint_output.platform_output);

        #[cfg(feature = "frame-profiling")]
        let swap_start = std::time::Instant::now();
        // Task 124.2: `FramePresentation::None` means nothing changed this
        // frame -- `paint_frame` already skipped the clear and every GL
        // primitive paint, and per the plan this window-bound shell must
        // likewise skip `pre_present_notify` (no point notifying the
        // compositor about a frame with no new content) and the buffer
        // swap entirely, rather than swapping a framebuffer that was never
        // touched.
        let swap_result = match paint_output.presentation {
            FramePresentation::None => None,
            FramePresentation::Full => {
                // Pre-present notify for Wayland frame pacing
                window.pre_present_notify();
                Some(gl_state.swap_buffers())
            }
            FramePresentation::Partial(region) => {
                // Pre-present notify for Wayland frame pacing
                window.pre_present_notify();
                Some(gl_state.swap_buffers_with_damage(std::slice::from_ref(&region)))
            }
        };
        #[cfg(feature = "frame-profiling")]
        let swap_elapsed = swap_start.elapsed();
        if let Some(Err(e)) = swap_result {
            tracing::error!("swap_buffers failed: {e}");
        }

        let viewport_output = paint_output.viewport_output.get(&egui::ViewportId::ROOT);

        let repaint_delay = viewport_output.map_or(std::time::Duration::MAX, |vo| vo.repaint_delay);

        let commands = viewport_output
            .map(|vo| vo.commands.clone())
            .unwrap_or_default();

        // Do NOT call `window.request_redraw()` here — let the event loop
        // manage scheduling via `repaint_at` / `about_to_wait`.  Calling
        // `request_redraw()` directly bypasses `ControlFlow::WaitUntil` and
        // causes an unbounded render loop on platforms where `swap_buffers`
        // returns immediately (macOS with vsync disabled).

        // 124.17/124.2: fold the skip-clear + partial-present decision
        // `paint_frame` computed into the six mutually-exclusive counters --
        // moved here (rather than immediately after `paint_frame` returns)
        // so it sits next to the rest of the frame-profiling fold/flush
        // below; nothing reads `self.frame_profile` in between, so the
        // accumulated values are identical to updating them immediately.
        #[cfg(feature = "frame-profiling")]
        match paint_output.decision {
            PartialPresentDecision::NoPresentation => {
                self.frame_profile.present_frame_damage_none = self
                    .frame_profile
                    .present_frame_damage_none
                    .saturating_add(1);
            }
            PartialPresentDecision::NotRequested => {
                self.frame_profile.present_partial_not_requested = self
                    .frame_profile
                    .present_partial_not_requested
                    .saturating_add(1);
            }
            PartialPresentDecision::RequestedWithNoRects => {
                self.frame_profile.present_partial_no_rects = self
                    .frame_profile
                    .present_partial_no_rects
                    .saturating_add(1);
            }
            PartialPresentDecision::BlockedBySurface => {
                self.frame_profile.present_partial_blocked_surface = self
                    .frame_profile
                    .present_partial_blocked_surface
                    .saturating_add(1);
            }
            PartialPresentDecision::BlockedByBufferAge { age } => {
                self.frame_profile.present_partial_blocked_buffer_age = self
                    .frame_profile
                    .present_partial_blocked_buffer_age
                    .saturating_add(1);
                self.frame_profile.record_buffer_age(age);
            }
            PartialPresentDecision::Taken { age, .. } => {
                self.frame_profile.present_partial_taken =
                    self.frame_profile.present_partial_taken.saturating_add(1);
                // 124.18: record the REAL queried age (previously always
                // `1`, back when `Taken` was only reachable at `age == 1`)
                // -- the histogram exists precisely to show the actual
                // hardware distribution, which 124.17's GPU re-take found
                // is overwhelmingly `2`, not `1`.
                self.frame_profile.record_buffer_age(age);
            }
        }

        // Task 121 frame-profiling harness: flush this window's own
        // `tracing::debug!` line every `FrameProfile::FLUSH_EVERY` frames,
        // tagged with `window_id` (defect-3 fix) so a multi-window session's
        // interleaved lines can be told apart and matched against
        // `freminal`'s own per-window line for the same `window_id`. See the
        // `FrameProfile` doc for why `frame_counter` and `freminal`'s
        // `frames_drawn` are NOT guaranteed to stay in lockstep for a given
        // window (they can drift on `App::update`'s early-return paths).
        #[cfg(feature = "frame-profiling")]
        {
            // Fold `paint_frame`'s phase timings and repaint causes into
            // this window's accumulators (`+=` for totals, `.max()` for
            // maxima) exactly as `run_frame` did inline before `paint_frame`
            // was split out of it -- see that function's doc for why the
            // per-phase `Instant` spans stay where they are.
            self.frame_profile.run_ui_total += paint_output.profiling.run_ui;
            self.frame_profile.run_ui_max = self
                .frame_profile
                .run_ui_max
                .max(paint_output.profiling.run_ui);
            self.frame_profile.tessellate_total += paint_output.profiling.tessellate;
            self.frame_profile.tessellate_max = self
                .frame_profile
                .tessellate_max
                .max(paint_output.profiling.tessellate);
            self.frame_profile.paint_total += paint_output.profiling.paint;
            self.frame_profile.paint_max = self
                .frame_profile
                .paint_max
                .max(paint_output.profiling.paint);
            for cause in &paint_output.profiling.repaint_causes {
                self.frame_profile.record_repaint_cause(cause);
            }
            self.frame_profile.swap_total += swap_elapsed;
            self.frame_profile.swap_max = self.frame_profile.swap_max.max(swap_elapsed);

            // Defect 4: `phase_total` covers the WHOLE of `run_frame`,
            // taken as late as possible (just before this flush check) so it
            // includes every gap named on `FrameProfile::phase_total_total`'s
            // doc: `handle_platform_output`, the band-shape clone, the GL
            // clear, and the texture set/free loops.
            let phase_total_this_frame = total_start.elapsed();
            self.frame_profile.phase_total_total += phase_total_this_frame;
            self.frame_profile.phase_total_max = self
                .frame_profile
                .phase_total_max
                .max(phase_total_this_frame);

            self.frame_profile.frame_counter = self.frame_profile.frame_counter.saturating_add(1);
            if self
                .frame_profile
                .frame_counter
                .is_multiple_of(FrameProfile::FLUSH_EVERY)
            {
                let p = &self.frame_profile;
                // Defect 3: `window_id`, `{:?}` -- the same representation
                // `freminal`'s own `frame_profiling` line uses for its
                // `window_id` field, so the two crates' lines for the same
                // OS window can be matched by eye or by log tooling.
                tracing::debug!(
                    target: "freminal_windowing::frame_profiling",
                    window_id = ?crate::WindowId(window.id()),
                    frame_counter = p.frame_counter,
                    phase_total_total_us = p.phase_total_total.as_micros(),
                    phase_total_max_us = p.phase_total_max.as_micros(),
                    phase_total_mean_us =
                        FrameProfile::mean_duration(p.phase_total_total, p.frame_counter)
                            .as_micros(),
                    run_ui_total_us = p.run_ui_total.as_micros(),
                    run_ui_max_us = p.run_ui_max.as_micros(),
                    run_ui_mean_us =
                        FrameProfile::mean_duration(p.run_ui_total, p.frame_counter).as_micros(),
                    tessellate_total_us = p.tessellate_total.as_micros(),
                    tessellate_max_us = p.tessellate_max.as_micros(),
                    tessellate_mean_us =
                        FrameProfile::mean_duration(p.tessellate_total, p.frame_counter)
                            .as_micros(),
                    paint_total_us = p.paint_total.as_micros(),
                    paint_max_us = p.paint_max.as_micros(),
                    paint_mean_us =
                        FrameProfile::mean_duration(p.paint_total, p.frame_counter).as_micros(),
                    swap_total_us = p.swap_total.as_micros(),
                    swap_max_us = p.swap_max.as_micros(),
                    swap_mean_us =
                        FrameProfile::mean_duration(p.swap_total, p.frame_counter).as_micros(),
                    // Repaint-cause aggregation (task 121 defect-5): what,
                    // exactly, requested an immediate/zero-delay repaint
                    // this flush window -- egui-internal machinery vs. a
                    // freminal call site -- and how many requests total,
                    // for comparison against `frame_counter` (e.g. 120
                    // frames producing 480 causes means ~4 requests/frame).
                    // These are the PREVIOUS pass's causes, one frame
                    // lagged -- see the `repaint_cause_counts` field doc.
                    repaint_cause_total = total_repaint_cause_count(&p.repaint_cause_counts),
                    repaint_cause_top8 =
                        %format_repaint_causes(&top_repaint_causes(&p.repaint_cause_counts, 8)),
                    // Task 121 pointer-motion repaint-gate spike: how many
                    // `CursorMoved` events scheduled vs. were suppressed,
                    // cumulative since window creation. Incremented from
                    // `event_loop.rs` via `record_pointer_frame_scheduled`/
                    // `record_pointer_frame_suppressed`.
                    pointer_frames_scheduled = p.pointer_frames_scheduled,
                    pointer_frames_suppressed = p.pointer_frames_suppressed,
                    pointer_suppressed_duty_cycle_pct = FrameProfile::duty_cycle_pct(
                        p.pointer_frames_scheduled,
                        p.pointer_frames_suppressed
                    ),
                    // 124.17/124.2: skip-clear + partial-present path
                    // attribution, cumulative since window creation.
                    // Mutually exclusive and sum to `frame_counter`.
                    // `buffer_age_histogram`'s total is
                    // `present_partial_blocked_buffer_age +
                    // present_partial_taken`, NOT `frame_counter` — see that
                    // field's doc.
                    present_frame_damage_none = p.present_frame_damage_none,
                    present_partial_not_requested = p.present_partial_not_requested,
                    present_partial_no_rects = p.present_partial_no_rects,
                    present_partial_blocked_surface = p.present_partial_blocked_surface,
                    present_partial_blocked_buffer_age = p.present_partial_blocked_buffer_age,
                    present_partial_taken = p.present_partial_taken,
                    buffer_age_histogram = ?p.buffer_age_histogram,
                    "windowing frame-profiling stats (task 121 harness): the \
                     windowing-owned phase_total/run_ui/tessellate/paint/swap \
                     wall-clock split over frame_counter drawn frames for this \
                     window_id (phase_total minus (run_ui + tessellate + paint + \
                     swap) is the unmeasured residual -- see \
                     FrameProfile::phase_total_total), the top 8 \
                     ctx.repaint_causes() by occurrence count this flush window \
                     plus their total (repaint_cause_top8/repaint_cause_total), \
                     the Task 121 pointer-motion-suppression-spike counters \
                     (pointer_frames_scheduled/pointer_frames_suppressed/\
                     pointer_suppressed_duty_cycle_pct, cumulative since window \
                     creation), and the 124.17/124.2 skip-clear + \
                     partial-present path attribution (present_frame_damage_none \
                     plus present_partial_*, cumulative, mutually exclusive, \
                     summing to frame_counter; buffer_age_histogram bucketed by \
                     min(age, 3), summing to present_partial_blocked_buffer_age + \
                     present_partial_taken instead)"
                );

                // The repaint-cause aggregation map is windowed, not
                // cumulative-since-creation (see its field doc) -- clear it
                // now that this window's line has been logged.
                self.frame_profile.reset_repaint_cause_window();
            }
        }

        FrameOutput {
            commands,
            repaint_delay,
            app_requested_delay: paint_output.terminal_requested_delay,
        }
    }

    /// Pass a winit `WindowEvent` to egui.
    ///
    /// Forward a window event to egui-winit.
    pub(crate) fn on_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        // Mirror the modifier state before handing the event to egui-winit.
        //
        // Note what this does NOT claim: `event_loop::window_event` reads
        // `modifiers()` at its interception paths *before* it reaches this
        // call for the event in hand (and may early-return without reaching
        // it at all). Correctness comes from the events being different ones
        // -- modifier state arrives as its own `ModifiersChanged`, which no
        // interception path claims, so it lands here during an earlier
        // `window_event` call than the `KeyboardInput` that reads the result.
        // See `ModifierTracker`'s module doc for the full invariant and the
        // one way to break it.
        self.modifier_tracker.on_window_event(event);
        self.winit_state.on_window_event(window, event)
    }

    /// Inject a paste event directly into egui's input queue.
    pub(crate) fn inject_paste(&mut self, text: String) {
        self.winit_state
            .egui_input_mut()
            .events
            .push(egui::Event::Paste(text));
    }

    /// Read clipboard text via this window's egui-winit clipboard.
    pub(crate) fn clipboard_text(&mut self) -> Option<String> {
        self.winit_state.clipboard_text()
    }

    /// Read the current egui modifier state.
    ///
    /// Sourced from [`ModifierTracker`], not from egui: egui 0.36 removed
    /// `RawInput::modifiers`, and `Context::input(|i| i.modifiers)` only
    /// advances when a pass runs, so it would report last frame's modifiers to
    /// the pre-egui interception paths that call this.
    pub(crate) const fn modifiers(&self) -> egui::Modifiers {
        self.modifier_tracker.current()
    }

    /// Free the painter's OpenGL resources.
    ///
    /// Must be called while this window's GL context is current and before the
    /// painter is dropped. `egui_glow::Painter::destroy` is idempotent (guarded
    /// by an internal `destroyed` flag), so calling it more than once is safe.
    pub(crate) fn destroy_painter(&mut self) {
        self.painter.destroy();
    }
}

#[cfg(all(test, feature = "frame-profiling"))]
mod frame_profiling_tests {
    use super::FrameProfile;
    use std::time::Duration;

    #[test]
    fn duty_cycle_is_zero_with_no_frames() {
        assert!((FrameProfile::duty_cycle_pct(0, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_zero_when_b_never_engages() {
        assert!((FrameProfile::duty_cycle_pct(120, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_100_when_always_b() {
        let pct = FrameProfile::duty_cycle_pct(0, 120);
        assert!((pct - 100.0).abs() < 0.001, "pct was {pct}");
    }

    #[test]
    fn duty_cycle_is_25_for_3_a_1_b() {
        let pct = FrameProfile::duty_cycle_pct(90, 30);
        assert!((pct - 25.0).abs() < 0.001, "pct was {pct}");
    }

    #[test]
    fn mean_duration_is_zero_with_no_samples() {
        assert_eq!(
            FrameProfile::mean_duration(Duration::from_millis(500), 0),
            Duration::ZERO
        );
    }

    #[test]
    fn mean_duration_divides_evenly() {
        let mean = FrameProfile::mean_duration(Duration::from_micros(2400), 120);
        assert_eq!(mean, Duration::from_micros(20));
    }

    #[test]
    fn mean_duration_handles_a_single_sample() {
        let mean = FrameProfile::mean_duration(Duration::from_micros(42), 1);
        assert_eq!(mean, Duration::from_micros(42));
    }

    // ── `trim_cause_file_path` ───────────────────────────────────────────

    #[test]
    fn trim_cause_file_path_leaves_a_short_path_unchanged() {
        // freminal's own `file!()` paths are workspace-relative and
        // typically well under the 4-segment keep threshold.
        assert_eq!(
            super::trim_cause_file_path("freminal/src/main.rs"),
            "freminal/src/main.rs"
        );
    }

    #[test]
    fn trim_cause_file_path_leaves_exactly_four_segments_unchanged() {
        assert_eq!(
            super::trim_cause_file_path("freminal/src/gui/widget.rs"),
            "freminal/src/gui/widget.rs"
        );
    }

    #[test]
    fn trim_cause_file_path_keeps_last_four_segments_of_a_long_registry_path() {
        // A realistic egui-0.35.0 registry cache path -- the crate
        // name+version directory component must survive the trim so it
        // remains distinguishable from a freminal source path.
        let long_path = "/home/fred/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/context.rs";
        assert_eq!(
            super::trim_cause_file_path(long_path),
            "index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/context.rs"
        );
    }

    #[test]
    fn trim_cause_file_path_handles_backslash_separated_windows_paths() {
        let long_path =
            r"C:\Users\fred\.cargo\registry\src\index.crates.io-abc\egui-0.35.0\src\context.rs";
        assert_eq!(
            super::trim_cause_file_path(long_path),
            "index.crates.io-abc/egui-0.35.0/src/context.rs"
        );
    }

    // ── `total_repaint_cause_count` ──────────────────────────────────────

    #[test]
    fn total_repaint_cause_count_is_zero_for_an_empty_map() {
        let counts = std::collections::BTreeMap::new();
        assert_eq!(super::total_repaint_cause_count(&counts), 0);
    }

    #[test]
    fn total_repaint_cause_count_sums_all_entries() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("a".to_string(), 3u64);
        counts.insert("b".to_string(), 5u64);
        counts.insert("c".to_string(), 2u64);
        assert_eq!(super::total_repaint_cause_count(&counts), 10);
    }

    // ── `top_repaint_causes` ─────────────────────────────────────────────

    #[test]
    fn top_repaint_causes_orders_by_count_descending() {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("rare".to_string(), 1u64);
        counts.insert("common".to_string(), 100u64);
        counts.insert("medium".to_string(), 10u64);
        let top = super::top_repaint_causes(&counts, 8);
        assert_eq!(
            top,
            vec![
                ("common".to_string(), 100),
                ("medium".to_string(), 10),
                ("rare".to_string(), 1),
            ]
        );
    }

    #[test]
    fn top_repaint_causes_truncates_to_n() {
        let mut counts = std::collections::BTreeMap::new();
        for i in 0..20u64 {
            counts.insert(format!("cause_{i:02}"), i);
        }
        let top = super::top_repaint_causes(&counts, 8);
        assert_eq!(top.len(), 8);
        // The 8 largest counts (19..=12) must be present, descending.
        let expected_counts: Vec<u64> = (12..20).rev().collect();
        let actual_counts: Vec<u64> = top.iter().map(|(_, c)| *c).collect();
        assert_eq!(actual_counts, expected_counts);
    }

    #[test]
    fn top_repaint_causes_breaks_ties_by_key_ascending_deterministically() {
        // Three entries tied at count 5: BTreeMap::iter yields them
        // alphabetically ("a" < "b" < "c"), and a stable sort by count
        // descending must preserve that relative order for the tie.
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("c_cause".to_string(), 5u64);
        counts.insert("a_cause".to_string(), 5u64);
        counts.insert("b_cause".to_string(), 5u64);
        let top = super::top_repaint_causes(&counts, 8);
        assert_eq!(
            top,
            vec![
                ("a_cause".to_string(), 5),
                ("b_cause".to_string(), 5),
                ("c_cause".to_string(), 5),
            ],
            "ties must break by key ascending, deterministically -- re-running \
             must always produce this exact order"
        );
    }

    #[test]
    fn top_repaint_causes_handles_an_empty_map() {
        let counts = std::collections::BTreeMap::new();
        assert_eq!(super::top_repaint_causes(&counts, 8), Vec::new());
    }

    // ── `format_repaint_causes` ───────────────────────────────────────────

    #[test]
    fn format_repaint_causes_is_literal_none_when_empty() {
        assert_eq!(super::format_repaint_causes(&[]), "none");
    }

    #[test]
    fn format_repaint_causes_formats_a_realistic_example() {
        let entries = vec![
            (
                "index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/context.rs:1879 ".to_string(),
                2846u64,
            ),
            (
                "freminal/src/gui/terminal/widget.rs:1936 ".to_string(),
                12u64,
            ),
        ];
        assert_eq!(
            super::format_repaint_causes(&entries),
            "2846x index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/context.rs:1879 ; \
             12x freminal/src/gui/terminal/widget.rs:1936 "
        );
    }

    // ── 124.17: `FrameProfile::record_buffer_age` ────────────────────────

    #[test]
    fn record_buffer_age_buckets_ages_zero_one_two_exactly() {
        let mut profile = FrameProfile::default();
        profile.record_buffer_age(0);
        profile.record_buffer_age(1);
        profile.record_buffer_age(2);
        assert_eq!(profile.buffer_age_histogram, [1, 1, 1, 0]);
    }

    #[test]
    fn record_buffer_age_buckets_three_and_above_into_index_three() {
        let mut profile = FrameProfile::default();
        profile.record_buffer_age(3);
        profile.record_buffer_age(4);
        profile.record_buffer_age(1000);
        assert_eq!(profile.buffer_age_histogram, [0, 0, 0, 3]);
    }
}

#[cfg(test)]
mod tests {
    use egui::epaint::Primitive;
    use egui::{Color32, Rect, pos2, vec2};

    /// Sum the vertex/index counts across every `Mesh` primitive in a
    /// tessellation result. `Callback` primitives (paint callbacks) carry no
    /// mesh data of their own, so they are not part of this count; the test
    /// below paints only `rect_filled` shapes, which always tessellate to
    /// `Mesh` primitives, so no callback primitives appear.
    fn total_verts_indices(primitives: &[egui::ClippedPrimitive]) -> (usize, usize) {
        let mut vertices = 0;
        let mut indices = 0;
        for clipped in primitives {
            if let Primitive::Mesh(mesh) = &clipped.primitive {
                vertices += mesh.vertices.len();
                indices += mesh.indices.len();
            }
        }
        (vertices, indices)
    }

    /// A single mesh vertex flattened to comparable primitives (egui's
    /// `Vertex`/`Pos2`/`Color32` are `PartialEq`, but bundling the fields
    /// makes the assertion failure message readable and avoids depending on
    /// `Vertex: PartialEq` staying derived). Field order: position, uv, color.
    type FlatVertex = ([f32; 2], [f32; 2], [u8; 4]);

    /// Flatten a primitive list into an ORDERED sequence of vertices and an
    /// ORDERED sequence of indices (offset so indices are global across the
    /// whole list, matching how the meshes would be drawn back-to-back).
    /// Comparing these sequences — not just their lengths — pins that the
    /// 3-call split preserves geometry *order*, which is the exact property
    /// `paint_primitives`' head->band->tail sequencing depends on.
    fn flatten_mesh_geometry(primitives: &[egui::ClippedPrimitive]) -> (Vec<FlatVertex>, Vec<u32>) {
        let mut verts: Vec<FlatVertex> = Vec::new();
        let mut idxs: Vec<u32> = Vec::new();
        for clipped in primitives {
            if let Primitive::Mesh(mesh) = &clipped.primitive {
                let base = u32::try_from(verts.len()).unwrap_or(u32::MAX);
                for v in &mesh.vertices {
                    verts.push((
                        [v.pos.x, v.pos.y],
                        [v.uv.x, v.uv.y],
                        [v.color.r(), v.color.g(), v.color.b(), v.color.a()],
                    ));
                }
                for &i in &mesh.indices {
                    idxs.push(base + i);
                }
            }
        }
        (verts, idxs)
    }

    /// Pins the losslessness of `run_frame`'s head/band/tail split
    /// (#436.4a): tessellating `full_output.shapes` as three slices
    /// (`[..start]`, `[start..end]`, `[end..]`) and summing the resulting
    /// primitives' vertex/index counts must equal tessellating the whole
    /// list at once. `egui::Context::tessellate` builds a fresh
    /// `Tessellator` per call from only `pixels_per_point`,
    /// `tessellation_options`, and the font texture atlas size — none of
    /// which vary between the whole-list call and the three sliced calls —
    /// so per-shape tessellation is independent of what else is in the
    /// list. The *batching* of tessellated meshes into `ClippedPrimitive`s
    /// may differ (adjacent same-clip-rect shapes can merge into fewer,
    /// larger meshes when tessellated together), but the underlying vertex
    /// and index data — and therefore the pixels drawn — must be identical
    /// either way. This is the property `run_frame`'s 3-call paint depends
    /// on for byte-identical rendering.
    #[test]
    fn head_band_tail_split_is_lossless_vs_whole_tessellation() {
        let ctx = egui::Context::default();
        let pixels_per_point = 1.0;

        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // Shape 0: "head" (chrome painted before the band).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
                0.0,
                Color32::RED,
            );
            // Shapes 1-2: "band" (terminal content).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(5.0, 5.0)),
                0.0,
                Color32::GREEN,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(20.0, 20.0), vec2(5.0, 5.0)),
                0.0,
                Color32::BLUE,
            );
            // Shape 3: "tail" (chrome painted after the band).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(30.0, 30.0), vec2(5.0, 5.0)),
                0.0,
                Color32::YELLOW,
            );
        });
        full_output.textures_delta.clear();

        let shapes = full_output.shapes;
        assert_eq!(shapes.len(), 4, "sanity: exactly the four shapes painted");

        let whole_primitives = ctx.tessellate(shapes.clone(), pixels_per_point);

        // Band range covering shapes 1..3 (the green and blue rects), as
        // `App::take_terminal_band_range` would report.
        let start = 1;
        let end = 3;
        let head_shapes = shapes[..start].to_vec();
        let band_shapes = shapes[start..end].to_vec();
        let tail_shapes = shapes[end..].to_vec();

        let head_primitives = ctx.tessellate(head_shapes, pixels_per_point);
        let band_primitives = ctx.tessellate(band_shapes, pixels_per_point);
        let tail_primitives = ctx.tessellate(tail_shapes, pixels_per_point);

        let (whole_vertices, whole_indices) = total_verts_indices(&whole_primitives);
        let (head_vertices, head_indices) = total_verts_indices(&head_primitives);
        let (band_vertices, band_indices) = total_verts_indices(&band_primitives);
        let (tail_vertices, tail_indices) = total_verts_indices(&tail_primitives);

        assert_eq!(
            whole_vertices,
            head_vertices + band_vertices + tail_vertices,
            "split tessellation must produce the same total vertex count as \
             tessellating the whole shape list at once"
        );
        assert_eq!(
            whole_indices,
            head_indices + band_indices + tail_indices,
            "split tessellation must produce the same total index count as \
             tessellating the whole shape list at once"
        );
        assert!(
            whole_vertices > 0,
            "sanity: the shapes actually tessellated to something"
        );

        // Stronger than counts: the ORDERED vertex/index sequences must be
        // identical. head ++ band ++ tail (concatenated in paint order, with
        // indices re-based across the concatenation) must equal the whole
        // list's geometry vertex-for-vertex and index-for-index. This is the
        // exact property `run_frame`'s head->band->tail `paint_primitives`
        // sequence relies on for byte-identical pixels — counts matching
        // alone would not rule out a reordering.
        let (whole_verts, whole_idxs) = flatten_mesh_geometry(&whole_primitives);

        let mut split_primitives = head_primitives;
        split_primitives.extend(band_primitives);
        split_primitives.extend(tail_primitives);
        let (split_verts, split_idxs) = flatten_mesh_geometry(&split_primitives);

        assert_eq!(
            whole_verts, split_verts,
            "split tessellation must produce the same vertex SEQUENCE (order \
             included) as the whole-list tessellation"
        );
        assert_eq!(
            whole_idxs, split_idxs,
            "split tessellation must produce the same index SEQUENCE (order \
             included) as the whole-list tessellation"
        );
    }

    /// Confirms the `0..0` default `band_range` (an app that has not wired
    /// up `App::take_terminal_band_range`) behaves as `run_frame` assumes:
    /// `head_shapes` and `band_shapes` are empty, and `tail_shapes` is the
    /// ENTIRE shape list — i.e. painting all shapes as a single "tail"
    /// `paint_primitives` call, byte-identical to the pre-#436.4a
    /// single-call path.
    #[test]
    fn default_band_range_puts_everything_in_tail() {
        let ctx = egui::Context::default();

        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
                0.0,
                Color32::RED,
            );
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(5.0, 5.0)),
                0.0,
                Color32::GREEN,
            );
        });
        full_output.textures_delta.clear();

        let shapes = full_output.shapes;
        assert_eq!(shapes.len(), 2, "sanity: exactly the two shapes painted");

        let band_range: std::ops::Range<usize> = 0..0;
        let start = band_range.start.min(shapes.len());
        let end = band_range.end.clamp(start, shapes.len());

        let head_shapes = &shapes[..start];
        let band_shapes = &shapes[start..end];
        let tail_shapes = &shapes[end..];

        assert!(head_shapes.is_empty());
        assert!(band_shapes.is_empty());
        assert_eq!(tail_shapes.len(), shapes.len());
    }

    /// Extract, in order, the `rect` of every `Primitive::Callback` in a
    /// tessellated primitive list. `run_frame`'s band contains GL
    /// `PaintCallback`s (the pre-clear FBO callback, one per-pane draw
    /// callback per pane, and the post-shader composite callback); their
    /// `rect` is a stable, headlessly-observable identity we can assert
    /// order/containment against without a GL context.
    fn callback_rects(primitives: &[egui::ClippedPrimitive]) -> Vec<Rect> {
        primitives
            .iter()
            .filter_map(|clipped| match &clipped.primitive {
                Primitive::Callback(cb) => Some(cb.rect),
                Primitive::Mesh(_) => None,
            })
            .collect()
    }

    /// #436.5: the terminal band's GL `PaintCallback`s (pre-clear FBO,
    /// per-pane draw(s), post-shader composite) must stay CONTIGUOUS and IN
    /// ORDER inside the band slice across the head/band/tail split, so their
    /// offscreen-FBO round-trip is never interrupted by a chrome
    /// `paint_primitives` call. This is the property "Finding A" relies on:
    /// because `band_shape_start` is captured before the pre-clear callback
    /// and `band_shape_end` after the post-shader callback (`app_impl.rs`),
    /// all three callback kinds fall inside the band's contiguous shape
    /// range by construction, and egui's tessellator (verified against
    /// epaint 0.35: `tessellate_clipped_shape` emits each `Shape::Callback`
    /// as its own `Primitive::Callback`, never merged into an adjacent mesh,
    /// in input order) preserves that.
    ///
    /// This is a data-shape/ordering test only — it does NOT invoke the
    /// callbacks or exercise any GL. The closures are inert. Real FBO-state
    /// atomicity and pixel output are GPU-bound and deferred to 436.9's
    /// pixel harness (no headless-GL harness exists in-repo).
    #[test]
    fn band_gl_callbacks_stay_contiguous_and_ordered_across_the_split() {
        use std::sync::Arc;

        // Distinguishable rects identify each callback by position in the
        // tessellated output (their `rect` survives tessellation verbatim).
        let preclear_rect = Rect::from_min_size(pos2(1.0, 0.0), vec2(100.0, 100.0));
        let pane0_rect = Rect::from_min_size(pos2(2.0, 0.0), vec2(40.0, 40.0));
        let pane1_rect = Rect::from_min_size(pos2(3.0, 0.0), vec2(40.0, 40.0));
        let postshader_rect = Rect::from_min_size(pos2(4.0, 0.0), vec2(100.0, 100.0));

        let make_cb = |rect: Rect| egui::PaintCallback {
            rect,
            // Inert closure — never invoked in this headless test; mirrors
            // production's `Arc::new(egui_glow::CallbackFn::new(move |info,
            // painter| { .. }))` construction shape (app_impl.rs:1876,2427).
            callback: Arc::new(egui_glow::CallbackFn::new(|_info, _painter| {})),
        };

        let ctx = egui::Context::default();
        let pixels_per_point = 1.0;

        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // HEAD: chrome painted before the band (menu/tab bar stand-in).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(5.0, 5.0)),
                0.0,
                Color32::RED,
            );

            // BAND begins here — mirrors app_impl.rs's terminal-band order:
            // pre-clear FBO callback, then per-pane draw callbacks, then the
            // post-shader composite callback, then a pane-border rect
            // (Band-C chrome, still inside the band range).
            ui.painter().add(make_cb(preclear_rect));
            ui.painter().add(make_cb(pane0_rect));
            ui.painter().add(make_cb(pane1_rect));
            ui.painter().add(make_cb(postshader_rect));
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(10.0, 10.0), vec2(5.0, 5.0)),
                0.0,
                Color32::GREEN,
            );

            // TAIL: chrome painted after the band (overlay/tooltip stand-in).
            ui.painter().rect_filled(
                Rect::from_min_size(pos2(30.0, 30.0), vec2(5.0, 5.0)),
                0.0,
                Color32::YELLOW,
            );
        });
        full_output.textures_delta.clear();

        let shapes = full_output.shapes;
        // 1 head rect + 4 callbacks + 1 border rect + 1 tail rect.
        assert_eq!(shapes.len(), 7, "sanity: exactly the shapes painted");

        // Band range: from the first callback (index 1) through the border
        // rect (index 5, exclusive end 6) — as `band_shape_start`/
        // `band_shape_end` would bracket it in production.
        let start = 1;
        let end = 6;
        let head_primitives = ctx.tessellate(shapes[..start].to_vec(), pixels_per_point);
        let band_primitives = ctx.tessellate(shapes[start..end].to_vec(), pixels_per_point);
        let tail_primitives = ctx.tessellate(shapes[end..].to_vec(), pixels_per_point);

        // The band slice contains exactly the four callbacks, in order.
        assert_eq!(
            callback_rects(&band_primitives),
            vec![preclear_rect, pane0_rect, pane1_rect, postshader_rect],
            "the band must contain pre-clear -> pane0 -> pane1 -> post-shader \
             callbacks, contiguous and in order"
        );
        // No callback leaks into head or tail.
        assert!(
            callback_rects(&head_primitives).is_empty(),
            "no GL callback may fall in the chrome_head slice"
        );
        assert!(
            callback_rects(&tail_primitives).is_empty(),
            "no GL callback may fall in the chrome_tail slice"
        );

        // Splitting must not drop, duplicate, or reorder callbacks vs.
        // tessellating the whole list at once.
        let whole_primitives = ctx.tessellate(shapes, pixels_per_point);
        assert_eq!(
            callback_rects(&whole_primitives),
            vec![preclear_rect, pane0_rect, pane1_rect, postshader_rect],
            "the whole-list tessellation must carry the same callbacks in the \
             same order the split does"
        );
    }
}
