// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! winit event loop and `ApplicationHandler` implementation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Instant;

use tracing::{debug, error, info};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowAttributes};

use crate::egui_integration::EguiState;
use crate::error::Error;
use crate::gl_context::GlState;
use crate::{
    App, FrameSignals, RawKeyEvent, RawKeyMods, UserEvent, WindowConfig, WindowGeometry,
    WindowHandle, WindowId, WindowOp,
};

use conv2::{ApproxFrom, ConvUtil, RoundToZero};

/// Convert an `f64` logical dimension to `u32`, clamping non-positive and
/// non-finite values to 0 and saturating on overflow.  Used for logical
/// window sizes, which should never realistically exceed `u32::MAX`.
///
/// Positive sub-pixel values (e.g. `0.25`) are rounded *up* so that any
/// strictly-positive dimension yields at least `1`.  `round()` would map
/// such values to `0`, turning a "tiny but non-empty" window into a
/// zero-size window on round-trip through persisted state.
fn logical_dim_to_u32(v: f64) -> u32 {
    if !v.is_finite() || v <= 0.0 {
        return 0;
    }
    <u32 as ApproxFrom<f64, RoundToZero>>::approx_from(v.ceil()).unwrap_or(u32::MAX)
}

/// Convert a rounded `f64` logical coordinate to `i32`, saturating on
/// overflow in either direction.  Used for logical window positions which
/// can be negative on multi-monitor setups.
fn logical_coord_to_i32(v: f64) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    <i32 as ApproxFrom<f64, RoundToZero>>::approx_from(v.round()).unwrap_or_else(|_| {
        if v.is_sign_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}

/// Minimum interval between *delay-scheduled* repaints — the 60fps frame
/// budget.
///
/// Every path that schedules a repaint *after a delay* (both the cross-thread
/// [`UserEvent::RequestRepaintAfter`] and the same-thread
/// [`WindowOp::RequestRepaintAfter`], plus egui's own
/// `frame_output.repaint_delay`) floors that delay to this value so that no
/// single source can drive the GUI faster than ~60fps while idle. (Discrete
/// *immediate* repaints — `RequestRepaint`, resize/scale/occlusion redraws —
/// are one-shot responses to real events, not continuous streams, and are
/// intentionally not throttled here.)
///
/// This closes the issue #439 loophole where the cross-thread
/// [`UserEvent::RequestRepaintAfter`] path (used by the PTY consumer thread)
/// accepted an unclamped sub-16ms delay and `min`'d below any already-floored
/// deadline, letting a bursty PTY output stream (btop, htop, vim, less) drive
/// ~40+ full frames/sec for a screen that visually changes ~2x/sec.
const MIN_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);

/// Floor a requested repaint delay to [`MIN_REPAINT_INTERVAL`].
///
/// Pure so it can be unit-tested without a running event loop. A caller may
/// legitimately request a longer delay (e.g. a 500ms cursor-blink wake or a
/// 250ms toast-fade); those pass through unchanged. Only sub-16ms requests
/// are raised to the floor.
fn clamp_repaint_delay(delay: std::time::Duration) -> std::time::Duration {
    delay.max(MIN_REPAINT_INTERVAL)
}

/// Task 121 spike: fallback wake interval used when pointer motion was
/// suppressed, egui asked for an immediate repaint purely because of those
/// suppressed events, and the app itself requested no delay at all.
///
/// Deliberately bounded rather than "never repaint": the app requests nothing
/// when there is no blink schedule to honour (e.g. `DECTCEM` has hidden the
/// cursor under btop/vim), and in that state an unbounded wait would stall the
/// window until an unrelated event happened to arrive.
///
/// Subtask 121.12 routed every in-frame freminal repaint need (bell flash,
/// cursor trail, animated images, gutter hover, scrollbar damage) through
/// `app_requested_delay`, so the only remaining unrepresented need here is
/// egui's OWN chrome animation — e.g. a hover/tooltip fade still settling
/// after the pointer moved off chrome onto terminal content. egui's raw
/// delay is `ZERO` from the suppressed events regardless, so that need is
/// indistinguishable from "nothing needs a repaint" and the fallback must
/// stay bounded — `Duration::MAX` would freeze such a fade at partial alpha
/// until an unrelated event arrived.
///
/// 500ms (not the previous 250ms) is chosen to equal the cursor-blink period
/// requested at `app_impl.rs`'s per-pane scheduling, so that turning the
/// cursor blink OFF can never schedule MORE frames than leaving it on. The
/// old 250ms produced exactly that perversity: blink-on floored at a 2fps
/// wake, blink-off (no `app_requested_delay` at all) fell back to 4fps —
/// disabling the blink made the idle GUI repaint *more* often, not less.
///
/// ## Two distinct, honest gaps (review SHOULD-FIX #2/#3)
///
/// The "egui-internal chrome animation" risk above is actually TWO separate
/// mechanisms, and conflating them understates the second:
///
///   1. **Scheduling cadence.** Even when something egui-internal legitimately
///      wants a wake, this fallback only guarantees a wake every 500ms rather
///      than every frame — a bounded but coarser cadence than an unsuppressed
///      frame would give it. This is the risk the paragraph above describes.
///   2. **Non-construction, not just under-scheduling.** freminal's own chrome
///      widgets (menu bar, tab bar) are not merely repainted less often on a
///      settled/`Replay` frame — see `app_impl.rs`'s "FULL vs REPLAY chrome
///      construction" comment — they are not CONSTRUCTED at all. An
///      egui-internal animation living inside one of those widgets (e.g. a
///      hover-fade `Response` that needs `ctx.request_repaint` called again
///      next frame to keep advancing) would not just be scheduled less often
///      under continuous suppressed pointer motion; its own advancing logic
///      would simply not run, because the widget that would have driven it is
///      not built on `Replay` frames.
///
/// **This is latent, not live.** A repo-wide search confirms freminal's chrome
/// uses no `ctx.animate_bool` / `ctx.animate_value` anywhere (egui's own
/// per-frame animation-state helpers) — nothing in this codebase currently
/// relies on mechanism 2 actually recurring frame-over-frame while chrome is
/// unconstructed. Separately, opening a menu forces `ChromeMode::Full` via
/// `any_overlay_open` through an unrelated gate (menus are chrome input, and
/// chrome input forces Full), so the one interactive widget most likely to
/// carry egui-internal animation state cannot itself be open during a
/// `Replay` frame.
///
/// The Item 1 residual-gap fix (`effective_chrome_gate_delay`, see its doc)
/// widens this latent window: settling the chrome gate on `app_requested_delay:
/// None` now permits `Replay` in the "app requested nothing" case that used to
/// pin `Full`, so both mechanisms above are reachable in a strictly larger set
/// of frames than before that fix. This was a deliberate, accepted trade
/// (maintainer decision) — the primary win (correct settling for the
/// btop/`DECTCEM`-hidden-cursor workload) outweighs a latent risk with no
/// currently-existing trigger.
const SUPPRESSED_POINTER_FALLBACK_DELAY: std::time::Duration =
    std::time::Duration::from_millis(500);

/// Task 121 spike: decide the repaint delay to actually schedule, given what
/// egui asked for and whether the only thing that happened since the previous
/// frame was pointer motion the app classified as needing no frame.
///
/// egui's `InputState::wants_repaint_after` returns `Duration::ZERO` whenever
/// its event queue is non-empty. Suppressed pointer events are still handed to
/// `on_window_event` (egui's pointer state must stay fresh), so they sit in
/// that queue and egui re-arms an immediate frame from *inside* the frame —
/// a self-sustaining ~60fps loop that input-side suppression alone cannot
/// break. When `suppressed_only` holds, that zero is attributable to those
/// events and is overridden with whatever the app itself asked for.
///
/// The fallback is deliberately bounded, NOT `Duration::MAX`: the app requests
/// nothing when it has no blink schedule to honour (e.g. `DECTCEM` hid the
/// cursor under btop/vim), and an unbounded wait there would stall the window
/// until an unrelated event arrived. A stalled terminal is far worse than an
/// occasional redundant frame.
///
/// Pure so the liveness-critical substitution is unit-testable without a live
/// event loop — this is the highest-risk logic in the spike.
fn effective_repaint_delay(
    suppressed_only: bool,
    repaint_delay: std::time::Duration,
    app_requested_delay: Option<std::time::Duration>,
) -> std::time::Duration {
    if suppressed_only && repaint_delay.is_zero() {
        app_requested_delay.unwrap_or(SUPPRESSED_POINTER_FALLBACK_DELAY)
    } else {
        repaint_delay
    }
}

/// Subtask 121.13 residual-gap fix (maintainer decision): the value stashed
/// for next frame's chrome-settle check (see [`chrome_repaint_settled`] via
/// `EguiState::stash_effective_repaint_delay`), which answers a DIFFERENT
/// question than [`effective_repaint_delay`] despite sharing its inputs and
/// its substitution condition.
///
/// [`effective_repaint_delay`] answers "what delay do we actually schedule
/// the next wake at?" — and the answer there MUST be bounded
/// ([`SUPPRESSED_POINTER_FALLBACK_DELAY`]) when the app requested nothing,
/// because we cannot prove nothing needs drawing and an unbounded wait would
/// stall the window.
///
/// This function answers "did anything actually WANT a repaint this frame?"
/// — and when the app requested nothing, the absence of any request IS the
/// proof that nothing wanted one. Substituting the synthetic liveness poll
/// interval here would make it masquerade as evidence of a real want, which
/// is exactly the residual gap the maintainer flagged: `chrome_repaint_settled`'s
/// `None` arm requires the delay to equal `Duration::MAX` to call the frame
/// settled, so feeding it 500ms permanently reads as "unsettled" for as long
/// as suppressed pointer motion continues, even though nothing chrome-relevant
/// is scheduled.
///
/// The two functions diverge in EXACTLY one case: `suppressed_only &&
/// repaint_delay.is_zero() && app_requested_delay.is_none()`. Every other
/// input combination yields the same output from both — when `app_requested_delay`
/// is `Some(_)` the substitution is identical; when substitution does not
/// apply, both pass `repaint_delay` through unchanged.
///
/// Pure so the divergence itself is unit-testable without a live event loop.
fn effective_chrome_gate_delay(
    suppressed_only: bool,
    repaint_delay: std::time::Duration,
    app_requested_delay: Option<std::time::Duration>,
) -> std::time::Duration {
    if suppressed_only && repaint_delay.is_zero() {
        app_requested_delay.unwrap_or(std::time::Duration::MAX)
    } else {
        repaint_delay
    }
}

/// Returns `true` for the narrow set of physical keys that egui 0.35 cannot
/// deliver: print/pause/menu keys, keypad operators and digits, and the
/// media keys winit's `KeyCode` exposes (Task 114). These are intercepted
/// BEFORE egui-winit sees them and routed to [`App::on_raw_key_event`]
/// instead; every other key falls through to egui unchanged.
const fn is_blocked_key(key_code: winit::keyboard::KeyCode) -> bool {
    use winit::keyboard::KeyCode;

    matches!(
        key_code,
        // System keys.
        KeyCode::PrintScreen
            | KeyCode::Pause
            | KeyCode::ContextMenu
            // Keypad operators.
            | KeyCode::NumpadDivide
            | KeyCode::NumpadMultiply
            | KeyCode::NumpadSubtract
            | KeyCode::NumpadAdd
            | KeyCode::NumpadEnter
            | KeyCode::NumpadEqual
            | KeyCode::NumpadComma
            | KeyCode::NumpadDecimal
            | KeyCode::NumpadStar
            // Keypad digits (egui unifies these with the main-row digits, so
            // the physical distinction is otherwise lost).
            | KeyCode::Numpad0
            | KeyCode::Numpad1
            | KeyCode::Numpad2
            | KeyCode::Numpad3
            | KeyCode::Numpad4
            | KeyCode::Numpad5
            | KeyCode::Numpad6
            | KeyCode::Numpad7
            | KeyCode::Numpad8
            | KeyCode::Numpad9
            // Media keys.
            | KeyCode::MediaPlayPause
            | KeyCode::MediaStop
            | KeyCode::MediaTrackNext
            | KeyCode::MediaTrackPrevious
            | KeyCode::AudioVolumeUp
            | KeyCode::AudioVolumeDown
            | KeyCode::AudioVolumeMute
    )
}

