// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! egui integration: input translation and rendering via `egui-winit` and `egui_glow`.

use std::sync::Arc;

use conv2::ConvUtil;
use winit::window::Window;

use crate::error::Error;
use crate::gl_context::GlState;

/// Output from a single egui frame.
pub struct FrameOutput {
    /// Viewport commands emitted by the app during this frame.
    pub commands: Vec<egui::ViewportCommand>,
    /// Requested repaint delay (`Duration::MAX` = no repaint needed).
    pub repaint_delay: std::time::Duration,
}

/// Cached tessellated chrome (head + tail) primitives from the most recent
/// FULL frame (#436.4a/#436.4b/#436.4c).
///
/// Populated at the end of every FULL frame; consulted (via
/// [`decide_chrome_mode`]) to decide whether a later frame may REPLAY, and
/// when it does, `run_frame` paints `head_primitives`/`tail_primitives`
/// directly instead of re-tessellating. The source `head_shapes`/
/// `tail_shapes` are retained too (#436.4c, §5.2): a REPLAY frame that
/// detects the font atlas grew this frame (a new glyph shaped by the
/// terminal band, e.g.) re-tessellates these cached shapes against the
/// now-current atlas rather than painting stale UVs baked against the old
/// (smaller) atlas — see the `atlas_grew`/self-heal handling in
/// [`EguiState::run_frame`].
struct ChromeCache {
    /// The chrome shapes painted *before* the terminal band in
    /// `full_output.shapes` (e.g. the `CentralPanel` background fill, menu
    /// bar, tab bar), as recorded on the last FULL frame. Retained (#436.4c)
    /// so the §5.2 atlas-resize self-heal can re-tessellate them without
    /// re-running `update()`.
    head_shapes: Vec<egui::epaint::ClippedShape>,
    /// The chrome shapes painted *after* the terminal band (overlays, pane
    /// borders drawn as part of chrome, modals, tooltips), as recorded on
    /// the last FULL frame. Retained for the same reason as `head_shapes`.
    tail_shapes: Vec<egui::epaint::ClippedShape>,
    /// Tessellation of `head_shapes`, at `ppp`/`size`.
    head_primitives: Vec<egui::ClippedPrimitive>,
    /// Tessellation of `tail_shapes`, at `ppp`/`size`.
    tail_primitives: Vec<egui::ClippedPrimitive>,
    /// `pixels_per_point` the cached primitives were tessellated at. A
    /// mismatch on a later frame invalidates the cache.
    ppp: f32,
    /// Physical framebuffer size the cached primitives were tessellated
    /// for. A mismatch on a later frame invalidates the cache.
    size: [u32; 2],
}

/// Per-window egui state.
pub struct EguiState {
    pub(crate) ctx: egui::Context,
    pub(crate) winit_state: egui_winit::State,
    pub(crate) painter: egui_glow::Painter,
    /// Cached chrome primitives from the last FULL frame — see [`ChromeCache`].
    chrome_cache: Option<ChromeCache>,
    /// The repaint delay `run_frame` returned last frame. Consulted, via
    /// [`chrome_repaint_settled`], to decide this frame's [`crate::ChromeMode`].
    prev_repaint_delay: std::time::Duration,
    /// The chrome-damage decision drained last frame (from
    /// `App::take_chrome_damage`). Consulted to decide this frame's
    /// [`crate::ChromeMode`]: a REPLAY is only permitted when the PREVIOUS
    /// frame reported [`crate::ChromeDamage::Unchanged`].
    prev_chrome_damage: crate::ChromeDamage,
    /// The delay the app itself requested via `ctx.request_repaint_after`
    /// last frame (from `App::take_terminal_requested_delay`), if any.
    /// Consulted, via [`chrome_repaint_settled`], alongside
    /// `prev_repaint_delay` to decide this frame's [`crate::ChromeMode`].
    prev_terminal_requested_delay: Option<std::time::Duration>,
    /// Task 121 frame-profiling harness (feature-gated): per-window
    /// accumulated phase timings and `ChromeMode` counters, flushed as a
    /// `tracing::debug!` line every [`FrameProfile::FLUSH_EVERY`] frames.
    /// See the module-level rationale on [`FrameProfile`] itself.
    #[cfg(feature = "frame-profiling")]
    frame_profile: FrameProfile,
}

/// Task 121 frame-profiling harness (feature-gated only -- see the
/// `frame-profiling` Cargo feature on this crate and on `freminal`, which
/// enables it here).
///
/// Accumulates, per window, wall-clock phase timings for the windowing
/// layer's own share of `run_frame` (deciding `chrome_mode`, running
/// `ctx.run_ui`, tessellating, painting, swapping buffers, and the whole
/// frame as `phase_total`) plus a count of how often each `ChromeMode` was
/// actually chosen this session. Flushed as its own `tracing::debug!` line
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
    /// Frames where [`decide_chrome_mode`] chose [`crate::ChromeMode::Full`].
    chrome_mode_full: u64,
    /// Frames where [`decide_chrome_mode`] chose [`crate::ChromeMode::Replay`].
    /// Cross-checkable against `freminal`'s own `chrome_mode_replay` counter
    /// (sourced from the SAME `chrome_mode` value, passed into `App::update`)
    /// -- if the two crates' counts ever disagree, that disagreement is
    /// itself a finding (a `chrome_mode` mismatch between what `run_frame`
    /// decided and what the app observed).
    chrome_mode_replay: u64,
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
    /// - `atlas_grew`'s texture-delta scan
    /// - `gl_state.clear(clear_color)` (the actual GPU framebuffer clear)
    /// - the texture set/free loops (texture upload/free housekeeping
    ///   around the paint calls)
    phase_total_total: std::time::Duration,
    /// Largest single-frame `phase_total` duration observed.
    phase_total_max: std::time::Duration,
    /// Cumulative tessellation time: the band tessellation (always run) plus
    /// head/tail tessellation (FULL frames, and REPLAY frames that hit the
    /// atlas-grew self-heal path), summed as ONE total rather than split
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

    // ── Gate-blocker counters (feature-gated): issue #459/#461 follow-up ──
    //
    // `decide_chrome_mode` permits REPLAY only when ALL FOUR of
    // `ChromeGatePredicates`'s fields hold (see that struct and
    // `evaluate_chrome_gate`, the single source of truth both
    // `decide_chrome_mode` and this instrumentation call). Every counter
    // below is incremented independently -- a frame that fails more than one
    // predicate increments more than one counter -- so overlap between
    // blockers is visible rather than only ever seeing "the first" reason.
    /// Frames where `ChromeGatePredicates::cache_matches` was `false`: no
    /// chrome cache exists yet, or this frame's framebuffer size/`ppp`
    /// didn't match what the cache was tessellated at.
    gate_blocked_cache_mismatch: u64,
    /// Frames where `ChromeGatePredicates::repaint_settled` was `false`:
    /// [`chrome_repaint_settled`] found something wanted a wake sooner than
    /// this frame's own request permits. See the `settle_*` fields below
    /// for the VALUES behind this failure, not just the count.
    gate_blocked_not_settled: u64,
    /// Frames where `ChromeGatePredicates::damage_unchanged` was `false`:
    /// the PREVIOUS frame's `ChromeDamage` was `Changed`, so this frame
    /// cannot REPLAY regardless of anything else.
    gate_blocked_damage_changed: u64,
    /// Frames where `ChromeGatePredicates::no_chrome_input` was `false`:
    /// `chrome_input_this_frame` was `true` (a window input event this
    /// frame could plausibly have affected chrome).
    gate_blocked_chrome_input: u64,

    // ── Settle-gate value diagnostics (feature-gated), reset every flush
    // window (`reset_settle_window`) rather than cumulative-since-creation
    // like every other field above. A lifetime min/max would not be useful
    // here: the lifetime min collapses to whatever the very first settle
    // failure happened to be (often frame 0-ish transients) and the
    // lifetime max trivially saturates at `Duration::MAX` the first time
    // egui or the app ever reports "no repaint needed" on a losing frame --
    // neither tells you what is happening in THIS flush window, which is
    // the question ("did the app ask for 500ms and egui want 500ms too, or
    // did something ask for 16ms behind our back").
    /// Smallest `prev_repaint_delay` observed on a frame where the settle
    /// predicate failed, this flush window. `None` until the first such
    /// frame in this window.
    settle_repaint_delay_min: Option<std::time::Duration>,
    /// Largest `prev_repaint_delay` observed on a frame where the settle
    /// predicate failed, this flush window.
    settle_repaint_delay_max: Option<std::time::Duration>,
    /// Smallest `prev_terminal_requested_delay` (when `Some`) observed on a
    /// frame where the settle predicate failed, this flush window.
    settle_terminal_requested_delay_min: Option<std::time::Duration>,
    /// Largest `prev_terminal_requested_delay` (when `Some`) observed on a
    /// frame where the settle predicate failed, this flush window.
    settle_terminal_requested_delay_max: Option<std::time::Duration>,
    /// Count of settle-predicate failures this flush window where
    /// `prev_terminal_requested_delay` was `None` (nothing app-side had
    /// requested a delay at all this frame, counted separately from "the
    /// app requested something, but egui wanted sooner").
    settle_terminal_requested_delay_none_count: u64,
}

