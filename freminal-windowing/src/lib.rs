// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! `freminal-windowing` — Platform windowing layer for Freminal.
//!
//! Provides winit + glutin + egui integration, encapsulating the event loop,
//! GL context management, and egui rendering. The `freminal` binary crate
//! implements the [`App`] trait to receive per-window update callbacks.

#![deny(
    clippy::pedantic,
    clippy::cargo,
    clippy::nursery,
    clippy::style,
    clippy::correctness,
    clippy::all,
    clippy::suspicious,
    clippy::complexity,
    clippy::perf,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
#![allow(clippy::multiple_crate_versions)] // Allow multiple versions from transitive dependencies
#![allow(clippy::cargo_common_metadata)] // Metadata is inherited from workspace]

pub mod error;

mod egui_integration;
mod event_loop;
mod gl_context;

pub use error::Error;
pub use event_loop::run;

use std::time::Duration;

/// Opaque window identifier (wraps `winit::window::WindowId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(winit::window::WindowId);

/// A rectangle in **physical framebuffer pixels**, origin at the
/// **bottom-left** of the surface (OpenGL / EGL convention).
///
/// Used to describe the damaged (changed) region of a frame for
/// partial-present and scissored-clear optimizations. The bottom-left
/// origin matches both `glScissor` and `eglSwapBuffersWithDamageKHR`, so
/// no coordinate flip is needed between the damage rect and either GL call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    /// X of the lower-left corner, in physical pixels.
    pub x: i32,
    /// Y of the lower-left corner, in physical pixels (bottom-left origin).
    pub y: i32,
    /// Width in physical pixels.
    pub width: i32,
    /// Height in physical pixels.
    pub height: i32,
}

/// Describes how much of a rendered frame actually changed, so the
/// windowing layer can decide whether it may skip the full-framebuffer
/// clear and present only the changed region.
///
/// The default is [`FrameDamage::Full`]: the whole surface changed and
/// must be cleared, redrawn, and presented. This is the conservative,
/// always-correct behavior — an app that does not opt in gets exactly the
/// pre-optimization path.
///
/// [`FrameDamage::Partial`] is a *hint*, honored only when the platform can
/// prove the previous frame's contents are still in the back buffer (via
/// `buffer_age() == 1`). When that proof is unavailable (non-EGL backends,
/// a rotated/aged buffer, a resize) the windowing layer falls back to a
/// full frame regardless.
#[derive(Debug, Clone, Default)]
pub enum FrameDamage {
    /// The entire surface changed. Clear + full redraw + full present.
    #[default]
    Full,
    /// Only the listed rectangles changed since the previous frame. The
    /// caller guarantees every pixel outside these rects is identical to
    /// the previous frame's, so — when the back buffer still holds that
    /// previous frame — the clear may be skipped and the present may be
    /// restricted to these rects.
    Partial(Vec<DamageRect>),
}

/// Whether the static chrome changed on the frame just rendered.
///
/// "Chrome" is the menu bar, tab bar, pane borders, broadcast label, and all
/// overlays (modals/toasts/tooltips/popups) — the #436 decision input for
/// whether a frame may REPLAY cached chrome primitives or must do a FULL
/// chrome rebuild.
///
/// The default is [`ChromeDamage::Changed`]: the conservative,
/// always-correct behavior. An app that does not opt in (or has not yet
/// wired up the #436 §3.3/§3.5 signals) always reports `Changed`, so a
/// consumer of this signal (436.4) never mistakenly REPLAYs stale chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeDamage {
    /// Chrome changed (or we can't prove it didn't) — the next frame must be FULL.
    #[default]
    Changed,
    /// Chrome provably did not change this frame — a REPLAY is permitted
    /// (subject to the other #436 gates in §3.4).
    Unchanged,
}