/// Returns `true` for `WindowEvent`s that unconditionally force
/// `ChromeMode::Full` for the frame they arrive in, via
/// `WindowState::chrome_input_pending` — the non-pointer half of the
/// #436.4b §3.2 input gate.
///
/// Pointer events (`CursorMoved` / `MouseInput` / `MouseWheel`) are
/// deliberately EXCLUDED here (#436.8): they are region-tested instead (see
/// [`should_force_chrome_full_for_pointer`]), so that pointer motion purely
/// over terminal content does not force a chrome rebuild every frame (the
/// CPU-spike-under-btop complaint). `CursorEntered`/`CursorLeft` stay here
/// (rare events; not worth region-testing) alongside keyboard, IME, focus,
/// theme, and touch/gesture input — the maintainer decided pointer-only
/// narrowing for this subtask, no keyboard narrowing (avoids a one-frame lag
/// on keyboard-triggered chrome actions).
const fn is_unconditional_chrome_input(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. }
            | WindowEvent::KeyboardInput { .. }
            | WindowEvent::ModifiersChanged(_)
            | WindowEvent::Ime(_)
            | WindowEvent::Focused(_)
            // An OS dark/light theme switch synchronously rebuilds egui's
            // chrome visuals (see the `ThemeMode::Auto` path in the app's
            // `update`), so it must force `ChromeMode::Full` — otherwise the
            // app-level `style_changed` signal (which lags a frame, keyed off
            // the terminal snapshot's theme rather than the OS state) could
            // miss it and a REPLAY frame would paint stale-theme chrome
            // (#436.6 / §6 safety-net completeness).
            | WindowEvent::ThemeChanged(_)
            | WindowEvent::Touch(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::DoubleTapGesture { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::TouchpadPressure { .. }
    )
}

/// #436.4b §3.2 chrome-input gate decision for the general (non-pointer)
/// `window_event` path: should `event` force `WindowState::chrome_input_pending`
/// for the frame it arrives in, given whether `egui-winit`'s
/// `on_window_event` reported `repaint` for it?
///
/// This is `is_unconditional_chrome_input(event) || repaint` — EXCEPT for
/// `WindowEvent::RedrawRequested`, which always returns `false` here
/// regardless of `repaint`. That carve-out is load-bearing, not an
/// oversight — it is the fix for the #436-chrome-cache-inert bug (`Replay`
/// measured 0/360 frames at idle) and must not be "simplified" away:
///
/// - `egui-winit` 0.35.0's `on_window_event` groups `RedrawRequested` into a
///   match arm commented "Things that may require repaint:" and returns
///   `EventResponse { repaint: true, .. }` for it *unconditionally*
///   (`egui-winit-0.35.0/src/lib.rs:492-500`).
/// - `RedrawRequested` is the event that drives every single frame — see
///   this module's `window_event`'s `RedrawRequested` arm, which reads the
///   gate this function feeds via
///   `std::mem::take(&mut state.chrome_input_pending)`, roughly 110 lines
///   after the call site that uses this function, in the *same*
///   `window_event` invocation.
/// - Without the carve-out, `repaint == true` on `RedrawRequested` would set
///   `chrome_input_pending` on every frame, which `RedrawRequested`'s own
///   arm would then immediately read back as `true` — permanently
///   disqualifying `ChromeMode::Replay` regardless of any real input. This
///   is exactly the bug: the event that drives the frame set the flag that
///   disqualifies the frame.
/// - `RedrawRequested` is not user input, so excluding it from the gate is
///   correct on its own terms, not just a workaround.
///
/// Do NOT broaden this carve-out to other members of egui-winit's grouped
/// arm: `CursorEntered`/`CursorLeft` are genuine input and already covered
/// by `is_unconditional_chrome_input`; `Resized`/`Occluded` legitimately
/// affect chrome; `Destroyed`/`CloseRequested`/`Moved`/`TouchpadPressure`
/// are rare and harmless if they do force a frame `Full`. `RedrawRequested`
/// is the only member of that arm that fires every frame, which is what
/// makes it uniquely disqualifying.
const fn should_set_chrome_input_pending(event: &WindowEvent, repaint: bool) -> bool {
    if matches!(event, WindowEvent::RedrawRequested) {
        return false;
    }
    is_unconditional_chrome_input(event) || repaint
}

/// Convert a winit physical cursor position to egui logical points (lossy
/// `f64` -> `f32` narrowing via `conv2`'s default approximation, matching the
/// `window.scale_factor().approx_as::<f32>()` conversion in
/// `egui_integration.rs`). Returns `None` for a non-finite or non-positive
/// scale factor — the caller treats an unknown position conservatively (see
/// [`should_force_chrome_full_for_pointer`]).
///
/// LOAD-BEARING ASSUMPTION (#436.8): the chrome-interactive rects this
/// position is hit-tested against are captured in egui **logical points**,
/// which equal `physical / egui.pixels_per_point()`. We divide by
/// `window.scale_factor()` instead, and those are only equal while
/// `egui.pixels_per_point() == window.scale_factor()` — i.e. while egui's
/// zoom factor is exactly 1.0. Freminal guarantees this by setting
/// `Options::zoom_with_keyboard = false` (`gui/rendering.rs`) and never
/// calling `Context::set_zoom_factor`. If egui zoom is ever enabled, this
/// divisor is wrong and the region hit-test silently misclassifies chrome as
/// terminal (a stale-chrome-under-interaction bug) — see
/// `Documents/EGUI_UPGRADE_ASSUMPTIONS.md` A13. Fix then: derive the divisor
/// from `ctx.pixels_per_point()` rather than `window.scale_factor()`.
fn physical_to_logical_pos(
    pos: winit::dpi::PhysicalPosition<f64>,
    scale: f64,
) -> Option<egui::Pos2> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let x = (pos.x / scale).approx_as::<f32>().ok()?;
    let y = (pos.y / scale).approx_as::<f32>().ok()?;
    Some(egui::pos2(x, y))
}

/// #436.8 region-aware pointer chrome-gate decision: should this pointer
/// event force `ChromeMode::Full`?
///
/// `true` when a chrome-border drag is latched (`drag_latched` — the pointer
/// may have moved off the sensor mid-drag, but the drag itself is still
/// chrome-affecting), OR the pointer position is known to be over a
/// chrome-interactive region (`is_over_chrome == Some(true)`), OR the
/// position is unknown (`None` — conservative: force `Full` rather than risk
/// silently starving a chrome interaction of repaints).
fn should_force_chrome_full_for_pointer(is_over_chrome: Option<bool>, drag_latched: bool) -> bool {
    drag_latched || is_over_chrome.unwrap_or(true)
}

/// #436.8 chrome-border drag latch update: tracks presses that started over
/// a chrome-interactive region so a drag that later moves the pointer off
/// that region (still forcing `Full` via the latch) is not mistaken for
/// terminal-content motion. Saturating in both directions so an unbalanced
/// press/release sequence (e.g. a release delivered to a different window)
/// can never underflow or runaway-accumulate.
fn update_chrome_drag_latch(
    current: u32,
    button_state: winit::event::ElementState,
    is_over_chrome: Option<bool>,
) -> u32 {
    match button_state {
        // Conservative: an unknown position (`None`) counts as "over chrome"
        // for latch purposes, same as the force-Full decision itself.
        winit::event::ElementState::Pressed if is_over_chrome.unwrap_or(true) => {
            current.saturating_add(1)
        }
        winit::event::ElementState::Pressed => current,
        winit::event::ElementState::Released => current.saturating_sub(1),
    }
}

/// Task 121 spike: should THIS `CursorMoved` event actually schedule a
/// repaint (i.e. set `WindowState::repaint_at`)?
///
/// `chrome_drag_latched` (`chrome_drag_pressed_count > 0`) always forces
/// `true` — a chrome-border drag in progress must keep repainting regardless
/// of the app's opinion, mirroring [`should_force_chrome_full_for_pointer`]'s
/// same latch on the separate chrome-damage axis.
///
/// Otherwise: `previous_needed || current_needed`. `current_needed` is
/// `App::pointer_motion_needs_repaint`'s answer for THIS event's position;
/// `previous_needed` is that same answer for the PRIOR `CursorMoved` event.
/// The `previous_needed` half is the edge-detect: on a needed -> not-needed
/// transition (`previous_needed == true`, `current_needed == false`), this
/// still evaluates `true` — i.e. the transition frame itself is repainted —
/// so chrome the pointer just left (e.g. a hover tint) is redrawn one final
/// time before suppression begins on the FOLLOWING event (where
/// `previous_needed` has by then been updated to `false`). A
/// not-needed -> needed transition schedules via `current_needed` alone, and
/// two consecutive not-needed events correctly suppress
/// (`false || false == false`).
///
/// Pure, so directly unit-testable without a live event loop.
const fn should_schedule_cursor_moved(
    chrome_drag_latched: bool,
    previous_needed: bool,
    current_needed: bool,
) -> bool {
    chrome_drag_latched || previous_needed || current_needed
}

/// Per-window state.
struct WindowState {
    window: Window,
    gl: GlState,
    egui: EguiState,
    /// Next scheduled repaint time (if any).
    repaint_at: Option<Instant>,
    /// #436.4b §3.2 chrome-input gate: set `true` by a window input event
    /// this frame that forces `ChromeMode::Full` — either unconditionally
    /// (keyboard, focus, IME, theme — see [`is_unconditional_chrome_input`])
    /// or, for pointer events, only when the pointer is over (or mid-drag on)
    /// a chrome-interactive region (#436.8, see
    /// [`should_force_chrome_full_for_pointer`]). Drained (`mem::take`) into
    /// `run_frame`'s `chrome_input_this_frame` parameter at
    /// `RedrawRequested`.
    chrome_input_pending: bool,
    /// #436.8: last-known pointer position in egui logical points, updated on
    /// every `CursorMoved` and cleared on `CursorLeft`. `None` before the
    /// first `CursorMoved` (or after the pointer has left the window) — the
    /// region hit-test then has no position to test and callers treat that
    /// conservatively (force `Full`).
    last_cursor_pos: Option<egui::Pos2>,
    /// #436.8 chrome-border drag latch: incremented on a button press whose
    /// position is over (or unknown, conservatively) a chrome-interactive
    /// region, decremented on release. While `> 0`, pointer motion/wheel
    /// events force `ChromeMode::Full` regardless of the current pointer
    /// position, so a drag that moves off the sensor mid-drag is not
    /// mistaken for terminal-content motion.
    chrome_drag_pressed_count: u32,
    /// Task 121 spike: whether the PREVIOUS `CursorMoved` event decided a
    /// repaint was needed (before the chrome-drag-latch override). Consulted
    /// by [`should_schedule_cursor_moved`] to edge-detect a needed ->
    /// not-needed transition, so the frame where the pointer stops mattering
    /// (e.g. leaves a hover-sensitive region) is still repainted exactly
    /// once before suppression begins — otherwise stale chrome (e.g. a hover
    /// tint the pointer just left) would linger until the next unrelated
    /// repaint (up to the ~500ms blink interval). `true` before the first
    /// `CursorMoved`, matching this trait method's conservative default.
    pointer_motion_needed_last: bool,
    /// Task 121 spike: set when a `CursorMoved` was suppressed; cleared when
    /// the next frame consumes it.
    ///
    /// Suppressed events are still handed to `on_window_event` (egui's pointer
    /// state must stay fresh), so they queue in egui's `RawInput.events` until
    /// the next `take_egui_input`. `InputState::wants_repaint_after` returns
    /// `Duration::ZERO` whenever that queue is non-empty, so egui re-arms a
    /// 16ms frame from *inside* the frame no matter how thoroughly we
    /// suppressed the input-side scheduling — a self-sustaining ~60fps loop.
    /// This flag lets the `RedrawRequested` arm recognise that specific case
    /// and substitute the app's own requested delay for egui's zero.
    suppressed_pointer_since_last_frame: bool,
}