#[cfg(feature = "frame-profiling")]
impl FrameProfile {
    /// Emit a `debug` summary once every this many drawn frames -- same
    /// cadence as `freminal::gui::window::FrameStats::FLUSH_EVERY` so the
    /// two crates' log lines are easy to correlate by eye.
    const FLUSH_EVERY: u64 = 120;

    /// Percentage of `(chrome_mode_full + chrome_mode_replay)` frames that
    /// were `Replay`. Pure so it's unit-testable in isolation. `0.0` when no
    /// frames have been counted yet (rather than dividing by zero).
    ///
    /// Deliberately duplicated in `freminal::gui::window::FrameStats` rather
    /// than shared (reviewed and accepted) -- keep the two in sync by eye if
    /// either changes.
    fn chrome_replay_duty_cycle_pct(full: u64, replay: u64) -> f64 {
        let total = full.saturating_add(replay);
        if total == 0 {
            return 0.0;
        }
        // `u64 -> f64` is lossy for very large counts (beyond 2^53), but a
        // live session's frame counters never approach that range;
        // `approx_as` is the established lossy-conversion idiom in this
        // file (see `scale_factor().approx_as::<f32>()` in `new()` above).
        let replay_f: f64 = conv2::ConvUtil::approx_as(replay).unwrap_or(0.0);
        let total_f: f64 = conv2::ConvUtil::approx_as(total).unwrap_or(1.0);
        (replay_f / total_f) * 100.0
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

    /// Record one settle-gate failure's values into this flush window's
    /// min/max/none-count tracking. Called only when
    /// `ChromeGatePredicates::repaint_settled` is `false` for the frame
    /// (see the call site in `run_frame`) -- these fields answer "what were
    /// the actual delays on the frames that failed", not just "how many
    /// failed".
    fn record_settle_failure(
        &mut self,
        repaint_delay: std::time::Duration,
        terminal_requested_delay: Option<std::time::Duration>,
    ) {
        let (min, max) = min_max_duration(
            (self.settle_repaint_delay_min, self.settle_repaint_delay_max),
            repaint_delay,
        );
        self.settle_repaint_delay_min = min;
        self.settle_repaint_delay_max = max;

        match terminal_requested_delay {
            Some(d) => {
                let (min, max) = min_max_duration(
                    (
                        self.settle_terminal_requested_delay_min,
                        self.settle_terminal_requested_delay_max,
                    ),
                    d,
                );
                self.settle_terminal_requested_delay_min = min;
                self.settle_terminal_requested_delay_max = max;
            }
            None => {
                self.settle_terminal_requested_delay_none_count = self
                    .settle_terminal_requested_delay_none_count
                    .saturating_add(1);
            }
        }
    }

    /// Clear the settle-gate value diagnostics at the end of a flush window
    /// -- see the field docs on `settle_repaint_delay_min` etc. for why
    /// these are windowed rather than cumulative-since-creation.
    const fn reset_settle_window(&mut self) {
        self.settle_repaint_delay_min = None;
        self.settle_repaint_delay_max = None;
        self.settle_terminal_requested_delay_min = None;
        self.settle_terminal_requested_delay_max = None;
        self.settle_terminal_requested_delay_none_count = 0;
    }

    /// Format an optional `Duration` for a `tracing` field: `"none"` when
    /// no sample was recorded this window, the literal string `"MAX"` when
    /// the sample IS `Duration::MAX` (egui's "no repaint needed" sentinel --
    /// printing it as a microsecond count would show an absurd
    /// ~584,942,417,355-year figure), or the microsecond count otherwise.
    /// Pure, so directly unit-testable.
    fn format_duration_field(d: Option<std::time::Duration>) -> String {
        match d {
            None => "none".to_string(),
            Some(d) if d == std::time::Duration::MAX => "MAX".to_string(),
            Some(d) => format!("{}us", d.as_micros()),
        }
    }
}

/// Update a running `(min, max)` pair with one new `Duration` sample.
/// `current` starts `(None, None)` before the first sample. Pure, so
/// directly unit-testable; shared by [`FrameProfile::record_settle_failure`]
/// for both the `prev_repaint_delay` and `prev_terminal_requested_delay`
/// tracking (avoiding writing the same min/max-or-first-sample logic twice).
#[cfg(feature = "frame-profiling")]
fn min_max_duration(
    current: (Option<std::time::Duration>, Option<std::time::Duration>),
    sample: std::time::Duration,
) -> (Option<std::time::Duration>, Option<std::time::Duration>) {
    let (min, max) = current;
    (
        Some(min.map_or(sample, |m| m.min(sample))),
        Some(max.map_or(sample, |m| m.max(sample))),
    )
}

/// #436 §3.1 (amended): a REPLAY is permitted only if nothing OTHER than
/// freminal's own blink/content repaint scheduling asked egui for a wake this
/// frame. `repaint_delay` is egui's per-viewport requested delay (`Duration::MAX`
/// = egui itself wants no further repaint); `terminal_requested_delay` is what
/// the app itself passed to `ctx.request_repaint_after` this frame (its blink /
/// content / shader-anim scheduling). If egui's delay is SHORTER than what the
/// app requested, something egui-internal (a hover-fade, menu animation, a
/// cursor blink in a focused `TextEdit`, etc.) also wants a wake -> chrome is
/// not settled -> no replay. A literal `== Duration::MAX` gate would never
/// pass while a cursor blinks (the app calls `request_repaint_after(500ms)`
/// every frame), defeating the headline blink-under-idle case; comparing
/// against the app's own request is exact, not a heuristic.
fn chrome_repaint_settled(
    repaint_delay: std::time::Duration,
    terminal_requested_delay: Option<std::time::Duration>,
) -> bool {
    terminal_requested_delay.map_or_else(
        || repaint_delay == std::time::Duration::MAX,
        |app_delay| repaint_delay >= app_delay,
    )
}

/// #436.4c §5.2: did the font atlas (`TextureId::default()`) grow this
/// frame?
///
/// `egui`'s `TextureAtlas::allocate` grows the shared font atlas by
/// doubling its height when a not-yet-shaped glyph needs room; glyph UVs
/// are normalized to `[0, 1]` at tessellation time by dividing by the
/// atlas size *current at that call* and baked into the mesh — so a mesh
/// tessellated before a resize samples the wrong rows if painted after
/// one. `ImageDelta::is_whole()` (`pos.is_none()`) distinguishes a
/// whole-texture upload (initial allocation or a resize/respecify) from
/// an incremental patch (`pos: Some(_)`, a normal new-glyph-added-to-the-
/// existing-atlas upload that does NOT invalidate previously baked UVs).
/// Only `TextureId::default()` — the font atlas — is relevant; a whole
/// upload of some other (e.g. image-protocol) texture does not affect
/// chrome glyph UVs.
///
/// Pure (no `self`/`egui::Context` state needed beyond the delta itself),
/// so directly unit-testable.
fn atlas_grew(textures_delta: &egui::TexturesDelta) -> bool {
    textures_delta
        .set
        .iter()
        .any(|(id, delta)| *id == egui::TextureId::default() && delta.is_whole())
}

/// The four independent REPLAY gate predicates from [`decide_chrome_mode`]'s
/// doc, bundled so `decide_chrome_mode` and the (feature-gated) gate-blocker
/// instrumentation in `run_frame` evaluate them via the SAME function
/// ([`evaluate_chrome_gate`]) rather than two hand-maintained copies of the
/// same boolean logic drifting apart. See `evaluate_chrome_gate`'s doc for
/// what each field means.
// struct_excessive_bools: these are four independent, unrelated gate
// observations (mirrors the existing allow on `ChromeSignals`/
// `DismissiblePresence` in `freminal::gui::chrome_damage`) -- not a state
// machine, and each field is consumed both individually (the gate-blocker
// counters) and via `all_pass` (the actual decision), so collapsing them
// into an enum would make the individual-predicate consumer harder, not
// easier.
#[allow(clippy::struct_excessive_bools)]
struct ChromeGatePredicates {
    cache_matches: bool,
    repaint_settled: bool,
    damage_unchanged: bool,
    no_chrome_input: bool,
}

impl ChromeGatePredicates {
    /// A REPLAY is permitted only when every predicate holds.
    const fn all_pass(&self) -> bool {
        self.cache_matches && self.repaint_settled && self.damage_unchanged && self.no_chrome_input
    }
}

/// #436.4b: evaluate the four independent REPLAY gate predicates. This is
/// the single source of truth [`decide_chrome_mode`] consumes to make its
/// decision, and (feature-gated only) the frame-profiling gate-blocker
/// counters in `run_frame` call this SAME function a second time with the
/// SAME arguments to see WHICH predicate(s) failed -- see the `run_frame`
/// call site. If this function's logic ever changes, both consumers change
/// with it; there is no second copy of the boolean expressions to forget.
///
///   - `cache_matches` — a chrome cache exists from a prior FULL frame
///     (`cache_valid`), AND `cache_size`/`cache_ppp` match
///     `cur_size`/`cur_ppp` — the cached primitives were tessellated at
///     this frame's framebuffer size and scale factor (a mismatch, e.g. a
///     resize or DPI change, invalidates the whole cache rather than
///     attempting a partial re-tessellation).
///   - `repaint_settled` — [`chrome_repaint_settled`]: nothing egui-internal
///     wants a wake sooner than the app's own scheduling this frame (§3.1
///     amendment).
///   - `damage_unchanged` — `prev_chrome_damage` is
///     [`crate::ChromeDamage::Unchanged`] — the PREVIOUS frame proved
///     static chrome did not change (§3.3/§3.5).
///   - `no_chrome_input` — `!chrome_input_this_frame`: no window input
///     event this frame could plausibly have affected chrome (§3.2,
///     conservative: any qualifying input forces `Full`, refined
///     region-aware in #436.8).
///
/// Pure (no `self`/`egui` state), so directly unit-testable.
// Nine independent, unrelated gate inputs (cache validity/size/ppp, this
// frame's size/ppp, the two settle-rule delays, prior chrome damage, and the
// input gate) -- bundling them into a struct would just relocate the same
// fields without adding clarity, and this function exists specifically so
// tests can drive each one independently.
#[allow(clippy::too_many_arguments)]
fn evaluate_chrome_gate(
    cache_valid: bool,
    cache_size: [u32; 2],
    cache_ppp: f32,
    cur_size: [u32; 2],
    cur_ppp: f32,
    prev_repaint_delay: std::time::Duration,
    prev_terminal_requested_delay: Option<std::time::Duration>,
    prev_chrome_damage: crate::ChromeDamage,
    chrome_input_this_frame: bool,
) -> ChromeGatePredicates {
    ChromeGatePredicates {
        cache_matches: cache_valid
            && cache_size == cur_size
            && (cache_ppp - cur_ppp).abs() < f32::EPSILON,
        repaint_settled: chrome_repaint_settled(prev_repaint_delay, prev_terminal_requested_delay),
        damage_unchanged: prev_chrome_damage == crate::ChromeDamage::Unchanged,
        no_chrome_input: !chrome_input_this_frame,
    }
}

/// #436.4b: decide this frame's [`crate::ChromeMode`].
///
/// A REPLAY is permitted only when ALL of [`evaluate_chrome_gate`]'s four
/// predicates hold. Any failure forces `ChromeMode::Full` — the
/// always-correct, conservative default. Pure (no `self`/`egui` state), so
/// directly unit-testable.
#[allow(clippy::too_many_arguments)]
fn decide_chrome_mode(
    cache_valid: bool,
    cache_size: [u32; 2],
    cache_ppp: f32,
    cur_size: [u32; 2],
    cur_ppp: f32,
    prev_repaint_delay: std::time::Duration,
    prev_terminal_requested_delay: Option<std::time::Duration>,
    prev_chrome_damage: crate::ChromeDamage,
    chrome_input_this_frame: bool,
) -> crate::ChromeMode {
    let gate = evaluate_chrome_gate(
        cache_valid,
        cache_size,
        cache_ppp,
        cur_size,
        cur_ppp,
        prev_repaint_delay,
        prev_terminal_requested_delay,
        prev_chrome_damage,
        chrome_input_this_frame,
    );

    if gate.all_pass() {
        crate::ChromeMode::Replay
    } else {
        crate::ChromeMode::Full
    }
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
            chrome_cache: None,
            prev_repaint_delay: std::time::Duration::MAX,
            prev_chrome_damage: crate::ChromeDamage::Changed,
            prev_terminal_requested_delay: None,
            #[cfg(feature = "frame-profiling")]
            frame_profile: FrameProfile::default(),
        })
    }

    /// Collect raw input from winit for the current frame.
    pub(crate) fn take_egui_input(&mut self, window: &Window) -> egui::RawInput {
        self.winit_state.take_egui_input(window)
    }

    /// Run a single egui frame and paint, using pre-collected raw input.
    ///
    /// `chrome_input_this_frame` is the #436.4b §3.2 conservative input gate:
    /// `true` if ANY window input event this frame could plausibly affect
    /// chrome (pointer/keyboard/scroll/focus/IME — see `event_loop`'s
    /// `is_potential_chrome_input`). `true` unconditionally forces
    /// `ChromeMode::Full` this frame; the region-aware refinement (only
    /// input actually over chrome, not the terminal band, forces `Full`) is
    /// deferred to #436.8.
    ///
    /// Returns [`FrameOutput`] containing viewport commands and repaint timing.
    // too_many_arguments: `chrome_input_this_frame` (#436.4b) is the one
    // parameter pushing this over the threshold; the others are pre-existing
    // (window/gl/clear_color/raw_input/present_flag/ui_fn), each independent
    // and already load-bearing -- bundling them into a struct would only
    // relocate the count, not reduce it.
    // too_many_lines: this is the single frame-lifecycle function (decide
    // chrome_mode -> run_ui -> tessellate head/band/tail -> present -> stash
    // next frame's signals); splitting it would scatter a single atomic
    // sequence across artificial sub-functions without reducing coupling
    // (mirrors the existing `too_many_lines` allows on `event_loop.rs`'s
    // `window_event`/`run` and `app_impl.rs`'s `update`).
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) fn run_frame<F>(
        &mut self,
        window: &Window,
        gl_state: &GlState,
        clear_color: [f32; 4],
        raw_input: egui::RawInput,
        present_flag: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
        chrome_input_this_frame: bool,
        ui_fn: F,
    ) -> FrameOutput
    where
        F: FnMut(&egui::Context, &glow::Context, crate::ChromeMode) -> crate::FrameSignals,
    {
        let mut ui_fn = ui_fn;

        // Task 121 frame-profiling harness (defect-4 fix): wall-clock start
        // of the WHOLE frame, captured before anything else in `run_frame`
        // runs. Its `.elapsed()` (taken just before this window's flush
        // check, near the end of this function) is `phase_total` -- see the
        // `FrameProfile::phase_total_total` doc for what its residual
        // against `run_ui + tessellate + paint + swap` exposes.
        #[cfg(feature = "frame-profiling")]
        let total_start = std::time::Instant::now();

        // ── #436.4b: decide this frame's `ChromeMode` BEFORE `run_ui` ──────
        //
        // The decision must be made before `run_ui` because `chrome_mode` is
        // passed INTO the app's `update()` call (inside the `run_ui`
        // closure below), which is what lets the app skip chrome-widget
        // construction on an eligible frame. It is therefore based on LAST
        // frame's signals (`prev_repaint_delay`, `prev_terminal_requested_delay`,
        // `prev_chrome_damage`) plus THIS frame's cache-validity inputs
        // (`size`, `pixels_per_point`) and input gate — never on anything
        // only knowable from this frame's own `update()` call (that data
        // does not exist yet).
        //
        // `pixels_per_point()` is read here BEFORE `run_ui`, which returns
        // whatever was in effect as of the END of the previous frame's
        // `begin_pass` (egui only updates its stored `pixels_per_point` file
        // during `begin_pass`, which is inside `run_ui`) — i.e. exactly the
        // value the chrome cache was tessellated at, UNLESS the incoming
        // `raw_input` we are about to feed into `run_ui` carries a changed
        // scale factor for THIS frame. In that rare case this pre-`run_ui`
        // read is stale by one frame: `decide_chrome_mode` would (wrongly)
        // see `cur_ppp == cache.ppp` and permit `Replay`, while the
        // definitive post-`run_ui` `pixels_per_point()` used below to
        // tessellate the band would reflect the NEW value. The visible
        // consequence is bounded to a single frame — cached chrome painted
        // at the old scale while the freshly-tessellated band paints at the
        // new one — because this frame's own `ChromeDamage` (driven by the
        // app's `ppp_changed` signal, sourced from the same post-`run_ui`
        // value) reports `Changed`, forcing the NEXT frame back to `Full`.
        // `window.inner_size()` has no such caveat: it is a direct winit
        // query, always current.
        let size = window.inner_size();
        let size_arr = [size.width, size.height];
        let ppp_before_run_ui = self.ctx.pixels_per_point();
        let (cache_valid, cache_size, cache_ppp) = self
            .chrome_cache
            .as_ref()
            .map_or((false, [0, 0], 0.0), |cache| (true, cache.size, cache.ppp));
        let chrome_mode = decide_chrome_mode(
            cache_valid,
            cache_size,
            cache_ppp,
            size_arr,
            ppp_before_run_ui,
            self.prev_repaint_delay,
            self.prev_terminal_requested_delay,
            self.prev_chrome_damage,
            chrome_input_this_frame,
        );

        // Task 121 frame-profiling harness: count which `ChromeMode` was
        // just decided. Cross-checkable against `freminal`'s own
        // `chrome_mode_full`/`chrome_mode_replay` counters, which are
        // sourced from this exact same `chrome_mode` value (passed into
        // `App::update` inside `ui_fn` below) -- if the two ever disagree,
        // that disagreement is itself a finding.
        #[cfg(feature = "frame-profiling")]
        match chrome_mode {
            crate::ChromeMode::Full => {
                self.frame_profile.chrome_mode_full =
                    self.frame_profile.chrome_mode_full.saturating_add(1);
            }
            crate::ChromeMode::Replay => {
                self.frame_profile.chrome_mode_replay =
                    self.frame_profile.chrome_mode_replay.saturating_add(1);
            }
        }

        // Gate-blocker instrumentation (feature-gated only): re-evaluate the
        // SAME four predicates `decide_chrome_mode` just consumed, via the
        // SAME `evaluate_chrome_gate` function and the SAME arguments, so
        // this can never drift from what actually gated the decision above
        // (see `evaluate_chrome_gate`'s doc). Every failing predicate
        // increments its own counter -- not just the first -- so overlap
        // between blockers (e.g. cache mismatch AND not-settled on the same
        // frame) is visible rather than only ever attributing to one cause.
        #[cfg(feature = "frame-profiling")]
        {
            let gate = evaluate_chrome_gate(
                cache_valid,
                cache_size,
                cache_ppp,
                size_arr,
                ppp_before_run_ui,
                self.prev_repaint_delay,
                self.prev_terminal_requested_delay,
                self.prev_chrome_damage,
                chrome_input_this_frame,
            );
            if !gate.cache_matches {
                self.frame_profile.gate_blocked_cache_mismatch = self
                    .frame_profile
                    .gate_blocked_cache_mismatch
                    .saturating_add(1);
            }
            if !gate.repaint_settled {
                self.frame_profile.gate_blocked_not_settled = self
                    .frame_profile
                    .gate_blocked_not_settled
                    .saturating_add(1);
                // Settle-gate value diagnostics: only recorded when this
                // predicate specifically failed -- see `record_settle_failure`.
                self.frame_profile.record_settle_failure(
                    self.prev_repaint_delay,
                    self.prev_terminal_requested_delay,
                );
            }
            if !gate.damage_unchanged {
                self.frame_profile.gate_blocked_damage_changed = self
                    .frame_profile
                    .gate_blocked_damage_changed
                    .saturating_add(1);
            }
            if !gate.no_chrome_input {
                self.frame_profile.gate_blocked_chrome_input = self
                    .frame_profile
                    .gate_blocked_chrome_input
                    .saturating_add(1);
            }
        }

        // egui 0.35 replaced `Context::run` (closure took `&Context`) with
        // `Context::run_ui` (closure takes the root `&mut Ui`).  Our `App`
        // trait still works in terms of `&Context`; `Ui` derefs to `Context`,
        // so deref explicitly rather than relying on a silent coercion.
        //
        // The closure both runs the app's `update` and returns this frame's
        // signals (damage report, terminal-band range, chrome-damage
        // decision, and the app's own requested repaint delay); we capture
        // them here to decide the clear/present path, the head/band/tail
        // split below, and next frame's `chrome_mode` decision. Running both
        // inside the one closure avoids two simultaneous `&mut app` borrows
        // in the caller.
        let mut frame_damage = crate::FrameDamage::Full;
        let mut band_range: std::ops::Range<usize> = 0..0;
        let mut chrome_damage = crate::ChromeDamage::Changed;
        let mut terminal_requested_delay: Option<std::time::Duration> = None;
        // Task 121 frame-profiling harness: `run_ui` itself calls into
        // `ui_fn` (and therefore `App::update`), so this timing is an upper
        // bound on freminal's own per-frame `update()` cost as observed from
        // the windowing side.
        #[cfg(feature = "frame-profiling")]
        let run_ui_start = std::time::Instant::now();
        let full_output = self.ctx.run_ui(raw_input, |root_ui| {
            let signals = ui_fn(&*root_ui, &gl_state.glow_context, chrome_mode);
            frame_damage = signals.frame_damage;
            band_range = signals.band_range;
            chrome_damage = signals.chrome_damage;
            terminal_requested_delay = signals.terminal_requested_delay;
        });
        #[cfg(feature = "frame-profiling")]
        {
            let d = run_ui_start.elapsed();
            self.frame_profile.run_ui_total += d;
            self.frame_profile.run_ui_max = self.frame_profile.run_ui_max.max(d);
        }

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        // Definitive `pixels_per_point` for THIS frame — read AFTER `run_ui`
        // has processed `raw_input` via `begin_pass`, so (unlike
        // `ppp_before_run_ui` above) this always reflects a scale-factor
        // change delivered this frame. Used for all tessellation below.
        let pixels_per_point = self.ctx.pixels_per_point();

        // #436.4c §5.2: did the font atlas grow (or otherwise get wholly
        // re-specified) THIS frame? Glyph shaping happens during `run_ui`
        // (layout/galley creation), so `full_output.textures_delta` already
        // reflects any atlas growth caused by this frame's chrome OR
        // terminal-band content — read this once, up front, before the
        // head/band/tail split below, since it drives the REPLAY self-heal
        // path and is irrelevant (see below) on the FULL path.
        let atlas_grew_this_frame = atlas_grew(&full_output.textures_delta);

        // ── 3-way paint-order split (#436.4a) ──────────────────────────
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
        // phase as one span (band tessellation below, plus whatever
        // head/tail tessellation the `match chrome_mode` block further down
        // performs) rather than splitting band vs. head/tail into separate
        // counters. The two are not cleanly separable into independent
        // costs worth reporting apart: the band tessellate always runs, but
        // head/tail tessellation is conditional (FULL frames, and REPLAY
        // frames that hit the atlas-grew self-heal path skip it entirely
        // otherwise), so a fixed split would mean "band" sometimes includes
        // the whole phase and sometimes a third of it depending on
        // `chrome_mode` -- one total, sampled per-frame, is the more
        // meaningful number to threshold/alert on.
        #[cfg(feature = "frame-profiling")]
        let tessellate_start = std::time::Instant::now();

        // The band is ALWAYS fresh — tessellated from this frame's own
        // shapes regardless of `chrome_mode` — since the terminal band is
        // rebuilt every frame whether or not chrome was.
        let band_primitives = self.ctx.tessellate(band_shapes, pixels_per_point);

        // Head/tail: FULL re-tessellates from this frame's shapes (and
        // repopulates the cache for a future REPLAY); REPLAY reuses the
        // cached primitives from the last FULL frame instead. On a REPLAY
        // frame the app skipped chrome-widget construction, so
        // `shapes[..start]`/`shapes[end..]` are expected to be empty (or, for
        // an app that has not wired up the band range, `shapes[..start]` is
        // trivially empty per the `0..0` default) — but even if the app
        // painted something there, a REPLAY frame must ignore it: the whole
        // point of replay is that head/tail are NOT re-tessellated, so
        // painting anything from `shapes[..start]`/`shapes[end..]` here
        // would silently diverge from what the cache (and therefore this
        // frame's actual pixels) represents.
        // On a FULL frame `atlas_grew_this_frame` is irrelevant: every band
        // is re-tessellated fresh from this frame's own shapes regardless,
        // so there is no stale-UV hazard to self-heal — the self-heal below
        // is REPLAY-only.
        let (head_primitives, tail_primitives) = match chrome_mode {
            crate::ChromeMode::Full => {
                let head_shapes: Vec<egui::epaint::ClippedShape> = shapes[..start].to_vec();
                let tail_shapes: Vec<egui::epaint::ClippedShape> = shapes[end..].to_vec();
                let head_primitives = self.ctx.tessellate(head_shapes.clone(), pixels_per_point);
                let tail_primitives = self.ctx.tessellate(tail_shapes.clone(), pixels_per_point);
                self.chrome_cache = Some(ChromeCache {
                    head_shapes,
                    tail_shapes,
                    head_primitives: head_primitives.clone(),
                    tail_primitives: tail_primitives.clone(),
                    ppp: pixels_per_point,
                    size: size_arr,
                });
                (head_primitives, tail_primitives)
            }
            crate::ChromeMode::Replay if atlas_grew_this_frame => {
                // #436.4c §5.2 self-heal: the font atlas grew this frame (a
                // new glyph shaped by the band or an overlay). The cached
                // chrome primitives' UVs were baked against the SMALLER
                // atlas that existed when they were last tessellated;
                // painting them now would sample the wrong texture rows
                // (garbled chrome text). Re-tessellate the cached SHAPES
                // (not `update()`, which is not idempotent and must never
                // be re-run mid-frame) against the current atlas and
                // refresh the cached primitives so the NEXT replay reuses
                // correct UVs too. One rare extra tessellate, zero visible
                // glitch.
                //
                // Clone the shapes out of an immutable borrow first (and
                // let that borrow end) before tessellating via `self.ctx`
                // and then re-borrowing `self.chrome_cache` mutably to
                // store the refreshed primitives — holding the cache borrow
                // across the `self.ctx.tessellate` calls would conflict
                // with `&self.ctx` while `self.chrome_cache` is exclusively
                // borrowed.
                let cached_shapes = self
                    .chrome_cache
                    .as_ref()
                    .map(|cache| (cache.head_shapes.clone(), cache.tail_shapes.clone()));
                cached_shapes.map_or_else(
                    || {
                        // Defensive fallback: `decide_chrome_mode` proved
                        // `cache_valid` before choosing `Replay`, so this
                        // should be unreachable — but degrade gracefully
                        // (empty chrome this frame) rather than panic.
                        (Vec::new(), Vec::new())
                    },
                    |(head_shapes, tail_shapes)| {
                        let head_primitives = self.ctx.tessellate(head_shapes, pixels_per_point);
                        let tail_primitives = self.ctx.tessellate(tail_shapes, pixels_per_point);
                        if let Some(cache) = self.chrome_cache.as_mut() {
                            cache.head_primitives.clone_from(&head_primitives);
                            cache.tail_primitives.clone_from(&tail_primitives);
                        }
                        (head_primitives, tail_primitives)
                    },
                )
            }
            crate::ChromeMode::Replay => {
                // Normal replay: no atlas growth this frame, cached
                // primitives' UVs remain valid — reuse them directly.
                self.chrome_cache.as_ref().map_or_else(
                    || {
                        // Defensive fallback: `decide_chrome_mode` proved
                        // `cache_valid` (`self.chrome_cache.is_some()`) before
                        // choosing `Replay`, so this should be unreachable —
                        // but degrade gracefully (empty chrome this frame)
                        // rather than panic if it ever is.
                        (Vec::new(), Vec::new())
                    },
                    |cache| (cache.head_primitives.clone(), cache.tail_primitives.clone()),
                )
            }
        };
        #[cfg(feature = "frame-profiling")]
        {
            let d = tessellate_start.elapsed();
            self.frame_profile.tessellate_total += d;
            self.frame_profile.tessellate_max = self.frame_profile.tessellate_max.max(d);
        }

        // Decide whether this frame may skip the full clear and present only
        // its damaged region. This is a two-part gate:
        //   1. The app reports the frame as `Partial` (only the listed rects
        //      changed; everything else is identical to the previous frame).
        //   2. The back buffer still holds the previous frame's contents
        //      (`buffer_age() == 1`), and the surface can present a sub-region.
        // If either fails we fall back to the always-correct full path:
        // clear + full paint + full swap.
        let partial = match frame_damage {
            crate::FrameDamage::Partial(rects)
                if !rects.is_empty()
                    && gl_state.supports_partial_present()
                    && gl_state.buffer_age() == 1 =>
            {
                Some(rects)
            }
            _ => None,
        };

        if partial.is_none() {
            gl_state.clear(clear_color);
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
        let size_px = [size.width, size.height];
        for (id, image_delta) in &full_output.textures_delta.set {
            self.painter.set_texture(*id, image_delta);
        }
        // Task 121 frame-profiling harness: time the three `paint_primitives`
        // calls, summed (a contiguous span across all three, since nothing
        // else runs between them, is equal to summing each individually).
        #[cfg(feature = "frame-profiling")]
        let paint_start = std::time::Instant::now();
        self.painter
            .paint_primitives(size_px, pixels_per_point, &head_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &band_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &tail_primitives);
        #[cfg(feature = "frame-profiling")]
        {
            let d = paint_start.elapsed();
            self.frame_profile.paint_total += d;
            self.frame_profile.paint_max = self.frame_profile.paint_max.max(d);
        }
        for id in &full_output.textures_delta.free {
            self.painter.free_texture(*id);
        }

        // Pre-present notify for Wayland frame pacing
        window.pre_present_notify();

        #[cfg(feature = "frame-profiling")]
        let swap_start = std::time::Instant::now();
        let swap_result = partial.as_ref().map_or_else(
            || gl_state.swap_buffers(),
            |rects| gl_state.swap_buffers_with_damage(rects),
        );
        #[cfg(feature = "frame-profiling")]
        {
            let d = swap_start.elapsed();
            self.frame_profile.swap_total += d;
            self.frame_profile.swap_max = self.frame_profile.swap_max.max(d);
        }
        if let Err(e) = swap_result {
            tracing::error!("swap_buffers failed: {e}");
        }

        let viewport_output = full_output.viewport_output.get(&egui::ViewportId::ROOT);

        let repaint_delay = viewport_output.map_or(std::time::Duration::MAX, |vo| vo.repaint_delay);

        let commands = viewport_output
            .map(|vo| vo.commands.clone())
            .unwrap_or_default();

        // Do NOT call `window.request_redraw()` here — let the event loop
        // manage scheduling via `repaint_at` / `about_to_wait`.  Calling
        // `request_redraw()` directly bypasses `ControlFlow::WaitUntil` and
        // causes an unbounded render loop on platforms where `swap_buffers`
        // returns immediately (macOS with vsync disabled).

        // Stash this frame's signals for next frame's `chrome_mode` decision.
        self.prev_repaint_delay = repaint_delay;
        self.prev_chrome_damage = chrome_damage;
        self.prev_terminal_requested_delay = terminal_requested_delay;

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
            // Defect 4: `phase_total` covers the WHOLE of `run_frame`,
            // taken as late as possible (just before this flush check) so it
            // includes every gap named on `FrameProfile::phase_total_total`'s
            // doc: `handle_platform_output`, the band-shape clone,
            // `atlas_grew`, the GL clear, and the texture set/free loops.
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
                    chrome_mode_full = p.chrome_mode_full,
                    chrome_mode_replay = p.chrome_mode_replay,
                    chrome_replay_duty_cycle_pct = FrameProfile::chrome_replay_duty_cycle_pct(
                        p.chrome_mode_full,
                        p.chrome_mode_replay
                    ),
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
                    // Gate-blocker counters: which of the four REPLAY gate
                    // predicates (see `evaluate_chrome_gate`) blocked a Full
                    // decision this window, cumulative since window
                    // creation. More than one may be nonzero for the same
                    // set of frames -- that overlap is itself a finding.
                    gate_blocked_cache_mismatch = p.gate_blocked_cache_mismatch,
                    gate_blocked_not_settled = p.gate_blocked_not_settled,
                    gate_blocked_damage_changed = p.gate_blocked_damage_changed,
                    gate_blocked_chrome_input = p.gate_blocked_chrome_input,
                    // Settle-gate value diagnostics: only populated on
                    // frames where `gate_blocked_not_settled` fired this
                    // flush window (see `record_settle_failure`); "none"
                    // means the settle gate never failed this window, "MAX"
                    // means a failing frame's `prev_repaint_delay` was
                    // itself `Duration::MAX` (egui wanted no repaint yet the
                    // gate still failed -- only possible via the
                    // `prev_terminal_requested_delay` arm, since `MAX >=
                    // app_delay` is only false when `app_delay` is also
                    // `MAX`, which cannot itself fail the gate -- so seeing
                    // this would itself be a notable finding).
                    settle_repaint_delay_min_us =
                        %FrameProfile::format_duration_field(p.settle_repaint_delay_min),
                    settle_repaint_delay_max_us =
                        %FrameProfile::format_duration_field(p.settle_repaint_delay_max),
                    settle_terminal_requested_delay_min_us = %FrameProfile::format_duration_field(
                        p.settle_terminal_requested_delay_min
                    ),
                    settle_terminal_requested_delay_max_us = %FrameProfile::format_duration_field(
                        p.settle_terminal_requested_delay_max
                    ),
                    settle_terminal_requested_delay_none_count =
                        p.settle_terminal_requested_delay_none_count,
                    "windowing frame-profiling stats (task 121 harness): ChromeMode \
                     duty cycle, the windowing-owned phase_total/run_ui/tessellate/\
                     paint/swap wall-clock split over frame_counter drawn frames for \
                     this window_id (phase_total minus (run_ui + tessellate + paint + \
                     swap) is the unmeasured residual -- see \
                     FrameProfile::phase_total_total), which of the four REPLAY gate \
                     predicates blocked Full->Replay this window (gate_blocked_*, \
                     cumulative, non-exclusive), and -- when the settle gate \
                     specifically failed -- the min/max prev_repaint_delay and \
                     prev_terminal_requested_delay values behind that failure this \
                     flush window (settle_*)"
                );

                // Settle-gate value diagnostics are windowed, not
                // cumulative-since-creation (see the field docs) -- clear
                // them now that this window's line has been logged. The
                // `gate_blocked_*` counters above are NOT reset; they stay
                // cumulative like every other `FrameProfile` counter.
                self.frame_profile.reset_settle_window();
            }
        }

        FrameOutput {
            commands,
            repaint_delay,
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
    pub(crate) fn modifiers(&self) -> egui::Modifiers {
        self.winit_state.egui_input().modifiers
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
        assert!((FrameProfile::chrome_replay_duty_cycle_pct(0, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_zero_when_replay_never_engages() {
        assert!((FrameProfile::chrome_replay_duty_cycle_pct(120, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_100_when_always_replay() {
        let pct = FrameProfile::chrome_replay_duty_cycle_pct(0, 120);
        assert!((pct - 100.0).abs() < 0.001, "pct was {pct}");
    }

    #[test]
    fn duty_cycle_is_25_for_3_full_1_replay() {
        let pct = FrameProfile::chrome_replay_duty_cycle_pct(90, 30);
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

    // ── `min_max_duration` ──────────────────────────────────────────────

    #[test]
    fn min_max_duration_first_sample_sets_both() {
        let (min, max) = super::min_max_duration((None, None), Duration::from_millis(16));
        assert_eq!(min, Some(Duration::from_millis(16)));
        assert_eq!(max, Some(Duration::from_millis(16)));
    }

    #[test]
    fn min_max_duration_tracks_a_smaller_second_sample() {
        let first = super::min_max_duration((None, None), Duration::from_millis(16));
        let second = super::min_max_duration(first, Duration::from_millis(4));
        assert_eq!(second.0, Some(Duration::from_millis(4)));
        assert_eq!(second.1, Some(Duration::from_millis(16)));
    }

    #[test]
    fn min_max_duration_tracks_a_larger_second_sample() {
        let first = super::min_max_duration((None, None), Duration::from_millis(16));
        let second = super::min_max_duration(first, Duration::from_millis(500));
        assert_eq!(second.0, Some(Duration::from_millis(16)));
        assert_eq!(second.1, Some(Duration::from_millis(500)));
    }

    // ── `FrameProfile::format_duration_field` ────────────────────────────

    #[test]
    fn format_duration_field_none_is_literal_none() {
        assert_eq!(FrameProfile::format_duration_field(None), "none");
    }

    #[test]
    fn format_duration_field_max_is_literal_max_not_an_absurd_number() {
        assert_eq!(
            FrameProfile::format_duration_field(Some(Duration::MAX)),
            "MAX"
        );
    }

    #[test]
    fn format_duration_field_formats_a_real_duration_as_micros() {
        assert_eq!(
            FrameProfile::format_duration_field(Some(Duration::from_millis(16))),
            "16000us"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{atlas_grew, chrome_repaint_settled, decide_chrome_mode, evaluate_chrome_gate};
    use egui::ImageData;
    use egui::epaint::{ImageDelta, Primitive};
    use egui::{Color32, ColorImage, Rect, TextureId, TextureOptions, TexturesDelta, pos2, vec2};
    use std::time::Duration;

    // ── #436.4b: `chrome_repaint_settled` ──────────────────────────────

    #[test]
    fn settled_when_egui_wants_no_repaint_and_app_requested_none() {
        assert!(chrome_repaint_settled(Duration::MAX, None));
    }

    #[test]
    fn settled_when_egui_wants_no_repaint_even_if_app_requested_a_blink_delay() {
        // Nothing is shorter than "no repaint at all" (Duration::MAX) --
        // the app's own 500ms blink request is not itself evidence of an
        // unsettled chrome.
        assert!(chrome_repaint_settled(
            Duration::MAX,
            Some(Duration::from_millis(500))
        ));
    }

    #[test]
    fn not_settled_when_egui_wants_repaint_sooner_than_the_apps_own_request() {
        // Something egui-internal (hover fade, menu animation, a focused
        // TextEdit's cursor blink) wants a wake sooner than the app's own
        // 500ms blink schedule -- not settled.
        assert!(!chrome_repaint_settled(
            Duration::from_millis(16),
            Some(Duration::from_millis(500))
        ));
    }

    #[test]
    fn settled_when_egui_delay_exactly_matches_the_apps_own_request() {
        // Nothing SHORTER than what the app itself asked for -- settled.
        assert!(chrome_repaint_settled(
            Duration::from_millis(500),
            Some(Duration::from_millis(500))
        ));
    }

    #[test]
    fn settled_when_egui_delay_is_longer_than_the_apps_own_request() {
        assert!(chrome_repaint_settled(
            Duration::from_secs(1),
            Some(Duration::from_millis(500))
        ));
    }

    #[test]
    fn not_settled_when_app_requested_nothing_but_egui_still_wants_a_repaint() {
        assert!(!chrome_repaint_settled(Duration::from_millis(16), None));
    }

    // ── #436.4b: `decide_chrome_mode` ──────────────────────────────────

    /// Bundles `decide_chrome_mode`'s nine gate inputs so each test can
    /// start from an all-clear baseline (which decides `Replay`) and flip
    /// exactly one field to prove that field alone forces `Full`. A plain
    /// tuple would work too but trips `clippy::type_complexity`; a named
    /// struct is also more readable at each call site below.
    struct Inputs {
        cache_valid: bool,
        cache_size: [u32; 2],
        cache_ppp: f32,
        cur_size: [u32; 2],
        cur_ppp: f32,
        prev_repaint_delay: Duration,
        prev_terminal_requested_delay: Option<Duration>,
        prev_chrome_damage: crate::ChromeDamage,
        chrome_input_this_frame: bool,
    }

    impl Inputs {
        /// An all-clear input set that should decide `Replay`.
        fn all_clear() -> Self {
            Self {
                cache_valid: true,
                cache_size: [800, 600],
                cache_ppp: 1.0,
                cur_size: [800, 600],
                cur_ppp: 1.0,
                prev_repaint_delay: Duration::MAX,
                prev_terminal_requested_delay: None,
                prev_chrome_damage: crate::ChromeDamage::Unchanged,
                chrome_input_this_frame: false,
            }
        }

        fn decide(&self) -> crate::ChromeMode {
            decide_chrome_mode(
                self.cache_valid,
                self.cache_size,
                self.cache_ppp,
                self.cur_size,
                self.cur_ppp,
                self.prev_repaint_delay,
                self.prev_terminal_requested_delay,
                self.prev_chrome_damage,
                self.chrome_input_this_frame,
            )
        }

        /// The four raw gate predicates -- used by the tests below to prove
        /// `evaluate_chrome_gate` (the gate-blocker instrumentation's data
        /// source) fails exactly the expected predicate(s), independent of
        /// `decide_chrome_mode`'s Full/Replay collapse.
        fn gate(&self) -> super::ChromeGatePredicates {
            evaluate_chrome_gate(
                self.cache_valid,
                self.cache_size,
                self.cache_ppp,
                self.cur_size,
                self.cur_ppp,
                self.prev_repaint_delay,
                self.prev_terminal_requested_delay,
                self.prev_chrome_damage,
                self.chrome_input_this_frame,
            )
        }
    }

    #[test]
    fn all_clear_decides_replay() {
        assert_eq!(Inputs::all_clear().decide(), crate::ChromeMode::Replay);
    }

    // ── `evaluate_chrome_gate` (gate-blocker instrumentation's data source) ──
    // These mirror the `decide_chrome_mode` per-predicate tests below, but
    // assert on the individual `ChromeGatePredicates` fields rather than the
    // collapsed `ChromeMode`, proving the instrumentation can distinguish
    // WHICH predicate(s) failed -- including more than one at once.

    #[test]
    fn gate_all_clear_passes_every_predicate() {
        let gate = Inputs::all_clear().gate();
        assert!(gate.cache_matches);
        assert!(gate.repaint_settled);
        assert!(gate.damage_unchanged);
        assert!(gate.no_chrome_input);
        assert!(gate.all_pass());
    }

    #[test]
    fn gate_invalid_cache_fails_only_cache_matches() {
        let gate = Inputs {
            cache_valid: false,
            ..Inputs::all_clear()
        }
        .gate();
        assert!(!gate.cache_matches);
        assert!(gate.repaint_settled);
        assert!(gate.damage_unchanged);
        assert!(gate.no_chrome_input);
    }

    #[test]
    fn gate_not_settled_fails_only_repaint_settled() {
        let gate = Inputs {
            prev_repaint_delay: Duration::from_millis(16),
            prev_terminal_requested_delay: Some(Duration::from_millis(500)),
            ..Inputs::all_clear()
        }
        .gate();
        assert!(gate.cache_matches);
        assert!(!gate.repaint_settled);
        assert!(gate.damage_unchanged);
        assert!(gate.no_chrome_input);
    }

    #[test]
    fn gate_chrome_damage_changed_fails_only_damage_unchanged() {
        let gate = Inputs {
            prev_chrome_damage: crate::ChromeDamage::Changed,
            ..Inputs::all_clear()
        }
        .gate();
        assert!(gate.cache_matches);
        assert!(gate.repaint_settled);
        assert!(!gate.damage_unchanged);
        assert!(gate.no_chrome_input);
    }

    #[test]
    fn gate_chrome_input_fails_only_no_chrome_input() {
        let gate = Inputs {
            chrome_input_this_frame: true,
            ..Inputs::all_clear()
        }
        .gate();
        assert!(gate.cache_matches);
        assert!(gate.repaint_settled);
        assert!(gate.damage_unchanged);
        assert!(!gate.no_chrome_input);
    }

    /// Proves overlap is visible: a frame that fails BOTH the cache-match
    /// AND the chrome-input predicates simultaneously reports both as
    /// failed, not just one -- this is exactly what lets the (feature-gated)
    /// `run_frame` instrumentation increment more than one gate-blocker
    /// counter on the same frame.
    #[test]
    fn gate_multiple_simultaneous_failures_are_all_visible() {
        let gate = Inputs {
            cache_valid: false,
            chrome_input_this_frame: true,
            ..Inputs::all_clear()
        }
        .gate();
        assert!(!gate.cache_matches);
        assert!(gate.repaint_settled);
        assert!(gate.damage_unchanged);
        assert!(!gate.no_chrome_input);
        assert!(!gate.all_pass());
    }

    #[test]
    fn invalid_cache_decides_full() {
        let inputs = Inputs {
            cache_valid: false, // no cache yet (e.g. frame 0)
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    #[test]
    fn size_mismatch_decides_full() {
        let inputs = Inputs {
            cur_size: [801, 600], // resized since the cache was built
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    #[test]
    fn ppp_mismatch_decides_full() {
        let inputs = Inputs {
            cur_ppp: 1.25, // DPI/zoom changed since the cache was built
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    #[test]
    fn prev_chrome_damage_changed_decides_full() {
        let inputs = Inputs {
            prev_chrome_damage: crate::ChromeDamage::Changed,
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    #[test]
    fn chrome_input_this_frame_decides_full() {
        let inputs = Inputs {
            chrome_input_this_frame: true, // e.g. a mouse click landed this frame
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    #[test]
    fn not_settled_repaint_decides_full() {
        let inputs = Inputs {
            // egui wants a wake sooner than what the app itself requested.
            prev_repaint_delay: Duration::from_millis(16),
            prev_terminal_requested_delay: Some(Duration::from_millis(500)),
            ..Inputs::all_clear()
        };
        assert_eq!(inputs.decide(), crate::ChromeMode::Full);
    }

    /// The headline #436 case (design's test #2): a blinking cursor with
    /// nothing else happening -- egui's delay matches exactly what the app
    /// itself requested (its own blink schedule), chrome is provably
    /// unchanged, and there is no chrome input this frame -- decides
    /// `Replay`.
    #[test]
    fn blinking_cursor_idle_frame_decides_replay() {
        assert_eq!(
            decide_chrome_mode(
                true,
                [800, 600],
                1.0,
                [800, 600],
                1.0,
                Duration::from_millis(500),
                Some(Duration::from_millis(500)),
                crate::ChromeDamage::Unchanged,
                false,
            ),
            crate::ChromeMode::Replay
        );
    }

    // ── #436.7: multi-frame FULL/REPLAY sequence properties ─────────────
    //
    // `decide_chrome_mode` is pure and consumes only last-frame signals, so
    // a frame sequence is faithfully modelled by chaining calls and threading
    // each frame's outputs into the next frame's `prev_*` inputs — exactly
    // what `run_frame` does (see the `self.prev_* = ...` stashes at the end
    // of `run_frame`). These tests drive the design's mandatory sequence
    // scenarios (#436 §9 tests #6-#8 + OQ-4) at the decision layer. The
    // pixel-level halves (byte-identical framebuffer, real texture isolation,
    // real ghost-free rendering) need the GPU harness and are 436.9's.

    /// Design test #8: the settings window NEVER REPLAYs. It has no
    /// `PerWindowState`, so `App::take_chrome_damage` returns its
    /// `ChromeDamage::Changed` default every frame; `run_frame` stashes that
    /// into `prev_chrome_damage`. Even once its own cache is valid at a stable
    /// size/ppp and egui is settled, `prev_chrome_damage == Changed` forces
    /// `Full` on every single frame, forever — never `Replay`.
    #[test]
    fn settings_window_never_replays_over_a_frame_sequence() {
        // Frame 0: no cache yet -> Full regardless of everything else.
        let frame0 = Inputs {
            cache_valid: false,
            ..Inputs::all_clear()
        };
        assert_eq!(frame0.decide(), crate::ChromeMode::Full);

        // Frames 1..N: cache is now valid at a stable size/ppp and egui is
        // fully settled — the ONLY thing keeping it Full is that
        // `take_chrome_damage` always returns `Changed` for a window with no
        // `PerWindowState`, which `run_frame` fed back as `prev_chrome_damage`.
        for frame in 1..8 {
            let inputs = Inputs {
                cache_valid: true,
                prev_chrome_damage: crate::ChromeDamage::Changed,
                ..Inputs::all_clear()
            };
            assert_eq!(
                inputs.decide(),
                crate::ChromeMode::Full,
                "settings-window frame {frame} must be Full (stuck on Changed forever)"
            );
        }
    }

    /// Design test #7 (decision-layer half): two windows decide independently.
    /// `decide_chrome_mode` takes only explicit per-window inputs and holds no
    /// shared state, so one window's decision cannot influence another's. The
    /// GPU/texture-isolation half is 436.9's.
    #[test]
    fn two_windows_decide_independently() {
        // Window A: fully idle/settled -> Replay.
        let window_a = Inputs::all_clear();
        // Window B: mid-resize (size mismatch) and chrome changed last frame
        // -> Full. Same frame, different window, independent inputs.
        let window_b = Inputs {
            cur_size: [1024, 768],
            prev_chrome_damage: crate::ChromeDamage::Changed,
            ..Inputs::all_clear()
        };
        assert_eq!(window_a.decide(), crate::ChromeMode::Replay);
        assert_eq!(window_b.decide(), crate::ChromeMode::Full);
        // Re-decide A after B to prove B left no residue (pure fn, but pin it).
        assert_eq!(window_a.decide(), crate::ChromeMode::Replay);
    }

    /// OQ-4 golden idle sequence: after startup, a steady idle screen yields
    /// exactly ONE Full frame (frame 0, cache not yet built) and then all
    /// REPLAY. Models warm-up completing and the cache arming.
    #[test]
    fn golden_idle_sequence_is_one_full_then_all_replay() {
        // Frame 0: cache not yet built -> Full (builds the cache).
        let frame0 = Inputs {
            cache_valid: false,
            ..Inputs::all_clear()
        };
        assert_eq!(frame0.decide(), crate::ChromeMode::Full);

        // Frames 1..N: cache valid, nothing changed, egui settled, no input
        // -> Replay every frame. (Warm-up's extra forced-Full frames are
        // modelled via `prev_chrome_damage`; here we start from the
        // post-warm-up steady state where chrome has gone Unchanged.)
        for frame in 1..10 {
            let inputs = Inputs::all_clear();
            assert_eq!(
                inputs.decide(),
                crate::ChromeMode::Replay,
                "idle frame {frame} after warm-up must be Replay"
            );
        }
    }

    /// Design test #6 (decision-layer half): an overlay opening then closing
    /// re-arms the cache only after chrome has been quiet through the settle
    /// window — the decision sequence never permits a REPLAY frame that would
    /// paint a stale "overlay still open" tail. Models the `ChromeDamage`
    /// sequence a modal open->close produces (§3.5 settle rule keeps the
    /// close frame AND the next frame `Changed`).
    #[test]
    fn overlay_open_close_rearms_cache_only_after_settle() {
        // A helper: decide this frame given the chrome_damage the PREVIOUS
        // frame reported (that is what run_frame threads into prev_chrome_damage).
        let decide_with_prev = |prev: crate::ChromeDamage| {
            Inputs {
                prev_chrome_damage: prev,
                ..Inputs::all_clear()
            }
            .decide()
        };

        // The chrome_damage each frame REPORTS (as decide_chrome_damage would):
        //   f1 modal opens          -> Changed
        //   f2 modal still open      -> Changed
        //   f3 modal closes (transition) -> Changed
        //   f4 settle frame (§3.5)   -> Changed
        //   f5 quiet                 -> Unchanged
        //   f6 quiet                 -> Unchanged
        // run_frame decides frame N using frame N-1's reported chrome_damage.
        // So the earliest REPLAY is the frame whose PREVIOUS frame reported
        // Unchanged — i.e. f6 (prev = f5 = Unchanged).
        assert_eq!(
            decide_with_prev(crate::ChromeDamage::Changed),
            crate::ChromeMode::Full
        ); // during f2 (prev f1)
        assert_eq!(
            decide_with_prev(crate::ChromeDamage::Changed),
            crate::ChromeMode::Full
        ); // during f3 (prev f2)
        assert_eq!(
            decide_with_prev(crate::ChromeDamage::Changed),
            crate::ChromeMode::Full
        ); // during f4 (prev f3, the close)
        assert_eq!(
            decide_with_prev(crate::ChromeDamage::Changed),
            crate::ChromeMode::Full
        ); // during f5 (prev f4, the settle)
        // Only now, after a fully-quiet previous frame, may the cache re-arm:
        assert_eq!(
            decide_with_prev(crate::ChromeDamage::Unchanged),
            crate::ChromeMode::Replay,
            "cache may re-arm to Replay only after chrome was quiet last frame"
        );
    }

    // ── #436.4c §5.2: `atlas_grew` ──────────────────────────────────────

    /// A tiny 2x2 filled `ImageData`, sufficient to build `ImageDelta`
    /// values without depending on any real font/image data.
    fn tiny_image() -> egui::ImageData {
        ImageData::Color(std::sync::Arc::new(ColorImage::filled(
            [2, 2],
            Color32::WHITE,
        )))
    }

    #[test]
    fn atlas_grew_false_on_empty_delta() {
        assert!(!atlas_grew(&TexturesDelta::default()));
    }

    #[test]
    fn atlas_grew_true_on_font_atlas_whole_upload() {
        // `ImageDelta::full` sets `pos: None` -- `is_whole() == true` --
        // exactly the resize/respecify marker the self-heal watches for,
        // on `TextureId::default()` (the font atlas).
        let delta = ImageDelta::full(tiny_image(), TextureOptions::default());
        assert!(delta.is_whole(), "sanity: ImageDelta::full is whole");
        let mut textures_delta = TexturesDelta::default();
        textures_delta.set.push((TextureId::default(), delta));
        assert!(atlas_grew(&textures_delta));
    }

    #[test]
    fn atlas_grew_false_on_font_atlas_incremental_patch() {
        // `ImageDelta::partial` sets `pos: Some(_)` -- a normal
        // new-glyph-added-to-the-existing-atlas upload, NOT a resize. Must
        // NOT be mistaken for atlas growth.
        let delta = ImageDelta::partial([0, 0], tiny_image(), TextureOptions::default());
        assert!(
            !delta.is_whole(),
            "sanity: ImageDelta::partial is not whole"
        );
        let mut textures_delta = TexturesDelta::default();
        textures_delta.set.push((TextureId::default(), delta));
        assert!(!atlas_grew(&textures_delta));
    }

    #[test]
    fn atlas_grew_false_on_whole_upload_of_a_non_font_texture() {
        // A whole-texture upload of some OTHER texture (e.g. an
        // image-protocol texture) is not the font atlas and must not
        // trigger the chrome-UV self-heal.
        let delta = ImageDelta::full(tiny_image(), TextureOptions::default());
        let mut textures_delta = TexturesDelta::default();
        textures_delta.set.push((TextureId::User(42), delta));
        assert!(!atlas_grew(&textures_delta));
    }

    #[test]
    fn atlas_grew_true_when_font_atlas_whole_upload_mixed_with_other_entries() {
        // A realistic frame may carry multiple texture deltas; growth must
        // be detected even when the font-atlas entry is not the only one
        // (or not first).
        let mut textures_delta = TexturesDelta::default();
        textures_delta.set.push((
            TextureId::User(7),
            ImageDelta::partial([0, 0], tiny_image(), TextureOptions::default()),
        ));
        textures_delta.set.push((
            TextureId::default(),
            ImageDelta::full(tiny_image(), TextureOptions::default()),
        ));
        assert!(atlas_grew(&textures_delta));
    }

    /// Reproduces the exact §5.2 hazard end-to-end using only
    /// `egui::Context` (pure CPU, no GL/painter needed — this does NOT
    /// require the 436.9 pixel harness) and proves the self-heal's core
    /// operation — re-tessellating cached shapes against the current atlas
    /// — actually fixes it.
    ///
    /// Mechanics (see `atlas_grew`'s doc comment): a `rect_filled` shape's
    /// UV is the constant `WHITE_UV` (invariant to atlas size — verified
    /// against `epaint::WHITE_UV`/`Mesh::add_colored_rect`), but a TEXT
    /// shape's glyph UVs are normalized by the LIVE atlas size at the
    /// moment `ctx.tessellate` is called
    /// (`epaint::tessellator::Tessellator::tessellate_text`).
    /// `TextureAtlas::allocate` doubles the atlas height *in place*
    /// whenever a glyph's row does not fit, marking the WHOLE image dirty
    /// (`ImageDelta::full`, `pos: None`) for that frame's `textures_delta`
    /// — so cached chrome text tessellated *before* a resize holds
    /// different glyph UVs than tessellating the identical shapes again
    /// *afterward*.
    ///
    /// How this test forces the resize: it rasterizes a large run of glyphs
    /// at a font size (80pt) far taller than the freshly-initialized atlas's
    /// first row, which reliably overflows the initial atlas height and
    /// triggers the in-place doubling on the very next frame. The *specific*
    /// glyphs are immaterial — the trigger is atlas-space exhaustion, not
    /// glyph novelty per se (the bundled egui fonts have no CJK coverage, so
    /// the U+4E00.. codepoints below actually rasterize as the fallback/tofu
    /// glyph; that does not matter, since a single tall glyph is enough to
    /// overflow the tiny initial row). In production the same resize is
    /// driven by any glyph the terminal band or an overlay shapes that does
    /// not fit the current atlas — a normal-size never-before-seen glyph is
    /// the common case; a large one is simply the most reliable way to force
    /// it deterministically in a test. The `atlas_grew` sanity assert below
    /// fails loudly (never vacuously passes) if the resize ever stops
    /// happening, so the test cannot silently rot into a no-op.
    #[test]
    fn atlas_growth_invalidates_cached_text_uvs_and_retessellation_fixes_it() {
        let ctx = egui::Context::default();
        let ppp = 1.0;

        // Frame 1 ("chrome"): a small text label. Establishes a baseline
        // atlas and a cached shape list + its tessellation — this stands
        // in for `ChromeCache::{head_shapes, head_primitives}` after a
        // FULL frame.
        let chrome_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("Chrome");
        });
        let cached_shapes = chrome_output.shapes;
        let cached_primitives_before = ctx.tessellate(cached_shapes.clone(), ppp);

        // Frame 2 ("terminal band"): a large run of glyphs at a font size
        // (80pt) far taller than the freshly-initialized atlas's first row,
        // which overflows the initial atlas height and forces the in-place
        // doubling. This is NOT a chrome trigger (#436 §3.3 — terminal
        // content never forces FULL on its own) — exactly the
        // REPLAY-candidate scenario §5.2 describes: the band grows the shared
        // font atlas mid-REPLAY. (The codepoints are in the CJK range but the
        // bundled fonts lack CJK coverage, so they rasterize as the fallback
        // glyph — immaterial, since the resize is driven by glyph SIZE
        // overflowing the atlas row, not by which glyph it is; see the fn
        // doc comment.)
        let big_text: String = (0x4E00u32..0x4E00u32 + 300)
            .filter_map(char::from_u32)
            .collect();
        let band_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label(egui::RichText::new(big_text.clone()).size(80.0));
        });

        assert!(
            atlas_grew(&band_output.textures_delta),
            "sanity: the band frame must actually grow the font atlas for \
             this test to exercise the hazard"
        );

        // The hazard: painting `cached_primitives_before` now (after
        // growth, without re-tessellating) would sample the WRONG atlas
        // rows — prove it by re-tessellating the IDENTICAL cached shapes
        // and observing at least one glyph UV changed. `WHITE_UV`-only
        // vertices (rect fills) are excluded since they are
        // atlas-size-invariant by construction.
        let cached_primitives_after = ctx.tessellate(cached_shapes.clone(), ppp);
        let mut any_glyph_uv_changed = false;
        for (before, after) in cached_primitives_before
            .iter()
            .zip(cached_primitives_after.iter())
        {
            if let (Primitive::Mesh(mesh_before), Primitive::Mesh(mesh_after)) =
                (&before.primitive, &after.primitive)
            {
                for (vb, va) in mesh_before.vertices.iter().zip(mesh_after.vertices.iter()) {
                    if vb.uv != egui::epaint::WHITE_UV && vb.uv != va.uv {
                        any_glyph_uv_changed = true;
                    }
                }
            }
        }
        assert!(
            any_glyph_uv_changed,
            "the cached chrome text's glyph UVs must differ before/after \
             atlas growth -- this is the exact staleness `run_frame`'s \
             REPLAY self-heal (re-tessellating `cache.head_shapes`/\
             `cache.tail_shapes`) corrects"
        );

        // The fix: this IS the self-heal's operation
        // (`self.ctx.tessellate(cache.head_shapes.clone(), pixels_per_point)`
        // in `run_frame`). Prove it converges: tessellating the cached
        // shapes AGAIN, with no further atlas change, reproduces the exact
        // same (correct, current-atlas) primitives — i.e. the self-heal
        // result is stable once applied, matching what `run_frame` stores
        // back into `cache.head_primitives`/`cache.tail_primitives`.
        let cached_primitives_after_again = ctx.tessellate(cached_shapes, ppp);
        assert_eq!(
            flatten_mesh_geometry(&cached_primitives_after),
            flatten_mesh_geometry(&cached_primitives_after_again),
            "re-tessellating the self-healed shapes again (no further atlas \
             change) must reproduce identical geometry -- the self-heal \
             result is stable"
        );
    }

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

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
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

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
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

        let full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
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