/// Whether the app should rebuild chrome widgets this frame or may reuse
/// cached chrome primitives from a previous frame (#436.4a scaffolding).
///
/// The default, [`ChromeMode::Full`], is the always-correct behavior: the
/// app re-records and re-tessellates every widget, exactly as it always
/// has. [`ChromeMode::Replay`] is not yet produced by `run_frame` — 436.4b
/// wires up the decision (chrome-damage + cache-validity gates) that flips
/// this to `Replay` for eligible frames. An app that has not been updated
/// to consult this parameter may simply ignore it; doing so is always safe
/// because the windowing layer only passes `Full` until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChromeMode {
    /// Rebuild chrome widgets this frame (re-record + re-tessellate).
    #[default]
    Full,
    /// Reuse cached chrome primitives; skip chrome-widget construction.
    /// (Not yet produced by `run_frame` — 436.4b flips this on.)
    Replay,
}

/// Per-frame signals drained from the [`App`] immediately after
/// [`App::update`] returns, consumed by the windowing layer to decide
/// paint-order splitting (#436.4a) and chrome replay (#436.4b).
#[derive(Debug, Clone)]
pub struct FrameSignals {
    /// How much of the frame changed (see [`FrameDamage`]).
    pub frame_damage: FrameDamage,
    /// The terminal band's shape-index range within this frame's
    /// `full_output.shapes` (#436.4a) — the contiguous `[start, end)` slice
    /// painted separately from chrome. Defaults to `0..0` (empty) for an
    /// app that has not wired up the band range; in that case the whole
    /// shape list is treated as "tail" and painted in a single
    /// `paint_primitives` call, byte-identical to the pre-#436.4a
    /// single-call path.
    pub band_range: std::ops::Range<usize>,
    /// Whether static chrome changed this frame (#436.3/#436.4b) — mirrors
    /// [`App::take_chrome_damage`]. Consulted, together with cache validity
    /// and the §3.1 settle gate, to decide next frame's [`ChromeMode`].
    pub chrome_damage: ChromeDamage,
    /// The delay the app itself requested via `ctx.request_repaint_after`
    /// during this frame's `update()` (#436.4b §3.1 amendment), if any.
    /// `None` when the app made no such request this frame. Compared
    /// against egui's own requested repaint delay (returned by `run_frame`)
    /// to distinguish "only our own blink/content scheduling wants a wake"
    /// from "something egui-internal (hover fade, menu animation, a
    /// focused `TextEdit`'s cursor blink) also wants one" — see
    /// `egui_integration::chrome_repaint_settled`.
    pub terminal_requested_delay: Option<std::time::Duration>,
}

/// Configuration for creating a new window.
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Initial inner size in logical pixels `(width, height)`.
    pub inner_size: Option<(u32, u32)>,
    /// Initial outer position in logical pixels `(x, y)`.
    ///
    /// Silently ignored on Wayland (compositor controls placement).
    pub position: Option<(i32, i32)>,
    /// Whether the window background should be transparent.
    pub transparent: bool,
    /// Window icon.
    pub icon: Option<egui::IconData>,
    /// Wayland `app_id` / X11 `WM_CLASS`.
    pub app_id: Option<String>,
}

/// The application trait that `freminal` implements.
pub trait App {
    /// Called once per window per frame, only when a redraw is needed.
    ///
    /// `handle` allows queuing window operations (title, close, repaint, etc.)
    /// that are executed after this callback returns.
    ///
    /// `chrome_mode` is the #436.4a scaffold for chrome-primitive replay: the
    /// app should skip chrome-widget construction (menu bar, tab bar, and
    /// other static chrome) when it is [`ChromeMode::Replay`], rebuilding
    /// only the terminal band. As of this subtask the windowing layer always
    /// passes [`ChromeMode::Full`] — 436.4b is what starts passing `Replay`
    /// — so an implementer may ignore this parameter for now.
    fn update(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        gl: &glow::Context,
        handle: &WindowHandle<'_>,
        chrome_mode: ChromeMode,
    );

    /// Called when a window is created.
    ///
    /// `inner_size` is the window's inner size in physical pixels at creation
    /// time. On X11 / tiling WMs this is typically the final tiled geometry.
    /// On Wayland it may be a placeholder until the first configure event.
    ///
    /// Use `handle` to obtain a [`RepaintProxy`] for cross-thread repaint
    /// requests (e.g. PTY consumer threads).
    fn on_window_created(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &WindowHandle<'_>,
        inner_size: (u32, u32),
    );

