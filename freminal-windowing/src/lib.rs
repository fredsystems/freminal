//! `freminal-windowing` — Platform windowing layer for Freminal.
//!
//! Provides winit + glutin + egui integration, encapsulating the event loop,
//! GL context management, and egui rendering. The `freminal` binary crate
//! implements the [`App`] trait to receive per-window update callbacks.

#![deny(clippy::unwrap_used, clippy::expect_used)]

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

/// Configuration for creating a new window.
pub struct WindowConfig {
    /// Window title.
    pub title: String,
    /// Initial inner size in logical pixels `(width, height)`.
    pub inner_size: Option<(u32, u32)>,
    /// Whether the window background should be transparent.
    pub transparent: bool,
    /// Window icon.
    pub icon: Option<egui::IconData>,
    /// Wayland app_id / X11 WM_CLASS.
    pub app_id: Option<String>,
}

/// The application trait that `freminal` implements.
pub trait App {
    /// Called once per window per frame, only when a redraw is needed.
    ///
    /// `handle` allows queuing window operations (title, close, repaint, etc.)
    /// that are executed after this callback returns.
    fn update(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        gl: &glow::Context,
        handle: &WindowHandle<'_>,
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

    /// Hook to modify raw input before egui processes it.
    ///
    /// Default implementation does nothing.
    fn raw_input_hook(&mut self, _window_id: WindowId, _raw_input: &mut egui::RawInput) {}
}

/// Handle for requesting window operations from the [`App`].
///
/// Passed by reference during event loop callbacks. Operations are queued
/// and executed after the current callback returns.
pub struct WindowHandle<'a> {
    proxy: &'a winit::event_loop::EventLoopProxy<UserEvent>,
    pending_ops: &'a std::cell::RefCell<Vec<WindowOp>>,
}

impl<'a> WindowHandle<'a> {
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
    pub fn event_loop_proxy(&self) -> RepaintProxy {
        RepaintProxy {
            proxy: self.proxy.clone(),
        }
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
}
