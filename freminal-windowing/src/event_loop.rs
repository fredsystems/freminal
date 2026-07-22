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
                    }
                    WindowEvent::CursorEntered { .. } => {
                        // Unconditional (matches `is_unconditional_chrome_input`).
                        state.chrome_input_pending = true;
                    }
                    WindowEvent::CursorLeft { .. } => {
                        // The pointer is gone — a stale position must not be
                        // used to wrongly classify a later event.
                        state.last_cursor_pos = None;
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
                if response.repaint {
                    let deadline = Instant::now() + std::time::Duration::from_millis(16);
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
            if is_unconditional_chrome_input(&event) || response.repaint {
                state.chrome_input_pending = true;
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

                // Honour egui's repaint_delay but clamp to a minimum of 16ms
                // to prevent unbounded rendering from zero-delay requests
                // (hover state, tooltip updates).  This ensures layout-settling
                // frames still fire while keeping idle CPU near zero.
                if frame_output.repaint_delay < std::time::Duration::from_hours(1) {
                    let min_delay = std::time::Duration::from_millis(16);
                    let delay = frame_output.repaint_delay.max(min_delay);
                    let deadline = Instant::now() + delay;
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
                    // same throttle as window_event to prevent unbounded rendering.
                    let min_deadline = Instant::now() + std::time::Duration::from_millis(16);
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
        MIN_REPAINT_INTERVAL, ViewportCommandFlags, clamp_repaint_delay, is_blocked_key,
        is_unconditional_chrome_input, logical_coord_to_i32, logical_dim_to_u32,
        physical_to_logical_pos, should_force_chrome_full_for_pointer, update_chrome_drag_latch,
        viewport_command_flags,
    };
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