    /// Called when a window close is requested. Return `false` to cancel.
    fn on_close_requested(&mut self, window_id: WindowId) -> bool;

    /// GL clear color for the given window (supports transparency via alpha).
    fn clear_color(&self, window_id: WindowId) -> [f32; 4];

    /// Report how much of the frame just rendered in [`App::update`]
    /// actually changed, so the windowing layer can decide whether to skip
    /// the full-framebuffer clear and present only the damaged region.
    ///
    /// Called by the windowing layer **once per frame, immediately after
    /// [`App::update`] returns** for that window. The app should compute the
    /// answer during `update` and hand it back here (typically by draining a
    /// per-window value it set during the UI pass).
    ///
    /// The default returns [`FrameDamage::Full`] — the conservative,
    /// always-correct behavior. An app only needs to override this to opt
    /// into partial-present / skip-clear optimizations, and returning `Full`
    /// at any time is always safe.
    fn take_frame_damage(&mut self, _window_id: WindowId) -> FrameDamage {
        FrameDamage::Full
    }

    /// Drain the terminal band's shape-index range within this frame's
    /// `full_output.shapes` (#436.4a).
    ///
    /// The "terminal band" is the region of the frame — pane content, pane
    /// borders, and related overlays — that is rebuilt every frame and is
    /// tessellated/painted separately from the rest of the chrome (menu bar,
    /// tab bar, modals) by `run_frame`'s 3-way head/band/tail split. An app
    /// that wants to participate paints the band into the SAME egui layer as
    /// the rest of its chrome during `update()` (routing it into a dedicated
    /// layer instead trips egui's cross-layer hit-test "hidden" rule and can
    /// suppress hover/click/drag on band widgets), remembers the band's
    /// shape-index range within that layer's `PaintList`, and returns a
    /// clone of exactly that range here. This range is interpreted as an
    /// index range into `full_output.shapes` (the background layer drains
    /// first, so the two coincide) — supersedes the shape-cloning approach
    /// of the prior `take_terminal_band_shapes` (#436.2a), which is no
    /// longer needed once `run_frame` slices by range directly.
    ///
    /// Called by the windowing layer once per frame, after [`App::update`]
    /// returns for that window, mirroring [`App::take_frame_damage`].
    /// Reset-on-read: the app leaves `0..0` behind so a stale range can
    /// never be reused by a frame that didn't recompute it.
    ///
    /// The default returns `0..0` — the conservative, always-correct
    /// behavior for an app that does not participate in this optimization
    /// (or has not yet wired it up): `run_frame` treats an empty range as
    /// "everything is tail", painting the whole frame in one call, exactly
    /// as before #436.4a.
    fn take_terminal_band_range(&mut self, _window_id: WindowId) -> std::ops::Range<usize> {
        0..0
    }

    /// Drain the chrome-damage decision computed during `update()` for this
    /// window (#436). Called once per frame after `update()` returns, mirroring
    /// `take_frame_damage`. Default returns `ChromeDamage::Changed` (always safe:
    /// forces a FULL frame). Reset-on-read: the app leaves `Changed` behind so a
    /// stale `Unchanged` can never be reused by a frame that didn't recompute it.
    fn take_chrome_damage(&mut self, _window_id: WindowId) -> ChromeDamage {
        ChromeDamage::Changed
    }

    /// Drain the delay the app itself requested via `ctx.request_repaint_after`
    /// during this frame's `update()` (#436.4b §3.1 amendment), for this
    /// window. Called once per frame after `update()` returns, mirroring
    /// `take_frame_damage`/`take_chrome_damage`/`take_terminal_band_range`.
    ///
    /// The default returns `None` — the conservative, always-correct
    /// behavior: paired with `chrome_repaint_settled`'s `None` branch (which
    /// requires egui's own `repaint_delay` to be exactly `Duration::MAX`),
    /// an app that does not opt in never falsely reports "settled" while it
    /// is still driving its own blink/content repaint schedule under the
    /// hood some other way. Reset-on-read: the app leaves `None` behind so a
    /// stale delay can never be reused by a frame that didn't recompute it.
    fn take_terminal_requested_delay(&mut self, _window_id: WindowId) -> Option<Duration> {
        None
    }