impl WindowState {
    /// Release the egui-glow painter's GPU resources before this window's
    /// state is dropped.
    ///
    /// `egui_glow::Painter` owns OpenGL objects (program, textures, VBO/EBO)
    /// that must be freed with `destroy()` while the owning GL context is
    /// current; otherwise the painter's `Drop` impl logs a "you forgot to call
    /// `destroy()`" resource-leak warning. This runs on every window close
    /// (including the standalone settings window) and at event-loop exit.
    fn destroy_egui(&mut self) {
        if let Err(e) = self.gl.make_current() {
            // If the context can't be made current we still call destroy()
            // below — it is a no-op-safe GL teardown — but the GL calls may
            // not take effect. Log so the cause is visible.
            tracing::warn!("make_current failed during painter teardown: {e}");
        }
        self.egui.destroy_painter();
    }
}

/// Main application handler that owns the `App` and all window state.
struct Handler<A: App> {
    app: A,
    initial_config: Option<WindowConfig>,
    windows: HashMap<winit::window::WindowId, WindowState>,
    proxy: EventLoopProxy<UserEvent>,
    /// Scratch buffer for pending `WindowOp`s queued by `WindowHandle`.
    pending_ops: RefCell<Vec<WindowOp>>,
    /// Last-known geometry for each window, updated on Resized / Moved.
    ///
    /// Shared with `WindowHandle` via `&RefCell` so the `App` can query
    /// live geometry during its `update()` callback.
    geometry: RefCell<HashMap<WindowId, WindowGeometry>>,
}

impl<A: App> Handler<A> {
    fn create_window_from_config(&mut self, event_loop: &ActiveEventLoop, config: &WindowConfig) {
        let mut attrs = WindowAttributes::default().with_title(&config.title);

        if let Some((w, h)) = config.inner_size {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(w, h));
        }

        if let Some((x, y)) = config.position {
            attrs = attrs.with_position(winit::dpi::LogicalPosition::new(x, y));
        }

        if config.transparent {
            attrs = attrs.with_transparent(true);
        }

        if let Some(ref icon_data) = config.icon {
            if let Ok(icon) = winit::window::Icon::from_rgba(
                icon_data.rgba.clone(),
                icon_data.width,
                icon_data.height,
            ) {
                attrs = attrs.with_window_icon(Some(icon));
            } else {
                error!("Failed to create window icon from RGBA data");
            }
        }

