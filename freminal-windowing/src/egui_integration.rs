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
/// FULL frame (#436.4a/#436.4b).
///
/// Populated at the end of every FULL frame; consulted (via
/// [`decide_chrome_mode`]) to decide whether a later frame may REPLAY, and
/// when it does, `run_frame` paints `head_primitives`/`tail_primitives`
/// directly instead of re-tessellating. Only the tessellated primitives are
/// retained — not the source shapes — because #436.4b invalidates the
/// whole cache (forcing `ChromeMode::Full`) on any `ppp`/`size` mismatch
/// rather than re-tessellating the same shapes at a new `ppp`; a future
/// subtask that wants that finer-grained recovery would need to retain the
/// shapes too.
struct ChromeCache {
    /// Tessellation of the chrome shapes painted *before* the terminal band
    /// in `full_output.shapes` (e.g. the `CentralPanel` background fill,
    /// menu bar, tab bar), at `ppp`/`size`.
    head_primitives: Vec<egui::ClippedPrimitive>,
    /// Tessellation of the chrome shapes painted *after* the terminal band
    /// (overlays, pane borders drawn as part of chrome, modals, tooltips),
    /// at `ppp`/`size`.
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

/// #436.4b: decide this frame's [`crate::ChromeMode`].
///
/// A REPLAY is permitted only when ALL of the following hold:
///   - `cache_valid` — a chrome cache exists from a prior FULL frame.
///   - `cache_size`/`cache_ppp` match `cur_size`/`cur_ppp` — the cached
///     primitives were tessellated at this frame's framebuffer size and
///     scale factor (a mismatch, e.g. a resize or DPI change, invalidates
///     the whole cache rather than attempting a partial re-tessellation).
///   - [`chrome_repaint_settled`] — nothing egui-internal wants a wake
///     sooner than the app's own scheduling this frame (§3.1 amendment).
///   - `prev_chrome_damage` is [`crate::ChromeDamage::Unchanged`] — the
///     PREVIOUS frame proved static chrome did not change (§3.3/§3.5).
///   - `!chrome_input_this_frame` — no window input event this frame could
///     plausibly have affected chrome (§3.2, conservative: any qualifying
///     input forces `Full`, refined region-aware in #436.8).
///
/// Any failure forces `ChromeMode::Full` — the always-correct, conservative
/// default. Pure (no `self`/`egui` state), so directly unit-testable.
// Nine independent, unrelated gate inputs (cache validity/size/ppp, this
// frame's size/ppp, the two settle-rule delays, prior chrome damage, and the
// input gate) -- bundling them into a struct would just relocate the same
// fields without adding clarity, and this function exists specifically so
// tests can drive each one independently.
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
    let cache_matches =
        cache_valid && cache_size == cur_size && (cache_ppp - cur_ppp).abs() < f32::EPSILON;

    let replay_allowed = cache_matches
        && chrome_repaint_settled(prev_repaint_delay, prev_terminal_requested_delay)
        && prev_chrome_damage == crate::ChromeDamage::Unchanged
        && !chrome_input_this_frame;

    if replay_allowed {
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
        let full_output = self.ctx.run_ui(raw_input, |root_ui| {
            let signals = ui_fn(&*root_ui, &gl_state.glow_context, chrome_mode);
            frame_damage = signals.frame_damage;
            band_range = signals.band_range;
            chrome_damage = signals.chrome_damage;
            terminal_requested_delay = signals.terminal_requested_delay;
        });

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        // Definitive `pixels_per_point` for THIS frame — read AFTER `run_ui`
        // has processed `raw_input` via `begin_pass`, so (unlike
        // `ppp_before_run_ui` above) this always reflects a scale-factor
        // change delivered this frame. Used for all tessellation below.
        let pixels_per_point = self.ctx.pixels_per_point();

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
        let (head_primitives, tail_primitives) = match chrome_mode {
            crate::ChromeMode::Full => {
                let head_shapes: Vec<egui::epaint::ClippedShape> = shapes[..start].to_vec();
                let tail_shapes: Vec<egui::epaint::ClippedShape> = shapes[end..].to_vec();
                let head_primitives = self.ctx.tessellate(head_shapes, pixels_per_point);
                let tail_primitives = self.ctx.tessellate(tail_shapes, pixels_per_point);
                self.chrome_cache = Some(ChromeCache {
                    head_primitives: head_primitives.clone(),
                    tail_primitives: tail_primitives.clone(),
                    ppp: pixels_per_point,
                    size: size_arr,
                });
                (head_primitives, tail_primitives)
            }
            crate::ChromeMode::Replay => {
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
        self.painter
            .paint_primitives(size_px, pixels_per_point, &head_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &band_primitives);
        self.painter
            .paint_primitives(size_px, pixels_per_point, &tail_primitives);
        for id in &full_output.textures_delta.free {
            self.painter.free_texture(*id);
        }

        // Pre-present notify for Wayland frame pacing
        window.pre_present_notify();

        let swap_result = partial.as_ref().map_or_else(
            || gl_state.swap_buffers(),
            |rects| gl_state.swap_buffers_with_damage(rects),
        );
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

#[cfg(test)]
mod tests {
    use super::{chrome_repaint_settled, decide_chrome_mode};
    use egui::epaint::Primitive;
    use egui::{Color32, Rect, pos2, vec2};
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
    }

    #[test]
    fn all_clear_decides_replay() {
        assert_eq!(Inputs::all_clear().decide(), crate::ChromeMode::Replay);
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
}