    /// Shared flag through which the windowing layer publishes the
    /// **authoritative** partial-present decision for each frame.
    ///
    /// Returning `Some(flag)` opts the window into damage-aware presentation.
    /// Each frame, after the windowing layer resolves the partial-present gate
    /// (the app's [`FrameDamage`] *and* buffer-age *and* platform support) and
    /// **before** the paint callbacks execute, it stores the result into this
    /// flag with [`Ordering::Relaxed`](std::sync::atomic::Ordering::Relaxed):
    /// `true` when only the damaged region is being presented (the full clear
    /// was skipped), `false` for a normal full clear + present.
    ///
    /// This is the single source of truth. An app that scissors its own draws
    /// to the damage region must read this same flag inside its paint
    /// callbacks, so the scissor can never disagree with whether the clear was
    /// actually skipped (the black-cell hazard). Because the callbacks run on
    /// the same thread immediately after the store, `Relaxed` ordering is
    /// sufficient.
    ///
    /// The default returns `None`: the window is presented fully every frame
    /// and no flag is published.
    fn present_partial_flag(
        &self,
        _window_id: WindowId,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        None
    }

    /// Hook to modify raw input before egui processes it.
    ///
    /// Default implementation does nothing.
    fn raw_input_hook(&mut self, _window_id: WindowId, _raw_input: &mut egui::RawInput) {}

    /// Called for keyboard keys that egui cannot deliver (keypad
    /// operators/directional, media, print/pause/menu keys — see Task 114).
    /// Delivered BEFORE egui-winit and only for that narrow blocked set; all
    /// other keys reach the app through egui as usual.
    ///
    /// `event` carries the physical key code, press/release state, and
    /// auto-repeat flag. `mods` is the current chorded modifier state. The
    /// default implementation does nothing.
    fn on_raw_key_event(&mut self, _window_id: WindowId, _event: RawKeyEvent, _mods: RawKeyMods) {}
}

/// Chorded modifier state accompanying a raw key event delivered via
/// [`App::on_raw_key_event`].
///
/// These are the "held-for-decoration" modifiers: Shift/Ctrl/Alt/Super,
/// chorded at the time of the raw key event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Four independent chorded-modifier flags mirroring egui::Modifiers; a state machine would add noise, not clarity.
pub struct RawKeyMods {
    /// `Shift` held.
    pub shift: bool,
    /// `Ctrl` held.
    pub ctrl: bool,
    /// `Alt` held.
    pub alt: bool,
    /// `Super`/`Cmd`/`Windows` held.
    pub super_key: bool,
}

/// A raw keyboard event for a key egui cannot deliver (Task 114).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKeyEvent {
    /// The physical key code (winit's `KeyCode`).
    pub key_code: winit::keyboard::KeyCode,
    /// True on press/repeat, false on release.
    pub pressed: bool,
    /// True if this is an auto-repeat.
    pub repeat: bool,
}

/// Last-known geometry for a window, tracked by the windowing layer.
///
/// All values are in **logical pixels** to match the units used by
/// [`WindowConfig::inner_size`] and [`WindowConfig::position`] when
/// creating a window.  This lets the app roundtrip geometry across
/// sessions without having to know the current scale factor.
///
/// On Wayland, `position` is typically `None` because the compositor does
/// not expose window position.  Either field may be `None` on a freshly
/// created window that has not yet received its first configure event.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowGeometry {
    /// Inner (client-area) size in logical pixels: `(width, height)`.
    pub size: Option<(u32, u32)>,
    /// Outer (frame) position in logical pixels: `(x, y)`.
    pub position: Option<(i32, i32)>,
}

/// Handle for requesting window operations from the [`App`].
///
/// Passed by reference during event loop callbacks. Operations are queued
/// and executed after the current callback returns.
pub struct WindowHandle<'a> {
    proxy: &'a winit::event_loop::EventLoopProxy<UserEvent>,
    pending_ops: &'a std::cell::RefCell<Vec<WindowOp>>,
    geometry: &'a std::cell::RefCell<std::collections::HashMap<WindowId, WindowGeometry>>,
}

