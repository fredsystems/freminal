//! winit event loop and `ApplicationHandler` implementation.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Instant;

use tracing::{debug, error, info};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes};

use crate::egui_integration::EguiState;
use crate::error::Error;
use crate::gl_context::GlState;
use crate::{App, UserEvent, WindowConfig, WindowId};

/// Per-window state.
struct WindowState {
    window: Window,
    gl: GlState,
    egui: EguiState,
    /// Next scheduled repaint time (if any).
    repaint_at: Option<Instant>,
}

/// Main application handler that owns the `App` and all window state.
struct Handler<A: App> {
    app: A,
    initial_config: Option<WindowConfig>,
    windows: HashMap<winit::window::WindowId, WindowState>,
}

impl<A: App> Handler<A> {
    fn create_window_from_config(&mut self, event_loop: &ActiveEventLoop, config: WindowConfig) {
        let mut attrs = WindowAttributes::default().with_title(&config.title);

        if let Some((w, h)) = config.inner_size {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(w, h));
        }

        if config.transparent {
            attrs = attrs.with_transparent(true);
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

        let state = WindowState {
            window,
            gl,
            egui,
            repaint_at: Some(Instant::now()),
        };

        self.windows.insert(winit_id, state);
        self.app
            .on_window_created(window_id, &self.windows[&winit_id].egui.ctx);

        debug!("Window created: {winit_id:?}");
    }

    fn close_window(&mut self, winit_id: winit::window::WindowId) {
        if let Some(state) = self.windows.remove(&winit_id) {
            // Destroy painter before GL context
            drop(state.egui);
            drop(state.gl);
            drop(state.window);
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
}

impl<A: App> ApplicationHandler<UserEvent> for Handler<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        info!("Event loop resumed");
        if let Some(config) = self.initial_config.take() {
            self.create_window_from_config(event_loop, config);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        winit_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Pass to egui first
        let egui_consumed = if let Some(state) = self.windows.get_mut(&winit_id) {
            let response = state.egui.on_window_event(&state.window, &event);
            if response.repaint {
                state.repaint_at = Some(Instant::now());
                state.window.request_redraw();
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
            WindowEvent::Resized(size) => {
                if let Some(state) = self.windows.get_mut(&winit_id)
                    && let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    state.gl.resize(w, h);
                    state.repaint_at = Some(Instant::now());
                    state.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Split borrows by destructuring self
                let Handler { app, windows, .. } = self;
                let Some(state) = windows.get_mut(&winit_id) else {
                    return;
                };
                let window_id = WindowId(winit_id);
                let clear_color = app.clear_color(window_id);

                state
                    .egui
                    .run_frame(&state.window, &state.gl, clear_color, |ctx, gl| {
                        app.update(window_id, ctx, gl);
                    });

                state.repaint_at = None;
            }
            _ => {
                if !egui_consumed {
                    // App could handle other events here in the future
                }
            }
        }

        // Update control flow based on deadlines
        if let Some(deadline) = self.earliest_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::RequestRepaint(id) => {
                if let Some(state) = self.windows.get_mut(&id.0) {
                    state.repaint_at = Some(Instant::now());
                    state.window.request_redraw();
                }
            }
            UserEvent::RequestRepaintAfter(id, delay) => {
                if let Some(state) = self.windows.get_mut(&id.0) {
                    let deadline = Instant::now() + delay;
                    state.repaint_at = Some(
                        state
                            .repaint_at
                            .map_or(deadline, |existing| existing.min(deadline)),
                    );
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Check if any windows need repaint based on timers
        let now = Instant::now();
        let ids: Vec<winit::window::WindowId> = self.windows.keys().copied().collect();
        for winit_id in ids {
            if let Some(state) = self.windows.get_mut(&winit_id)
                && let Some(deadline) = state.repaint_at
                && deadline <= now
            {
                state.window.request_redraw();
            }
        }

        if let Some(deadline) = self.earliest_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

/// Entry point — replaces `eframe::run_native()`.
///
/// Creates the event loop, opens the initial window with the given config,
/// and runs the application until all windows are closed.
pub fn run(config: WindowConfig, app: impl App + 'static) -> Result<(), Error> {
    let event_loop = EventLoop::with_user_event()
        .build()
        .map_err(|e| Error::EventLoopCreation(format!("{e}")))?;

    let mut handler = Handler {
        app,
        initial_config: Some(config),
        windows: HashMap::new(),
    };

    event_loop
        .run_app(&mut handler)
        .map_err(|e| Error::EventLoopCreation(format!("event loop exited with error: {e}")))?;

    Ok(())
}