        #[cfg(target_os = "linux")]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            if let Some(ref app_id) = config.app_id {
                attrs = attrs.with_name(app_id, "");
            }
        }

        let window = match event_loop.create_window(attrs) {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create window: {e}");
                return;
            }
        };

        let gl = match GlState::new(event_loop, &window, config.transparent) {
            Ok(gl) => gl,
            Err(e) => {
                error!("Failed to create GL context: {e}");
                return;
            }
        };

        let egui = match EguiState::new(&window, &gl) {
            Ok(egui) => egui,
            Err(e) => {
                error!("Failed to create egui state: {e}");
                return;
            }
        };

        let winit_id = window.id();
        let window_id = WindowId(winit_id);
        let phys = window.inner_size();

        let state = WindowState {
            window,
            gl,
            egui,
            repaint_at: Some(Instant::now()),
            chrome_input_pending: false,
            last_cursor_pos: None,
            chrome_drag_pressed_count: 0,
            pointer_motion_needed_last: true,
            suppressed_pointer_since_last_frame: false,
        };

        self.windows.insert(winit_id, state);

        // Seed geometry from the freshly-created window so the app can query
        // it even before the first Resized / Moved event arrives.  We store
        // geometry in logical pixels for consistency with `WindowConfig`.
        let scale = self.windows[&winit_id].window.scale_factor();
        let logical_size: winit::dpi::LogicalSize<f64> = phys.to_logical(scale);
        let outer_pos_logical = self.windows[&winit_id]
            .window
            .outer_position()
            .ok()
            .map(|p| {
                let lp: winit::dpi::LogicalPosition<f64> = p.to_logical(scale);
                (logical_coord_to_i32(lp.x), logical_coord_to_i32(lp.y))
            });
        self.geometry.borrow_mut().insert(
            window_id,
            WindowGeometry {
                size: Some((
                    logical_dim_to_u32(logical_size.width),
                    logical_dim_to_u32(logical_size.height),
                )),
                position: outer_pos_logical,
            },
        );

        // Track the first window as the primary clipboard source.

        // Request an immediate redraw so the first frame renders as soon as
        // the event loop is ready.  `repaint_at` alone only fires in
        // `about_to_wait`, which may not schedule a second frame quickly
        // enough for the terminal to display the initial shell prompt.
        self.windows[&winit_id].window.request_redraw();

        let handle = WindowHandle {
            proxy: &self.proxy,
            pending_ops: &self.pending_ops,
            geometry: &self.geometry,
        };
        self.app.on_window_created(
            window_id,
            &self.windows[&winit_id].egui.ctx,
            &handle,
            (phys.width, phys.height),
        );

        // Process any ops queued during on_window_created.
        self.process_pending_ops(event_loop);

        debug!("Window created: {winit_id:?}");
    }

    fn close_window(&mut self, winit_id: winit::window::WindowId) {
        if let Some(mut state) = self.windows.remove(&winit_id) {
            // Free the egui-glow painter's GPU resources while this window's
            // GL context is still current, then drop in dependency order:
            // egui (painter) -> gl context -> window.
            state.destroy_egui();
            drop(state.egui);
            drop(state.gl);
            drop(state.window);
            self.geometry.borrow_mut().remove(&WindowId(winit_id));
            debug!("Window closed: {winit_id:?}");
        }
    }

    /// Compute the earliest repaint deadline across all windows.
    fn earliest_deadline(&self) -> Option<Instant> {
        self.windows
            .values()
            .filter_map(|state| state.repaint_at)
            .min()
    }

    /// Drain and execute all pending `WindowOp`s queued by `WindowHandle`.
    fn process_pending_ops(&mut self, event_loop: &ActiveEventLoop) {
        let ops: Vec<WindowOp> = self.pending_ops.borrow_mut().drain(..).collect();
        for op in ops {
            match op {
                WindowOp::CreateWindow(config) => {
                    self.create_window_from_config(event_loop, &config);
                }
                WindowOp::CloseWindow(id) => {
                    self.close_window(id.0);
                    if self.windows.is_empty() {
                        event_loop.exit();
                    }
                }
                WindowOp::RequestRepaint(id) => {
                    if let Some(state) = self.windows.get_mut(&id.0) {
                        state.repaint_at = Some(Instant::now());
                        state.window.request_redraw();
                    }
                }
                WindowOp::RequestRepaintAfter(id, delay) => {
                    if let Some(state) = self.windows.get_mut(&id.0) {
                        // Same 16ms floor as every other repaint-scheduling
                        // path (issue #439). This same-thread `WindowOp` path
                        // has no sub-16ms caller today, but flooring it keeps
                        // the "no scheduling path can drive the GUI past
                        // ~60fps" invariant true for every caller, present and
                        // future.
                        let deadline = Instant::now() + clamp_repaint_delay(delay);
                        state.repaint_at = Some(
                            state
                                .repaint_at
                                .map_or(deadline, |existing| existing.min(deadline)),
                        );
                    }
                }
                WindowOp::SetTitle(id, title) => {
                    if let Some(state) = self.windows.get(&id.0) {
                        state.window.set_title(&title);
                    }
                }
                WindowOp::SetVisible(id, visible) => {
                    if let Some(state) = self.windows.get(&id.0) {
                        state.window.set_visible(visible);
                    }
                }
                WindowOp::SetMinimized(id, minimized) => {
                    if let Some(state) = self.windows.get(&id.0) {
                        state.window.set_minimized(minimized);
                    }
                }
                WindowOp::FocusWindow(id) => {
                    if let Some(state) = self.windows.get(&id.0) {
                        state.window.focus_window();
                    }
                }
            }
        }
    }

    /// Update `ControlFlow` based on the nearest repaint deadline.
    fn update_control_flow(&self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.earliest_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl<A: App> ApplicationHandler<UserEvent> for Handler<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        info!("Event loop resumed");
        if let Some(config) = self.initial_config.take() {
            self.create_window_from_config(event_loop, &config);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        winit_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Mouse-motion events arrive at 100+ Hz on macOS.  We pass them to
        // egui for pointer position tracking but only schedule a repaint if
        // egui actually wants one (e.g. menu hover highlight).  We skip the
        // full window_event path to avoid unnecessary work.
        //
        // #436.8: pointer events (`CursorMoved`/`MouseInput`/`MouseWheel`)
        // handle the chrome-input gate here, region-tested against
        // `App::is_chrome_interactive_at`, instead of the general path's
        // `is_unconditional_chrome_input` — see that function's doc for why
        // pointer events are excluded from it.
        if matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        ) {
            if let Some(state) = self.windows.get_mut(&winit_id) {
                let response = state.egui.on_window_event(&state.window, &event);
                // Task 121 spike: whether THIS pointer event should schedule
                // a repaint. Defaults to egui-winit's own opinion (always
                // `true` for all five event kinds handled in this fast path
                // — see `should_schedule_cursor_moved`'s doc) and is
                // narrowed ONLY inside the `CursorMoved` arm below, the sole
                // event kind `App::pointer_motion_needs_repaint` gates
                // (`MouseInput`/`MouseWheel`/`CursorEntered`/`CursorLeft`
                // are discrete, rare, and stay unconditional — deliberate
                // scope limit).
                let mut schedule_repaint = response.repaint;
                match event {
                    WindowEvent::CursorMoved { position, .. } => {
                        let scale = state.window.scale_factor();
                        state.last_cursor_pos = physical_to_logical_pos(position, scale);
                        let is_over_chrome = state
                            .last_cursor_pos
                            .map(|pos| self.app.is_chrome_interactive_at(WindowId(winit_id), pos));
                        state.chrome_input_pending |= should_force_chrome_full_for_pointer(
                            is_over_chrome,
                            state.chrome_drag_pressed_count > 0,
                        );

                        // Task 121 spike: independent of the chrome-input
                        // gate above (which drives `ChromeMode`), decide
                        // whether this pointer-motion event needs a repaint
                        // AT ALL. `last_cursor_pos.is_none()` (position
                        // could not be converted to logical points — a
                        // non-finite/non-positive scale factor) is
                        // conservative: treated as needed.
                        let app_says_needed = state.last_cursor_pos.is_none_or(|pos| {
                            self.app
                                .pointer_motion_needs_repaint(WindowId(winit_id), pos)
                        });
                        schedule_repaint = should_schedule_cursor_moved(
                            state.chrome_drag_pressed_count > 0,
                            state.pointer_motion_needed_last,
                            app_says_needed,
                        );

                        #[cfg(feature = "frame-profiling")]
                        if schedule_repaint {
                            state.egui.record_pointer_frame_scheduled();
                        } else {
                            state.egui.record_pointer_frame_suppressed();
                        }

                        state.pointer_motion_needed_last = app_says_needed;
                    }
                    WindowEvent::CursorEntered { .. } => {
                        // Unconditional (matches `is_unconditional_chrome_input`).
                        state.chrome_input_pending = true;
                    }
                    WindowEvent::CursorLeft { .. } => {
                        // The pointer is gone — a stale position must not be
                        // used to wrongly classify a later event.
                        state.last_cursor_pos = None;
                        // Task 121 spike: reset the edge-detect latch too —
                        // a stale "not needed" from before the pointer left
                        // must not suppress the first `CursorMoved` after it
                        // re-enters (at a possibly unrelated position).
                        state.pointer_motion_needed_last = true;
                        // Unconditional (matches `is_unconditional_chrome_input`).
                        state.chrome_input_pending = true;
                    }
                    WindowEvent::MouseInput {
                        state: btn_state, ..
                    } => {
                        let is_over_chrome = state
                            .last_cursor_pos
                            .map(|pos| self.app.is_chrome_interactive_at(WindowId(winit_id), pos));
                        // Decide using the PRE-update latch first, so the
                        // release event that ends a chrome-border drag still
                        // forces `Full` before the latch drops to 0.
                        state.chrome_input_pending |= should_force_chrome_full_for_pointer(
                            is_over_chrome,
                            state.chrome_drag_pressed_count > 0,
                        );
                        state.chrome_drag_pressed_count = update_chrome_drag_latch(
                            state.chrome_drag_pressed_count,
                            btn_state,
                            is_over_chrome,
                        );
                    }
                    WindowEvent::MouseWheel { .. } => {
                        let is_over_chrome = state
                            .last_cursor_pos
                            .map(|pos| self.app.is_chrome_interactive_at(WindowId(winit_id), pos));
                        state.chrome_input_pending |= should_force_chrome_full_for_pointer(
                            is_over_chrome,
                            state.chrome_drag_pressed_count > 0,
                        );
                    }
                    _ => unreachable!(
                        "matches! guard above restricts event to the five pointer variants"
                    ),
                }

                // Task 121 spike: `suppressed_pointer_since_last_frame` must
                // mean "the ONLY input since the last frame was pointer
                // motion we classified as needing no frame". So ANY event
                // that genuinely schedules — a click, a wheel tick, a
                // pointer enter/leave, or a `CursorMoved` the app said
                // mattered — invalidates that premise and clears the flag.
                //
                // Without this, a suppressed motion followed by a click
                // would leave the flag set when the click's own frame runs,
                // and the `RedrawRequested` override would then substitute
                // the app's long delay for the immediate follow-up frame the
                // click legitimately needed.
                state.suppressed_pointer_since_last_frame = !schedule_repaint;

                if schedule_repaint {
                    let deadline = Instant::now() + MIN_REPAINT_INTERVAL;
                    state.repaint_at = Some(
                        state
                            .repaint_at
                            .map_or(deadline, |existing| existing.min(deadline)),
                    );
                }
            }
            self.update_control_flow(event_loop);
            return;
        }

        // Intercept paste shortcuts before egui-winit can consume them.
        //
        // On Wayland, egui-winit creates a per-window smithay-clipboard instance.
        // Only the first instance receives wl_data_device events, so clipboard
        // reads silently fail on child windows — and egui-winit still swallows
        // the keypress.  We fix this by reading clipboard from whichever window
        // has a working clipboard and injecting Event::Paste into the target.
        if let winit::event::WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    ref logical_key,
                    state: winit::event::ElementState::Pressed,
                    ..
                },
            ..
        } = event
        {
            let is_paste = self.windows.get(&winit_id).is_some_and(|state| {
                let mods = state.egui.modifiers();
                matches!(
                    logical_key,
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::Paste)
                ) || (mods.command
                    && matches!(
                        logical_key,
                        winit::keyboard::Key::Character(c)
                            if c.as_str().eq_ignore_ascii_case("v")
                    ))
            });

            if is_paste {
                let text = self
                    .windows
                    .values_mut()
                    .find_map(|state| state.egui.clipboard_text());

                if let Some(text) = text {
                    let text = text.replace("\r\n", "\n");
                    if !text.is_empty()
                        && let Some(state) = self.windows.get_mut(&winit_id)
                    {
                        state.egui.inject_paste(text);
                        state.repaint_at = Some(Instant::now());
                        // Real input scheduled a frame — see the pointer fast
                        // path for why this invalidates the suppression premise.
                        state.suppressed_pointer_since_last_frame = false;
                        // A keyboard event, and it just mutated pane content
                        // via a paste — a potential-chrome-input (#436.4b §3.2).
                        state.chrome_input_pending = true;
                        // Don't pass to egui-winit — it would produce a
                        // duplicate paste on windows where its clipboard works.
                        self.update_control_flow(event_loop);
                        return;
                    }
                }
            }
        }

        // Intercept the narrow set of physical keys egui 0.35 cannot deliver
        // (Task 114: keypad operators/digits, media, print/pause/menu)
        // BEFORE egui-winit sees them, and route them to
        // `App::on_raw_key_event` instead. Every other key falls through to
        // egui unchanged — this must stay narrow (see `is_blocked_key`).
        if let winit::event::WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key: winit::keyboard::PhysicalKey::Code(key_code),
                    state: key_state,
                    repeat,
                    ..
                },
            ..
        } = event
            && is_blocked_key(key_code)
        {
            if let Some(state) = self.windows.get_mut(&winit_id) {
                let mods = state.egui.modifiers();
                let raw_event = RawKeyEvent {
                    key_code,
                    pressed: key_state == winit::event::ElementState::Pressed,
                    repeat,
                };
                let raw_mods = RawKeyMods {
                    shift: mods.shift,
                    ctrl: mods.ctrl,
                    alt: mods.alt,
                    super_key: mods.command,
                };
                self.app
                    .on_raw_key_event(WindowId(winit_id), raw_event, raw_mods);
                state.repaint_at = Some(Instant::now());
                // Real input scheduled a frame — see the pointer fast path for
                // why this invalidates the suppression premise.
                state.suppressed_pointer_since_last_frame = false;
                // A keyboard event, routed straight to the app — a
                // potential-chrome-input (#436.4b §3.2).
                state.chrome_input_pending = true;
            }
            // Don't pass to egui-winit — this key has no egui `Key` variant
            // and would otherwise be silently dropped.
            self.update_control_flow(event_loop);
            return;
        }

        // Pass to egui first
        let egui_consumed = if let Some(state) = self.windows.get_mut(&winit_id) {
            let response = state.egui.on_window_event(&state.window, &event);

            // #436.4b §3.2: any non-pointer window input event that could
            // plausibly affect chrome (or that egui itself says caused a
            // repaint, covering event kinds `is_unconditional_chrome_input`
            // doesn't enumerate) forces `ChromeMode::Full` for the frame this
            // event is delivered in. Pointer events never reach this arm —
            // they're handled, region-tested, in the fast path above (#436.8).
            //
            // `RedrawRequested` is carved out of the `repaint` half of this
            // decision — see `should_set_chrome_input_pending`'s doc for why
            // this is required (egui-winit reports `repaint: true`
            // unconditionally for it, and it's the event that drives every
            // frame, so treating it as chrome input here would permanently
            // disqualify `ChromeMode::Replay`).
            if should_set_chrome_input_pending(&event, response.repaint) {
                state.chrome_input_pending = true;
            }

            // Real input that schedules a frame invalidates the suppression
            // premise — see the pointer fast path for why.
            //
            // `RedrawRequested` MUST be excluded, for the same structural
            // reason it is excluded from the chrome-input gate
            // (`should_set_chrome_input_pending`): this general path runs
            // BEFORE the `match` below, and `egui-winit` reports
            // `repaint: true` for `RedrawRequested`. Clearing the flag here
            // would therefore wipe it moments before the `RedrawRequested`
            // arm `mem::take`s it, so the override could never fire at all.
            if !matches!(event, WindowEvent::RedrawRequested) {
                state.suppressed_pointer_since_last_frame = false;
            }

            if response.repaint {
                state.repaint_at = Some(Instant::now());
            }

            response.consumed
        } else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                let window_id = WindowId(winit_id);
                if self.app.on_close_requested(window_id) {
                    self.close_window(winit_id);
                    if self.windows.is_empty() {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::Focused(false) => {
                // #436.8 safety net: a chrome-border drag interrupted by
                // focus loss (e.g. alt-tab mid-drag) must not leave the latch
                // stuck non-zero, which would force every subsequent pointer
                // event `Full` forever.
                if let Some(state) = self.windows.get_mut(&winit_id) {
                    state.chrome_drag_pressed_count = 0;
                }
            }
            WindowEvent::Resized(size) => {
                let scale = self
                    .windows
                    .get(&winit_id)
                    .map_or(1.0, |s| s.window.scale_factor());
                if let Some(state) = self.windows.get_mut(&winit_id)
                    && let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    if let Err(e) = state.gl.make_current() {
                        error!("make_current failed during resize for {winit_id:?}: {e}");
                    } else {
                        state.gl.resize(w, h);
                    }
                    state.repaint_at = Some(Instant::now());
                    state.window.request_redraw();
                }
                // Track geometry in logical pixels (matches WindowConfig).
                let logical: winit::dpi::LogicalSize<f64> = size.to_logical(scale);
                let mut geom = self.geometry.borrow_mut();
                let entry = geom.entry(WindowId(winit_id)).or_default();
                entry.size = Some((
                    logical_dim_to_u32(logical.width),
                    logical_dim_to_u32(logical.height),
                ));
            }
            WindowEvent::Moved(pos) => {
                let scale = self
                    .windows
                    .get(&winit_id)
                    .map_or(1.0, |s| s.window.scale_factor());
                let logical: winit::dpi::LogicalPosition<f64> = pos.to_logical(scale);
                let mut geom = self.geometry.borrow_mut();
                let entry = geom.entry(WindowId(winit_id)).or_default();
                entry.position = Some((
                    logical_coord_to_i32(logical.x),
                    logical_coord_to_i32(logical.y),
                ));
            }
            WindowEvent::RedrawRequested => {
                // Split borrows by destructuring
                let Self {
                    app,
                    windows,
                    proxy,
                    pending_ops,
                    geometry,
                    ..
                } = self;
                let Some(state) = windows.get_mut(&winit_id) else {
                    return;
                };
                let window_id = WindowId(winit_id);
                let clear_color = app.clear_color(window_id);

                let handle = WindowHandle {
                    proxy,
                    pending_ops,
                    geometry,
                };

                // Ensure this window's GL context is current before rendering.
                if let Err(e) = state.gl.make_current() {
                    error!("make_current failed for {winit_id:?}: {e}");
                    return;
                }

                // Collect raw input, let app hook modify it, then run the frame.
                let mut raw_input = state.egui.take_egui_input(&state.window);
                app.raw_input_hook(window_id, &mut raw_input);

                // Fetch the partial-present flag up front (immutable borrow of
                // `app`, released before the `ui_fn` mutable borrow) so the
                // windowing layer can publish the authoritative decision into
                // it mid-frame without a second `&mut app` borrow.
                let present_flag = app.present_partial_flag(window_id);

                // Drain this frame's #436.4b §3.2 chrome-input gate,
                // resetting it so a later frame with no new input events
                // never inherits a stale `true`.
                let chrome_input_this_frame = std::mem::take(&mut state.chrome_input_pending);

                let frame_output = state.egui.run_frame(
                    &state.window,
                    &state.gl,
                    clear_color,
                    raw_input,
                    present_flag.as_ref(),
                    chrome_input_this_frame,
                    |ctx, gl, chrome_mode| {
                        app.update(window_id, ctx, gl, &handle, chrome_mode);
                        FrameSignals {
                            frame_damage: app.take_frame_damage(window_id),
                            band_range: app.take_terminal_band_range(window_id),
                            chrome_damage: app.take_chrome_damage(window_id),
                            terminal_requested_delay: app.take_terminal_requested_delay(window_id),
                        }
                    },
                );

                // Process egui viewport commands.
                let mut should_close = false;
                let mut paste_requested = false;
                for cmd in frame_output.commands {
                    process_viewport_command(
                        &state.window,
                        cmd,
                        &mut should_close,
                        &mut paste_requested,
                    );
                }

                // Honour `ViewportCommand::RequestPaste` (e.g. the terminal
                // right-click "Paste" menu entry). egui-winit does not action
                // this command itself in our custom integration — unlike
                // eframe, which we replaced — so we read the clipboard and
                // inject `Event::Paste` here, mirroring the keyboard paste
                // interceptor. The cross-window `find_map` works around the
                // Wayland per-window clipboard quirk documented there.
                if paste_requested {
                    let text = windows
                        .values_mut()
                        .find_map(|state| state.egui.clipboard_text());
                    if let Some(text) = text {
                        let text = text.replace("\r\n", "\n");
                        if !text.is_empty()
                            && let Some(state) = windows.get_mut(&winit_id)
                        {
                            state.egui.inject_paste(text);
                            state.repaint_at = Some(Instant::now());
                        }
                    }
                }

                let Some(state) = windows.get_mut(&winit_id) else {
                    self.update_control_flow(event_loop);
                    return;
                };
                state.repaint_at = None;

                // Honour egui's repaint_delay but clamp to the shared
                // MIN_REPAINT_INTERVAL floor to prevent unbounded rendering
                // from zero-delay requests (hover state, tooltip updates).
                // This ensures layout-settling frames still fire while keeping
                // idle CPU near zero.
                // Task 121 SPIKE: egui re-arms the frame schedule from inside
                // the frame. `InputState::wants_repaint_after` returns
                // `Duration::ZERO` whenever `!events.is_empty()`, and the
                // pointer events we deliberately suppressed are still in that
                // queue (they must be — egui's pointer state has to stay
                // fresh). So egui asks for an immediate repaint, we floor it
                // to 16ms, and the loop sustains ~60fps even though every one
                // of those events was classified as needing no frame.
                //
                // When the ONLY thing that happened since the last frame was
                // suppressed pointer motion, substitute the app's own
                // requested delay (e.g. the 500ms cursor blink) for egui's
                // zero. This overrides egui's judgement, but on OUR evidence
                // (we classified the events) rather than by pattern-matching
                // egui internals.
                // NOTE for future refactors: this `mem::take` deliberately sits
                // AFTER the `make_current` and window-lookup early-returns
                // above. If we bail before this point the flag is simply left
                // set for the next successful pass, which is correct — do not
                // move it earlier without re-checking that.
                let suppressed_only =
                    std::mem::take(&mut state.suppressed_pointer_since_last_frame);
                let effective_delay = effective_repaint_delay(
                    suppressed_only,
                    frame_output.repaint_delay,
                    frame_output.app_requested_delay,
                );

                // Subtask 121.13 + residual-gap fix (maintainer decision):
                // `run_frame` (inside `state.egui`) already stashed the RAW
                // `frame_output.repaint_delay` for next frame's chrome-settle
                // check. Overwrite it here with `effective_chrome_gate_delay`
                // — NOT `effective_delay`/`effective_repaint_delay` above,
                // which answers a different question (what to schedule) and
                // therefore must stay bounded even when the app requested
                // nothing. The gate value must NOT be bounded in that case:
                // see `effective_chrome_gate_delay`'s doc for why the two
                // diverge in exactly one case, and
                // `EguiState::stash_effective_repaint_delay`'s doc for why
                // this is a deliberate second write over `run_frame`'s stash.
                //
                // Unconditional rather than "only when it differs": the two
                // gate-delay branches are equal to `repaint_delay` whenever
                // `suppressed_only` was false, so an unconditional write is a
                // no-op on every non-suppressed frame and therefore
                // behaviourally identical to a guarded write, just without
                // the extra comparison.
                //
                // Must run on every path that reaches this point (no early
                // return follows it in this arm) — the field would go stale
                // on any suppressed-pointer frame that skipped it.
                let effective_gate_delay = effective_chrome_gate_delay(
                    suppressed_only,
                    frame_output.repaint_delay,
                    frame_output.app_requested_delay,
                );
                state
                    .egui
                    .stash_effective_repaint_delay(effective_gate_delay);

                if effective_delay < std::time::Duration::from_hours(1) {
                    let deadline = Instant::now() + clamp_repaint_delay(effective_delay);
                    state.repaint_at = Some(deadline);
                }

                // Process any ops queued during update.
                self.process_pending_ops(event_loop);

                if should_close {
                    // Route through `on_close_requested` so the app can run
                    // its normal shutdown/save logic.  `ViewportCommand::Close`
                    // (e.g. from a PTY exit triggering a last-pane close) used
                    // to bypass this hook, which meant `auto_save_session`
                    // and other cleanup never ran when the terminal exited
                    // itself.
                    let window_id = WindowId(winit_id);
                    if self.app.on_close_requested(window_id) {
                        self.close_window(winit_id);
                        if self.windows.is_empty() {
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {
                if !egui_consumed {
                    // App could handle other events here in the future
                }
            }
        }

        self.update_control_flow(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::RequestRepaint(id) => {
                if let Some(state) = self.windows.get_mut(&id.0) {
                    // Schedule rather than calling request_redraw() directly,
                    // throttled to the shared MIN_REPAINT_INTERVAL floor (same
                    // as window_event / RequestRepaintAfter) to prevent
                    // unbounded rendering.
                    let min_deadline = Instant::now() + MIN_REPAINT_INTERVAL;
                    state.repaint_at = Some(
                        state
                            .repaint_at
                            .map_or(min_deadline, |existing| existing.min(min_deadline)),
                    );
                }
            }
            UserEvent::RequestRepaintAfter(id, delay) => {
                if let Some(state) = self.windows.get_mut(&id.0) {
                    // Clamp the caller-supplied delay to the same 16ms floor
                    // the in-frame `frame_output.repaint_delay` path and the
                    // `RequestRepaint` arm enforce (issue #439). Without this
                    // floor, a cross-thread caller (the PTY consumer thread's
                    // `post_event`) could request an 8ms wake that `min`s
                    // below any 16ms-floored deadline already scheduled,
                    // defeating the floor entirely and letting a bursty PTY
                    // output stream drive the GUI past 60fps. Flooring here
                    // closes the loophole for every caller of this path, not
                    // just the one we know about today.
                    let deadline = Instant::now() + clamp_repaint_delay(delay);
                    state.repaint_at = Some(
                        state
                            .repaint_at
                            .map_or(deadline, |existing| existing.min(deadline)),
                    );
                }
            }
        }

        // Ensure the event loop wakes at the earliest deadline so timer-based
        // repaints actually fire.  Without this, the loop may stay in `Wait`
        // indefinitely on platforms where `about_to_wait` is not called after
        // `user_event` (observed on macOS).
        self.update_control_flow(event_loop);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Check if any windows need repaint based on timers.
        // Clear `repaint_at` immediately so spurious wake-ups between
        // now and the actual `RedrawRequested` delivery don't re-fire
        // `request_redraw()` on every pass through `about_to_wait`.
        let now = Instant::now();
        let ids: Vec<winit::window::WindowId> = self.windows.keys().copied().collect();
        for winit_id in ids {
            if let Some(state) = self.windows.get_mut(&winit_id)
                && let Some(deadline) = state.repaint_at
                && deadline <= now
            {
                state.repaint_at = None;
                state.window.request_redraw();
            }
        }

        self.update_control_flow(event_loop);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // The event loop is shutting down irreversibly. Any windows still in
        // the map (e.g. when `exit()` was called without routing every window
        // through `close_window`) must have their egui-glow painters torn down
        // so they don't leak / warn on drop.
        let ids: Vec<winit::window::WindowId> = self.windows.keys().copied().collect();
        for winit_id in ids {
            if let Some(state) = self.windows.get_mut(&winit_id) {
                state.destroy_egui();
            }
        }
        self.windows.clear();
        debug!("Event loop exiting; all painters destroyed");
    }
}

/// Side-effect flags a viewport command raises that the caller must action
/// after the per-frame command loop completes (rather than inline, because
/// each needs `&mut` access to state the command loop has borrowed).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ViewportCommandFlags {
    /// The window should be closed (via `on_close_requested`).
    should_close: bool,
    /// A clipboard paste was requested (e.g. the right-click "Paste" menu).
    paste_requested: bool,
}

/// Classify a viewport command into the deferred side-effect flags it raises.
///
/// Pure and window-free so it is unit-testable without a live event loop. The
/// window-affecting commands (title, size, focus, …) return the default
/// (no flags) and are actioned by [`process_viewport_command`].
const fn viewport_command_flags(cmd: &egui::ViewportCommand) -> ViewportCommandFlags {
    match cmd {
        egui::ViewportCommand::Close => ViewportCommandFlags {
            should_close: true,
            paste_requested: false,
        },
        egui::ViewportCommand::RequestPaste => ViewportCommandFlags {
            should_close: false,
            paste_requested: true,
        },
        _ => ViewportCommandFlags {
            should_close: false,
            paste_requested: false,
        },
    }
}

/// Process a single egui `ViewportCommand` by mapping it to the corresponding
/// winit `Window` API call.
///
/// Commands that require closing the window set `*should_close = true`; the
/// caller is responsible for actually closing the window after the frame
/// completes (to avoid mutating the window map during iteration).
///
/// `ViewportCommand::RequestPaste` sets `*paste_requested = true` instead of
/// being handled inline: clipboard reads need cross-window fallback on Wayland
/// (see the keyboard paste interceptor in [`Handler::window_event`]), which
/// requires `&mut` access to the whole window map. The caller injects the
/// paste after the command loop completes.
fn process_viewport_command(
    window: &Window,
    cmd: egui::ViewportCommand,
    should_close: &mut bool,
    paste_requested: &mut bool,
) {
    let flags = viewport_command_flags(&cmd);
    *should_close |= flags.should_close;
    *paste_requested |= flags.paste_requested;

    match cmd {
        // No-op inline:
        // - `Close` / `RequestPaste` are deferred via `viewport_command_flags`
        //   above and actioned by the caller after the command loop.
        // - `CancelClose`: close is synchronous via `on_close_requested`, so
        //   there is no queued deferred close to cancel.
        egui::ViewportCommand::Close
        | egui::ViewportCommand::RequestPaste
        | egui::ViewportCommand::CancelClose => {}
        egui::ViewportCommand::Title(title) => {
            window.set_title(&title);
        }
        egui::ViewportCommand::Minimized(minimized) => {
            window.set_minimized(minimized);
        }
        egui::ViewportCommand::Maximized(maximized) => {
            window.set_maximized(maximized);
        }
        egui::ViewportCommand::Fullscreen(fullscreen) => {
            if fullscreen {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            } else {
                window.set_fullscreen(None);
            }
        }
        egui::ViewportCommand::InnerSize(size) => {
            let _ = window.request_inner_size(winit::dpi::LogicalSize::new(size.x, size.y));
        }
        egui::ViewportCommand::OuterPosition(pos) => {
            window.set_outer_position(winit::dpi::LogicalPosition::new(pos.x, pos.y));
        }
        egui::ViewportCommand::Visible(visible) => {
            window.set_visible(visible);
        }
        egui::ViewportCommand::RequestUserAttention(kind) => {
            let winit_kind = match kind {
                egui::UserAttentionType::Informational => {
                    Some(winit::window::UserAttentionType::Informational)
                }
                egui::UserAttentionType::Critical => {
                    Some(winit::window::UserAttentionType::Critical)
                }
                egui::UserAttentionType::Reset => None,
            };
            window.request_user_attention(winit_kind);
        }
        egui::ViewportCommand::Focus => {
            window.focus_window();
        }
        // Commands we don't handle yet — log and ignore.
        _ => {
            tracing::trace!("Unhandled viewport command: {cmd:?}");
        }
    }
}

/// Entry point — replaces the old `eframe::run_native()` call.
///
/// Creates the event loop, opens the initial window with the given config,
/// and runs the application until all windows are closed.
///
/// # Errors
///
/// Returns [`Error::EventLoopCreation`] if the winit event loop fails to
/// initialise or exits with an error.
#[allow(clippy::too_many_lines)]
pub fn run(config: WindowConfig, app: impl App + 'static) -> Result<(), Error> {
    let event_loop = EventLoop::with_user_event()
        .build()
        .map_err(|e| Error::EventLoopCreation(format!("{e}")))?;

    let proxy = event_loop.create_proxy();

    let mut handler = Handler {
        app,
        initial_config: Some(config),
        windows: HashMap::new(),
        proxy,
        pending_ops: RefCell::new(Vec::new()),
        geometry: RefCell::new(HashMap::new()),
    };

    event_loop
        .run_app(&mut handler)
        .map_err(|e| Error::EventLoopCreation(format!("event loop exited with error: {e}")))?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        App, MIN_REPAINT_INTERVAL, SUPPRESSED_POINTER_FALLBACK_DELAY, ViewportCommandFlags,
        WindowId, clamp_repaint_delay, effective_chrome_gate_delay, effective_repaint_delay,
        is_blocked_key, is_unconditional_chrome_input, logical_coord_to_i32, logical_dim_to_u32,
        physical_to_logical_pos, should_force_chrome_full_for_pointer,
        should_schedule_cursor_moved, should_set_chrome_input_pending, update_chrome_drag_latch,
        viewport_command_flags,
    };
    use crate::tests::DummyApp;
    use winit::event::{DeviceId, WindowEvent};
    use winit::keyboard::KeyCode;

    #[test]
    fn clamp_repaint_delay_raises_sub_floor_delays_to_the_floor() {
        // Issue #439: the PTY consumer thread previously requested an 8ms
        // repaint delay through the unclamped cross-thread path, bypassing the
        // 60fps floor and letting bursty output (btop, htop, vim, less) drive
        // the GUI past 60fps. Any sub-16ms request must now be raised.
        assert_eq!(
            clamp_repaint_delay(std::time::Duration::from_millis(8)),
            MIN_REPAINT_INTERVAL,
            "the historical 8ms bypass must be floored to 16ms"
        );
        assert_eq!(
            clamp_repaint_delay(std::time::Duration::ZERO),
            MIN_REPAINT_INTERVAL,
            "a zero-delay (immediate) request must still respect the floor"
        );
        assert_eq!(
            clamp_repaint_delay(std::time::Duration::from_millis(15)),
            MIN_REPAINT_INTERVAL,
            "just-below-floor must be raised"
        );
    }

    #[test]
    fn clamp_repaint_delay_passes_through_at_and_above_the_floor() {
        // Exactly the floor is unchanged.
        assert_eq!(
            clamp_repaint_delay(MIN_REPAINT_INTERVAL),
            MIN_REPAINT_INTERVAL
        );
        // Longer legitimate delays (cursor-blink ~500ms, toast-fade ~250ms)
        // must pass through untouched — the floor is a minimum, not a cap.
        for ms in [17u64, 50, 100, 250, 500, 1000] {
            let d = std::time::Duration::from_millis(ms);
            assert_eq!(
                clamp_repaint_delay(d),
                d,
                "delay {ms}ms >= floor must pass through unchanged"
            );
        }
    }

    #[test]
    fn request_paste_command_sets_only_paste_flag() {
        // Regression (PLANNING #1 / Task 106.1): the terminal right-click
        // "Paste" menu sends `ViewportCommand::RequestPaste`. Before the fix
        // this command fell through to the catch-all log-and-ignore arm, so
        // right-click paste silently did nothing.
        assert_eq!(
            viewport_command_flags(&egui::ViewportCommand::RequestPaste),
            ViewportCommandFlags {
                should_close: false,
                paste_requested: true,
            }
        );
    }

    #[test]
    fn close_command_sets_only_close_flag() {
        assert_eq!(
            viewport_command_flags(&egui::ViewportCommand::Close),
            ViewportCommandFlags {
                should_close: true,
                paste_requested: false,
            }
        );
    }

    #[test]
    fn window_affecting_commands_raise_no_flags() {
        // Title / focus / etc. are actioned inline against the winit window
        // and must not set the deferred flags.
        assert_eq!(
            viewport_command_flags(&egui::ViewportCommand::Title("x".to_owned())),
            ViewportCommandFlags::default()
        );
        assert_eq!(
            viewport_command_flags(&egui::ViewportCommand::Focus),
            ViewportCommandFlags::default()
        );
        assert_eq!(
            viewport_command_flags(&egui::ViewportCommand::CancelClose),
            ViewportCommandFlags::default()
        );
    }

    #[test]
    fn logical_dim_to_u32_clamps_non_positive_and_non_finite() {
        assert_eq!(logical_dim_to_u32(0.0), 0);
        assert_eq!(logical_dim_to_u32(-1.0), 0);
        assert_eq!(logical_dim_to_u32(f64::NAN), 0);
        assert_eq!(logical_dim_to_u32(f64::NEG_INFINITY), 0);
    }

    #[test]
    fn logical_dim_to_u32_ceils_positive_subpixel_to_one() {
        // Regression: `round()` maps 0.25 to 0, which would persist a
        // zero-size window.  `ceil()` guarantees any strictly positive
        // dimension becomes at least 1.
        assert_eq!(logical_dim_to_u32(0.25), 1);
        assert_eq!(logical_dim_to_u32(0.5), 1);
        assert_eq!(logical_dim_to_u32(0.99), 1);
        assert_eq!(logical_dim_to_u32(1.0), 1);
        assert_eq!(logical_dim_to_u32(1.01), 2);
        assert_eq!(logical_dim_to_u32(1280.0), 1280);
    }

    #[test]
    fn logical_dim_to_u32_saturates_on_overflow() {
        assert_eq!(logical_dim_to_u32(f64::INFINITY), 0);
        // A value well beyond u32::MAX saturates rather than panicking.
        assert_eq!(logical_dim_to_u32(1.0e20), u32::MAX);
    }

    #[test]
    fn logical_coord_to_i32_handles_edge_cases() {
        assert_eq!(logical_coord_to_i32(0.0), 0);
        assert_eq!(logical_coord_to_i32(f64::NAN), 0);
        assert_eq!(logical_coord_to_i32(100.4), 100);
        assert_eq!(logical_coord_to_i32(-100.4), -100);
        assert_eq!(logical_coord_to_i32(1.0e20), i32::MAX);
        assert_eq!(logical_coord_to_i32(-1.0e20), i32::MIN);
    }

    #[test]
    fn is_blocked_key_covers_the_egui_blocked_set() {
        // Task 114.5: a representative key from each blocked group.
        assert!(is_blocked_key(KeyCode::PrintScreen));
        assert!(is_blocked_key(KeyCode::Pause));
        assert!(is_blocked_key(KeyCode::ContextMenu));
        assert!(is_blocked_key(KeyCode::NumpadEnter));
        assert!(is_blocked_key(KeyCode::NumpadDivide));
        assert!(is_blocked_key(KeyCode::NumpadMultiply));
        assert!(is_blocked_key(KeyCode::NumpadSubtract));
        assert!(is_blocked_key(KeyCode::NumpadAdd));
        assert!(is_blocked_key(KeyCode::NumpadEqual));
        assert!(is_blocked_key(KeyCode::NumpadComma));
        assert!(is_blocked_key(KeyCode::NumpadDecimal));
        assert!(is_blocked_key(KeyCode::NumpadStar));
        assert!(is_blocked_key(KeyCode::Numpad0));
        assert!(is_blocked_key(KeyCode::Numpad9));
        assert!(is_blocked_key(KeyCode::MediaPlayPause));
        assert!(is_blocked_key(KeyCode::MediaStop));
        assert!(is_blocked_key(KeyCode::MediaTrackNext));
        assert!(is_blocked_key(KeyCode::MediaTrackPrevious));
        assert!(is_blocked_key(KeyCode::AudioVolumeUp));
        assert!(is_blocked_key(KeyCode::AudioVolumeDown));
        assert!(is_blocked_key(KeyCode::AudioVolumeMute));
    }

    #[test]
    fn is_blocked_key_does_not_intercept_normal_keys() {
        // egui delivers these keys today; the intercept must stay narrow
        // and never swallow them. CapsLock/NumLock/ScrollLock are no longer
        // intercepted (Task 114 lock-state revert) — they fall through to
        // egui like any other normal-ish key (accepted gap: egui drops them).
        assert!(!is_blocked_key(KeyCode::CapsLock));
        assert!(!is_blocked_key(KeyCode::ScrollLock));
        assert!(!is_blocked_key(KeyCode::NumLock));
        assert!(!is_blocked_key(KeyCode::KeyA));
        assert!(!is_blocked_key(KeyCode::Digit1));
        assert!(!is_blocked_key(KeyCode::Enter));
        assert!(!is_blocked_key(KeyCode::ArrowUp));
        assert!(!is_blocked_key(KeyCode::ArrowDown));
        assert!(!is_blocked_key(KeyCode::ArrowLeft));
        assert!(!is_blocked_key(KeyCode::ArrowRight));
        assert!(!is_blocked_key(KeyCode::Space));
        assert!(!is_blocked_key(KeyCode::Escape));
        assert!(!is_blocked_key(KeyCode::AltRight));
    }

    /// #436.4b §3.2 / #436.8: a representative event from each
    /// unconditional-chrome-input category (keyboard, scroll-adjacent focus/
    /// IME/theme, `CursorEntered`/`CursorLeft`) forces `chrome_input_pending`
    /// via [`is_unconditional_chrome_input`].
    #[test]
    fn is_unconditional_chrome_input_covers_keyboard_ime_focus_theme_entered_left() {
        assert!(is_unconditional_chrome_input(&WindowEvent::CursorEntered {
            device_id: DeviceId::dummy(),
        }));
        assert!(is_unconditional_chrome_input(&WindowEvent::CursorLeft {
            device_id: DeviceId::dummy(),
        }));
        // `KeyEvent` has a private `platform_specific` field (no public
        // constructor), so `KeyboardInput` itself cannot be built outside
        // winit; it is exercised in a real frame instead via the paste-
        // interception / blocked-key tests elsewhere in this module, both
        // of which set `chrome_input_pending` directly at their early-return
        // sites (see `window_event`) rather than through this helper.
        assert!(is_unconditional_chrome_input(
            &WindowEvent::ModifiersChanged(winit::event::Modifiers::default(),)
        ));
        assert!(is_unconditional_chrome_input(&WindowEvent::Focused(true)));
        // An OS dark/light switch rebuilds egui chrome visuals synchronously
        // (#436.6): it must force `ChromeMode::Full`.
        assert!(is_unconditional_chrome_input(&WindowEvent::ThemeChanged(
            winit::window::Theme::Dark,
        )));
    }

    /// #436.8: pointer motion/click/scroll events are region-tested instead
    /// of unconditional — they must NOT be classified as unconditional
    /// chrome input any more, or region-testing would never actually apply
    /// (the unconditional check runs first in the general path).
    #[test]
    fn is_unconditional_chrome_input_excludes_pointer_events() {
        assert!(!is_unconditional_chrome_input(&WindowEvent::CursorMoved {
            device_id: DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(0.0, 0.0),
        }));
        assert!(!is_unconditional_chrome_input(&WindowEvent::MouseInput {
            device_id: DeviceId::dummy(),
            state: winit::event::ElementState::Pressed,
            button: winit::event::MouseButton::Left,
        }));
        assert!(!is_unconditional_chrome_input(&WindowEvent::MouseWheel {
            device_id: DeviceId::dummy(),
            delta: winit::event::MouseScrollDelta::LineDelta(0.0, 1.0),
            phase: winit::event::TouchPhase::Moved,
        }));
    }

    /// Events unrelated to chrome-affecting input do NOT set the gate —
    /// otherwise every frame would be forced `Full` and REPLAY would never
    /// fire.
    #[test]
    fn is_unconditional_chrome_input_excludes_unrelated_events() {
        assert!(!is_unconditional_chrome_input(
            &WindowEvent::RedrawRequested
        ));
        assert!(!is_unconditional_chrome_input(&WindowEvent::CloseRequested));
        assert!(!is_unconditional_chrome_input(&WindowEvent::Destroyed));
        assert!(!is_unconditional_chrome_input(
            &WindowEvent::HoveredFileCancelled
        ));
        assert!(!is_unconditional_chrome_input(&WindowEvent::Occluded(true)));
    }

    /// The bug (0/360 `Replay` frames at idle): `RedrawRequested` drives
    /// every frame, and egui-winit 0.35.0 reports `repaint: true` for it
    /// unconditionally, so before the fix `is_unconditional_chrome_input(event)
    /// || repaint` was always `true` on the exact event whose arm reads the
    /// gate back a moment later — permanently disqualifying `Replay`.
    /// `should_set_chrome_input_pending` must return `false` for
    /// `RedrawRequested` regardless of the `repaint` value egui-winit
    /// reports.
    #[test]
    fn should_set_chrome_input_pending_excludes_redraw_requested_regardless_of_repaint() {
        assert!(!should_set_chrome_input_pending(
            &WindowEvent::RedrawRequested,
            true
        ));
        assert!(!should_set_chrome_input_pending(
            &WindowEvent::RedrawRequested,
            false
        ));
    }

    /// A representative `is_unconditional_chrome_input` event (keyboard
    /// modifiers) must still set the gate through this wrapper, with or
    /// without `repaint` — the carve-out is specific to `RedrawRequested`,
    /// not a general weakening of the gate.
    #[test]
    fn should_set_chrome_input_pending_covers_unconditional_chrome_input_events() {
        let event = WindowEvent::ModifiersChanged(winit::event::Modifiers::default());
        assert!(should_set_chrome_input_pending(&event, false));
        assert!(should_set_chrome_input_pending(&event, true));
    }

    /// Proves the `RedrawRequested` carve-out doesn't silently undermine the
    /// reason `response.repaint` is consulted at all — an event kind NOT in
    /// `is_unconditional_chrome_input`'s enumeration (e.g. `Occluded`) must
    /// still set the gate when egui-winit reports `repaint: true` for it. If
    /// this regressed to "always false unless enumerated", A12's
    /// completeness guarantee (`repaint` as a safety net for un-enumerated
    /// event kinds) would be silently lost.
    #[test]
    fn should_set_chrome_input_pending_still_honors_repaint_for_non_enumerated_events() {
        let event = WindowEvent::Occluded(true);
        assert!(!is_unconditional_chrome_input(&event));
        assert!(should_set_chrome_input_pending(&event, true));
        assert!(!should_set_chrome_input_pending(&event, false));
    }

    // ── #436.8 region-aware pointer gate: pure helpers ───────────────────

    #[test]
    fn should_force_chrome_full_for_pointer_latched_forces_true_regardless_of_position() {
        assert!(should_force_chrome_full_for_pointer(Some(false), true));
        assert!(should_force_chrome_full_for_pointer(None, true));
    }

    #[test]
    fn should_force_chrome_full_for_pointer_unlatched_unknown_position_is_conservative_true() {
        assert!(should_force_chrome_full_for_pointer(None, false));
    }

    #[test]
    fn should_force_chrome_full_for_pointer_unlatched_over_chrome_is_true() {
        assert!(should_force_chrome_full_for_pointer(Some(true), false));
    }

    #[test]
    fn should_force_chrome_full_for_pointer_unlatched_over_terminal_is_false() {
        assert!(!should_force_chrome_full_for_pointer(Some(false), false));
    }

    #[test]
    fn update_chrome_drag_latch_press_over_chrome_increments() {
        assert_eq!(
            update_chrome_drag_latch(0, winit::event::ElementState::Pressed, Some(true)),
            1
        );
    }

    #[test]
    fn update_chrome_drag_latch_press_off_chrome_is_unchanged() {
        assert_eq!(
            update_chrome_drag_latch(0, winit::event::ElementState::Pressed, Some(false)),
            0
        );
    }

    #[test]
    fn update_chrome_drag_latch_press_unknown_position_is_conservative_increment() {
        assert_eq!(
            update_chrome_drag_latch(0, winit::event::ElementState::Pressed, None),
            1
        );
    }

    #[test]
    fn update_chrome_drag_latch_release_decrements() {
        assert_eq!(
            update_chrome_drag_latch(1, winit::event::ElementState::Released, Some(true)),
            0
        );
    }

    #[test]
    fn update_chrome_drag_latch_release_at_zero_saturates() {
        assert_eq!(
            update_chrome_drag_latch(0, winit::event::ElementState::Released, Some(true)),
            0
        );
    }

    // ── Task 121 spike: `should_schedule_cursor_moved` ───────────────────

    #[test]
    fn should_schedule_cursor_moved_latched_forces_true_regardless_of_app_opinion() {
        // A chrome-border drag in progress must keep repainting even if the
        // app says this position no longer needs one, and even if the
        // previous event also said "not needed".
        assert!(should_schedule_cursor_moved(true, false, false));
        assert!(should_schedule_cursor_moved(true, true, true));
    }

    #[test]
    fn should_schedule_cursor_moved_steady_needed_schedules() {
        // Both this event and the last agree a repaint is needed (e.g.
        // hovering over chrome, or an active selection drag) -> schedule.
        assert!(should_schedule_cursor_moved(false, true, true));
    }

    #[test]
    fn should_schedule_cursor_moved_steady_not_needed_suppresses() {
        // Two consecutive "not needed" events (steady motion over static
        // terminal content) -> suppress. This is the headline Task 121 case.
        assert!(!should_schedule_cursor_moved(false, false, false));
    }

    #[test]
    fn should_schedule_cursor_moved_needed_to_not_needed_transition_still_schedules_once() {
        // Edge detect: the LAST event needed a repaint, THIS event does not
        // (e.g. the pointer just left a hover-sensitive region) -> this
        // transition frame still schedules, so stale chrome (a hover tint)
        // is repainted one final time before suppression begins.
        assert!(should_schedule_cursor_moved(false, true, false));
    }

    #[test]
    fn should_schedule_cursor_moved_not_needed_to_needed_transition_schedules() {
        // The reverse transition schedules via `current_needed` alone.
        assert!(should_schedule_cursor_moved(false, false, true));
    }

    // ── 122.6: `CursorMoved` dispatch driven by a live `App`, not
    // hand-supplied booleans (`event_loop.rs:809-845`) ────────────────────
    //
    // The tests above pin `should_schedule_cursor_moved` and
    // `should_force_chrome_full_for_pointer` in isolation, fed literal
    // `true`/`false`. These tests instead call the SAME functions in the
    // SAME order the `CursorMoved` arm of `Handler::window_event` does, fed
    // by `App::pointer_motion_needs_repaint` / `App::is_chrome_interactive_at`'s
    // REAL answers from a live `App` (`DummyApp`, configured per 122.6) —
    // pinning the app-to-dispatch wiring itself, not just the pure helpers.
    //
    // `Handler::window_event` cannot be called directly from a unit test:
    // `WindowState` (`event_loop.rs:457-507`) holds a real
    // `winit::window::Window` plus a live GL context (`GlState`/
    // `EguiState`), neither constructible without an actual display/GL
    // driver, so a `Handler<A>` cannot exist headlessly. This is as close
    // to the real dispatch path as a unit test can get without one.

    #[test]
    fn cursor_moved_dispatch_conservative_app_schedules_at_steady_state() {
        // The trait-default (conservative) app answers "needed" for every
        // position, so dispatch — fed that REAL answer, not a literal
        // `true` — schedules every event, matching pre-Task-121 "every
        // pointer motion repaints" behavior.
        let app = DummyApp::default();
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let current_needed = app.pointer_motion_needs_repaint(window_id, pos);
        let previous_needed = true; // `WindowState::pointer_motion_needed_last`'s initial value
        assert!(should_schedule_cursor_moved(
            false, // no chrome-border drag in progress
            previous_needed,
            current_needed,
        ));
    }

    #[test]
    fn cursor_moved_dispatch_suppressing_app_suppresses_at_steady_state() {
        // A suppressing app answers "not needed"; once the edge-detect latch
        // (`previous_needed`) has also settled to `false`, dispatch fed that
        // REAL answer suppresses the repaint — the headline Task 121 case,
        // now proven against a live `App` instead of literal bools.
        let app = DummyApp {
            pointer_motion_needs_repaint: false,
            ..Default::default()
        };
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let current_needed = app.pointer_motion_needs_repaint(window_id, pos);
        assert!(
            !current_needed,
            "the configured app must actually answer false"
        );

        let previous_needed = false; // steady state after the one-time edge-detect frame
        assert!(!should_schedule_cursor_moved(
            false,
            previous_needed,
            current_needed
        ));
    }

    #[test]
    fn cursor_moved_dispatch_suppressing_app_still_schedules_the_transition_frame() {
        // Edge-detect: the FIRST event after a live app starts suppressing
        // still schedules once (`previous_needed` is still `true` from the
        // prior conservative answer), driven by the app's real transition.
        let app = DummyApp {
            pointer_motion_needs_repaint: false,
            ..Default::default()
        };
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let current_needed = app.pointer_motion_needs_repaint(window_id, pos);
        let previous_needed = true;
        assert!(should_schedule_cursor_moved(
            false,
            previous_needed,
            current_needed
        ));
    }

    #[test]
    fn cursor_moved_dispatch_chrome_drag_latch_overrides_a_suppressing_app() {
        // A chrome-border drag in progress must keep repainting even though
        // the app itself would suppress — proven with the app's real
        // (suppressing) answer, not a literal `false`.
        let app = DummyApp {
            pointer_motion_needs_repaint: false,
            ..Default::default()
        };
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let current_needed = app.pointer_motion_needs_repaint(window_id, pos);
        assert!(should_schedule_cursor_moved(true, false, current_needed));
    }

    #[test]
    fn cursor_moved_dispatch_chrome_interactive_app_forces_full_regardless_of_latch() {
        // The trait-default (conservative) app's `is_chrome_interactive_at`
        // forces `ChromeMode::Full` via `should_force_chrome_full_for_pointer`
        // even with no drag latch held — mirrors `event_loop.rs:813-819`.
        let app = DummyApp::default();
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let is_over_chrome = Some(app.is_chrome_interactive_at(window_id, pos));
        assert!(should_force_chrome_full_for_pointer(is_over_chrome, false));
    }

    #[test]
    fn cursor_moved_dispatch_chrome_non_interactive_app_does_not_force_full_without_latch() {
        // A live app that answers "not chrome-interactive" at this position,
        // with no drag latch held, does NOT force `ChromeMode::Full` —
        // mirrors terminal-content-only pointer motion.
        let app = DummyApp {
            chrome_interactive: false,
            ..Default::default()
        };
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let is_over_chrome = Some(app.is_chrome_interactive_at(window_id, pos));
        assert!(!should_force_chrome_full_for_pointer(is_over_chrome, false));
    }

    #[test]
    fn cursor_moved_dispatch_chrome_non_interactive_app_still_forced_full_by_drag_latch() {
        // The drag latch overrides even a non-interactive answer from the
        // app — a drag that moves off the sensor mid-drag must not lose
        // `ChromeMode::Full`.
        let app = DummyApp {
            chrome_interactive: false,
            ..Default::default()
        };
        let window_id = WindowId(winit::window::WindowId::dummy());
        let pos = egui::Pos2::new(4.0, 4.0);

        let is_over_chrome = Some(app.is_chrome_interactive_at(window_id, pos));
        assert!(should_force_chrome_full_for_pointer(is_over_chrome, true));
    }

    // ── Task 121 spike: `effective_repaint_delay` (liveness-critical) ────
    //
    // This is the substitution that breaks egui's self-sustaining ~60fps
    // re-arm loop. It is the highest-risk logic in the spike, so every branch
    // is pinned here — especially the fallback, because getting that wrong
    // stalls the window instead of merely wasting a frame.

    const MS16: std::time::Duration = std::time::Duration::from_millis(16);
    const MS500: std::time::Duration = std::time::Duration::from_millis(500);

    #[test]
    fn effective_repaint_delay_substitutes_app_delay_when_suppressed_and_egui_wants_immediate() {
        // The headline case: egui asked for an immediate repaint purely because
        // suppressed pointer events sit in its queue; the app only wants a
        // 500ms blink wake. Without the substitution this is a 16ms-floored
        // frame, i.e. ~60fps for zero visible change.
        assert_eq!(
            effective_repaint_delay(true, std::time::Duration::ZERO, Some(MS500)),
            MS500
        );
    }

    #[test]
    fn effective_repaint_delay_falls_back_to_bounded_delay_when_app_wants_nothing() {
        // Cursor hidden via DECTCEM (btop/vim): the app requests no delay at
        // all. Must NOT become `Duration::MAX`/no-schedule — that stalls the
        // window until an unrelated event arrives.
        let got = effective_repaint_delay(true, std::time::Duration::ZERO, None);
        assert_eq!(got, SUPPRESSED_POINTER_FALLBACK_DELAY);
        assert!(
            got < std::time::Duration::from_secs(1),
            "fallback must be a bounded wake, got {got:?}"
        );
    }

    #[test]
    fn effective_repaint_delay_fallback_is_never_faster_than_the_blink_period() {
        // 121.12 perversity pin: the fallback must be `>= 500ms`, the
        // cursor-blink period `app_impl.rs` schedules per-pane. Before this
        // subtask the fallback was 250ms, so turning the cursor blink OFF
        // (no `app_requested_delay` at all -> this fallback) scheduled MORE
        // frames (4fps) than leaving the blink ON (2fps at the 500ms floor).
        // A future edit that shrinks this constant back below the blink
        // period would silently reintroduce that "blink-off is worse than
        // blink-on" perversity — this test exists so that regresses loudly.
        assert!(
            SUPPRESSED_POINTER_FALLBACK_DELAY >= std::time::Duration::from_millis(500),
            "fallback ({SUPPRESSED_POINTER_FALLBACK_DELAY:?}) must be >= the 500ms blink period"
        );
    }

    #[test]
    fn effective_repaint_delay_passes_egui_delay_through_when_not_suppressed() {
        // Nothing was suppressed, so egui's request is authoritative even when
        // it is zero — a real interaction needs the immediate frame.
        assert_eq!(
            effective_repaint_delay(false, std::time::Duration::ZERO, Some(MS500)),
            std::time::Duration::ZERO
        );
        assert_eq!(effective_repaint_delay(false, MS16, Some(MS500)), MS16);
    }

    #[test]
    fn effective_repaint_delay_never_overrides_a_nonzero_egui_request() {
        // Suppression only ever reinterprets a ZERO delay. A non-zero egui
        // request is a genuine animation/timer cadence and must survive, or we
        // would stutter egui-driven animations.
        assert_eq!(effective_repaint_delay(true, MS16, Some(MS500)), MS16);
        assert_eq!(effective_repaint_delay(true, MS16, None), MS16);
    }

    #[test]
    fn effective_repaint_delay_preserves_max_when_not_suppressed() {
        // `Duration::MAX` means "egui needs no further frame". It must pass
        // through untouched so the caller's `< 1 hour` check still parks the
        // window correctly.
        assert_eq!(
            effective_repaint_delay(false, std::time::Duration::MAX, None),
            std::time::Duration::MAX
        );
    }

    // ── Residual-gap fix (maintainer decision): `effective_chrome_gate_delay` ──
    //
    // Mirrors the `effective_repaint_delay` tests above for its sibling: same
    // branches, same inputs, but the `None`-substitution branch answers
    // `Duration::MAX` instead of the bounded fallback, because this function
    // answers "did anything want a repaint?" rather than "what do we
    // schedule?". See the divergence-pinning test at the end of this block.

    #[test]
    fn effective_chrome_gate_delay_substitutes_app_delay_when_suppressed_and_egui_wants_immediate()
    {
        // Identical to `effective_repaint_delay`'s headline case: the app's
        // own request substitutes for egui's queue-artifact zero.
        assert_eq!(
            effective_chrome_gate_delay(true, std::time::Duration::ZERO, Some(MS500)),
            MS500
        );
    }

    #[test]
    fn effective_chrome_gate_delay_becomes_max_when_app_wants_nothing() {
        // THIS is where the two functions diverge: `effective_repaint_delay`
        // must stay bounded here (liveness), but the gate delay must become
        // `Duration::MAX` — the absence of any app request IS the proof that
        // nothing wanted a repaint, so the synthetic liveness poll interval
        // must not masquerade as evidence of one.
        assert_eq!(
            effective_chrome_gate_delay(true, std::time::Duration::ZERO, None),
            std::time::Duration::MAX
        );
    }

    #[test]
    fn effective_chrome_gate_delay_passes_egui_delay_through_when_not_suppressed() {
        assert_eq!(
            effective_chrome_gate_delay(false, std::time::Duration::ZERO, Some(MS500)),
            std::time::Duration::ZERO
        );
        assert_eq!(effective_chrome_gate_delay(false, MS16, Some(MS500)), MS16);
    }

    #[test]
    fn effective_chrome_gate_delay_never_overrides_a_nonzero_egui_request() {
        assert_eq!(effective_chrome_gate_delay(true, MS16, Some(MS500)), MS16);
        assert_eq!(effective_chrome_gate_delay(true, MS16, None), MS16);
    }

    #[test]
    fn effective_chrome_gate_delay_preserves_max_when_not_suppressed() {
        assert_eq!(
            effective_chrome_gate_delay(false, std::time::Duration::MAX, None),
            std::time::Duration::MAX
        );
    }

    #[test]
    fn effective_repaint_delay_and_effective_chrome_gate_delay_diverge_only_when_app_requested_nothing()
     {
        // Pin the divergence explicitly: same `suppressed_only`/`repaint_delay`
        // inputs, `app_requested_delay: None`, and the two functions now
        // disagree on purpose. `effective_repaint_delay` returns the bounded
        // liveness fallback (what we schedule); `effective_chrome_gate_delay`
        // returns `Duration::MAX` (nothing wanted a repaint).
        let scheduled = effective_repaint_delay(true, std::time::Duration::ZERO, None);
        let gated = effective_chrome_gate_delay(true, std::time::Duration::ZERO, None);

        assert_eq!(scheduled, SUPPRESSED_POINTER_FALLBACK_DELAY);
        assert_eq!(gated, std::time::Duration::MAX);
        assert_ne!(
            scheduled, gated,
            "the two questions ('what to schedule' vs 'did anything want a \
             repaint') must diverge in this exact case, or the residual gap \
             this fix closes has regressed"
        );
    }

    // ── #436.8 drag-latch multi-press/multi-release SEQUENCES (436.9 follow-up) ──
    //
    // The single-call tests above pin each transition in isolation. These
    // chain `update_chrome_drag_latch` across realistic event sequences,
    // asserting the latch value AND, at each step, what
    // `should_force_chrome_full_for_pointer` decides given the pre-update
    // latch (the ordering `event_loop.rs` uses: decide with the PRE-update
    // latch, then update — so a release ending a chrome drag still forces
    // Full before the latch drops).

    /// Press ON chrome -> pointer moves OFF chrome mid-drag -> release.
    /// The mandate-critical case: the whole drag must force Full (latch keeps
    /// it Full even while the pointer is over terminal content), and the latch
    /// must land back at exactly 0 after release.
    #[test]
    fn drag_latch_sequence_press_on_chrome_move_off_release_stays_full_throughout() {
        use winit::event::ElementState::{Pressed, Released};

        let mut latch = 0u32;

        // Press over chrome. Decide with pre-update latch (0) but is_over_chrome
        // = Some(true) -> Full. Then latch -> 1.
        assert!(should_force_chrome_full_for_pointer(Some(true), latch > 0));
        latch = update_chrome_drag_latch(latch, Pressed, Some(true));
        assert_eq!(latch, 1);

        // Pointer moves OFF chrome (over terminal content) while dragging.
        // is_over_chrome = Some(false), but latch (1) > 0 -> still Full.
        assert!(should_force_chrome_full_for_pointer(Some(false), latch > 0));
        // (motion doesn't touch the latch)
        assert_eq!(latch, 1);

        // Release (delivered while pointer is off chrome). Decide with the
        // PRE-update latch (1 > 0) -> Full, THEN decrement.
        assert!(should_force_chrome_full_for_pointer(Some(false), latch > 0));
        latch = update_chrome_drag_latch(latch, Released, Some(false));
        assert_eq!(latch, 0);

        // Post-drag: pointer still over terminal content, latch 0 -> NOT Full.
        assert!(!should_force_chrome_full_for_pointer(
            Some(false),
            latch > 0
        ));
    }

    /// Nested/rapid presses (e.g. a second button pressed before the first
    /// releases) must balance out to exactly 0 and force Full throughout.
    #[test]
    fn drag_latch_sequence_nested_presses_balance_to_zero() {
        use winit::event::ElementState::{Pressed, Released};

        let mut latch = 0u32;
        latch = update_chrome_drag_latch(latch, Pressed, Some(true));
        latch = update_chrome_drag_latch(latch, Pressed, Some(true));
        assert_eq!(latch, 2);
        // Both buttons held -> Full.
        assert!(should_force_chrome_full_for_pointer(Some(false), latch > 0));

        latch = update_chrome_drag_latch(latch, Released, Some(false));
        assert_eq!(latch, 1);
        // One button still held -> still Full.
        assert!(should_force_chrome_full_for_pointer(Some(false), latch > 0));

        latch = update_chrome_drag_latch(latch, Released, Some(false));
        assert_eq!(latch, 0);
        assert!(!should_force_chrome_full_for_pointer(
            Some(false),
            latch > 0
        ));
    }

    /// A press that STARTS over terminal content does not latch, so subsequent
    /// motion over terminal content stays REPLAY (the "helps normal mouse use"
    /// mandate piece: text-selection drags must not force chrome Full).
    #[test]
    fn drag_latch_sequence_press_on_terminal_never_latches() {
        use winit::event::ElementState::{Pressed, Released};

        let mut latch = 0u32;
        // Press over terminal content -> no latch.
        latch = update_chrome_drag_latch(latch, Pressed, Some(false));
        assert_eq!(latch, 0);
        // Dragging (text selection) over terminal content -> NOT Full.
        assert!(!should_force_chrome_full_for_pointer(
            Some(false),
            latch > 0
        ));
        // Release over terminal content -> still 0, still not Full.
        latch = update_chrome_drag_latch(latch, Released, Some(false));
        assert_eq!(latch, 0);
        assert!(!should_force_chrome_full_for_pointer(
            Some(false),
            latch > 0
        ));
    }

    /// An unbalanced release (delivered without a matching press, e.g. to a
    /// different window) must saturate at 0, never underflow to `u32::MAX` (which
    /// would force Full forever).
    #[test]
    fn drag_latch_sequence_unbalanced_release_saturates_not_underflows() {
        use winit::event::ElementState::{Pressed, Released};

        let mut latch = 0u32;
        // Spurious release first.
        latch = update_chrome_drag_latch(latch, Released, Some(true));
        assert_eq!(latch, 0);
        // A subsequent real press still latches correctly (not offset by the
        // spurious release).
        latch = update_chrome_drag_latch(latch, Pressed, Some(true));
        assert_eq!(latch, 1);
        latch = update_chrome_drag_latch(latch, Released, Some(true));
        assert_eq!(latch, 0);
    }

    // ── #436.8 physical -> logical pointer position conversion ──────────

    #[test]
    fn physical_to_logical_pos_normal_scale_halves_at_scale_two() {
        let pos = winit::dpi::PhysicalPosition::new(100.0, 50.0);
        let logical = physical_to_logical_pos(pos, 2.0).expect("scale 2.0 is valid");
        assert!((logical.x - 50.0).abs() < f32::EPSILON);
        assert!((logical.y - 25.0).abs() < f32::EPSILON);
    }

    #[test]
    fn physical_to_logical_pos_scale_one_is_identity() {
        let pos = winit::dpi::PhysicalPosition::new(123.0, 45.0);
        let logical = physical_to_logical_pos(pos, 1.0).expect("scale 1.0 is valid");
        assert!((logical.x - 123.0).abs() < f32::EPSILON);
        assert!((logical.y - 45.0).abs() < f32::EPSILON);
    }

    #[test]
    fn physical_to_logical_pos_invalid_scale_is_none() {
        let pos = winit::dpi::PhysicalPosition::new(10.0, 10.0);
        assert!(physical_to_logical_pos(pos, 0.0).is_none());
        assert!(physical_to_logical_pos(pos, -1.0).is_none());
        assert!(physical_to_logical_pos(pos, f64::NAN).is_none());
        assert!(physical_to_logical_pos(pos, f64::INFINITY).is_none());
    }
}