impl WindowHandle<'_> {
    /// Request that a new window be created.
    pub fn create_window(&self, config: WindowConfig) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::CreateWindow(config));
    }

    /// Request that a window be closed.
    pub fn close_window(&self, id: WindowId) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::CloseWindow(id));
    }

    /// Request an immediate repaint for a window.
    pub fn request_repaint(&self, id: WindowId) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::RequestRepaint(id));
    }

    /// Request a repaint after a delay.
    pub fn request_repaint_after(&self, id: WindowId, delay: Duration) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::RequestRepaintAfter(id, delay));
    }

    /// Set the title of a window.
    pub fn set_title(&self, id: WindowId, title: &str) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::SetTitle(id, title.to_owned()));
    }

    /// Set window visibility.
    pub fn set_visible(&self, id: WindowId, visible: bool) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::SetVisible(id, visible));
    }

    /// Set window minimized state.
    pub fn set_minimized(&self, id: WindowId, minimized: bool) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::SetMinimized(id, minimized));
    }

    /// Request that a window be focused (brought to front).
    pub fn focus_window(&self, id: WindowId) {
        self.pending_ops
            .borrow_mut()
            .push(WindowOp::FocusWindow(id));
    }

    /// Get a clone of the event loop proxy for cross-thread repaint requests.
    #[must_use]
    pub fn event_loop_proxy(&self) -> RepaintProxy {
        RepaintProxy {
            proxy: self.proxy.clone(),
        }
    }

    /// Query the last-known geometry for a window.
    ///
    /// Returns `None` if the window does not exist.  Either field inside the
    /// returned `WindowGeometry` may still be `None` if the compositor has
    /// not reported that value (e.g. `position` on Wayland).
    ///
    /// Geometry is tracked from winit `Resized` / `Moved` events and is
    /// always up to date with the window's current state at the time of
    /// this call.  This is more reliable than `ctx.input().viewport()`,
    /// which only populates `inner_rect` / `outer_rect` after the first
    /// such event arrives for the target window's egui context.
    #[must_use]
    pub fn window_geometry(&self, id: WindowId) -> Option<WindowGeometry> {
        self.geometry.borrow().get(&id).copied()
    }
}

/// Thread-safe handle for requesting repaints from non-GUI threads (e.g. PTY).
#[derive(Clone)]
pub struct RepaintProxy {
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

impl RepaintProxy {
    /// Request a repaint for the given window.
    pub fn request_repaint(&self, id: WindowId) {
        let _ = self.proxy.send_event(UserEvent::RequestRepaint(id));
    }

    /// Request a repaint after a delay for the given window.
    pub fn request_repaint_after(&self, id: WindowId, delay: Duration) {
        let _ = self
            .proxy
            .send_event(UserEvent::RequestRepaintAfter(id, delay));
    }
}

/// Internal user events sent via `EventLoopProxy`.
#[derive(Debug)]
pub(crate) enum UserEvent {
    RequestRepaint(WindowId),
    RequestRepaintAfter(WindowId, Duration),
}

/// Internal window operations queued by [`WindowHandle`].
///
/// Variant fields are consumed by the event loop's pending-ops processor.
pub(crate) enum WindowOp {
    CreateWindow(WindowConfig),
    CloseWindow(WindowId),
    RequestRepaint(WindowId),
    RequestRepaintAfter(WindowId, Duration),
    SetTitle(WindowId, String),
    SetVisible(WindowId, bool),
    SetMinimized(WindowId, bool),
    FocusWindow(WindowId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_config_defaults() {
        let config = WindowConfig {
            title: "test".to_owned(),
            inner_size: None,
            position: None,
            transparent: false,
            icon: None,
            app_id: None,
        };
        assert_eq!(config.title, "test");
        assert!(!config.transparent);
        assert!(config.inner_size.is_none());
        assert!(config.icon.is_none());
        assert!(config.app_id.is_none());
    }

    #[test]
    fn window_config_with_size() {
        let config = WindowConfig {
            title: "sized".to_owned(),
            inner_size: Some((800, 600)),
            position: None,
            transparent: true,
            icon: None,
            app_id: Some("test.app".to_owned()),
        };
        assert_eq!(config.inner_size, Some((800, 600)));
        assert!(config.transparent);
        assert_eq!(config.app_id.as_deref(), Some("test.app"));
    }

    #[test]
    fn error_display() {
        let err = Error::EventLoopCreation("test".to_owned());
        assert!(err.to_string().contains("test"));

        let err = Error::GlContextCreation("gl fail".to_owned());
        assert!(err.to_string().contains("gl fail"));

        let err = Error::SurfaceCreation("surface fail".to_owned());
        assert!(err.to_string().contains("surface fail"));

        let err = Error::WindowCreation("window fail".to_owned());
        assert!(err.to_string().contains("window fail"));

        let err = Error::MakeCurrent("make current fail".to_owned());
        assert!(err.to_string().contains("make current fail"));

        let err = Error::SwapBuffers("swap fail".to_owned());
        assert!(err.to_string().contains("swap fail"));
    }

    /// Minimal `App` implementer that relies entirely on default method
    /// bodies, used to pin the default behavior of
    /// `App::take_terminal_band_range` (#436.4a, superseding the removed
    /// #436.2a `take_terminal_band_shapes`) — mirroring the pre-existing
    /// `take_frame_damage` default — without constructing a full
    /// `freminal::gui::FreminalGui`, which is impractical headlessly (its
    /// windows are keyed by a real winit `WindowId`).
    struct DummyApp;

    impl App for DummyApp {
        fn update(
            &mut self,
            _window_id: WindowId,
            _ctx: &egui::Context,
            _gl: &glow::Context,
            _handle: &WindowHandle<'_>,
            _chrome_mode: ChromeMode,
        ) {
        }

        fn on_window_created(
            &mut self,
            _window_id: WindowId,
            _ctx: &egui::Context,
            _handle: &WindowHandle<'_>,
            _inner_size: (u32, u32),
        ) {
        }

        fn on_close_requested(&mut self, _window_id: WindowId) -> bool {
            true
        }

        fn clear_color(&self, _window_id: WindowId) -> [f32; 4] {
            [0.0, 0.0, 0.0, 1.0]
        }
    }

    #[test]
    fn take_terminal_band_range_default_is_empty_and_reset_on_read() {
        let mut app = DummyApp;
        let window_id = WindowId(winit::window::WindowId::dummy());

        // Empty (`0..0`) by default (no `update()` has run / default trait
        // body).
        assert_eq!(app.take_terminal_band_range(window_id), 0..0);

        // Reset-on-read: a second call still returns `0..0`, never a stale
        // or accumulated value.
        assert_eq!(app.take_terminal_band_range(window_id), 0..0);
    }

    /// Pins the default behavior of `App::take_chrome_damage` (#436.3) —
    /// mirroring the `take_frame_damage`/`take_terminal_band_range` default
    /// discipline above — using the same `DummyApp` (real `FreminalGui`
    /// windows are keyed by a real winit `WindowId`, impractical headlessly).
    #[test]
    fn take_chrome_damage_default_is_changed_and_reset_on_read() {
        let mut app = DummyApp;
        let window_id = WindowId(winit::window::WindowId::dummy());

        // Conservative default: `Changed` (no `update()` has run / default
        // trait body), so a consumer never mistakenly REPLAYs stale chrome.
        assert_eq!(app.take_chrome_damage(window_id), ChromeDamage::Changed);

        // Reset-on-read: a second call still returns `Changed`, never a
        // stale `Unchanged` a prior frame might have left behind.
        assert_eq!(app.take_chrome_damage(window_id), ChromeDamage::Changed);
    }

    /// Pins the default behavior of `App::take_terminal_requested_delay`
    /// (#436.4b) — mirroring the `take_chrome_damage` default discipline
    /// above, using the same `DummyApp`.
    #[test]
    fn take_terminal_requested_delay_default_is_none_and_reset_on_read() {
        let mut app = DummyApp;
        let window_id = WindowId(winit::window::WindowId::dummy());

        assert_eq!(app.take_terminal_requested_delay(window_id), None);
        // Reset-on-read: a second call still returns `None`.
        assert_eq!(app.take_terminal_requested_delay(window_id), None);
    }
}
