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
use crate::{App, UserEvent, WindowConfig, WindowHandle, WindowId, WindowOp};

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
    proxy: EventLoopProxy<UserEvent>,
    /// Scratch buffer for pending `WindowOp`s queued by `WindowHandle`.
    pending_ops: RefCell<Vec<WindowOp>>,
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
        };

        self.windows.insert(winit_id, state);

        // Track the first window as the primary clipboard source.

        // Request an immediate redraw so the first frame renders as soon as
        // the event loop is ready.  `repaint_at` alone only fires in
        // `about_to_wait`, which may not schedule a second frame quickly
        // enough for the terminal to display the initial shell prompt.
        self.windows[&winit_id].window.request_redraw();

        let handle = WindowHandle {
            proxy: &self.proxy,
            pending_ops: &self.pending_ops,
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

    /// Drain and execute all pending `WindowOp`s queued by `WindowHandle`.
    fn process_pending_ops(&mut self, event_loop: &ActiveEventLoop) {
        let ops: Vec<WindowOp> = self.pending_ops.borrow_mut().drain(..).collect();
        for op in ops {
            match op {
                WindowOp::CreateWindow(config) => {
                    self.create_window_from_config(event_loop, config);
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
                        let deadline = Instant::now() + delay;
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
            self.create_window_from_config(event_loop, config);
        }
    }

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
        if matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::CursorLeft { .. }
        ) {
            if let Some(state) = self.windows.get_mut(&winit_id) {
                let response = state.egui.on_window_event(&state.window, &event);
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
                        // Don't pass to egui-winit — it would produce a
                        // duplicate paste on windows where its clipboard works.
                        self.update_control_flow(event_loop);
                        return;
                    }
                }
            }
        }

        // Pass to egui first
        let egui_consumed = if let Some(state) = self.windows.get_mut(&winit_id) {
            let response = state.egui.on_window_event(&state.window, &event);

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
            WindowEvent::Resized(size) => {
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
            }
            WindowEvent::RedrawRequested => {
                // Split borrows by destructuring
                let Handler {
                    app,
                    windows,
                    proxy,
                    pending_ops,
                    ..
                } = self;
                let Some(state) = windows.get_mut(&winit_id) else {
                    return;
                };
                let window_id = WindowId(winit_id);
                let clear_color = app.clear_color(window_id);

                let handle = WindowHandle { proxy, pending_ops };

                // Ensure this window's GL context is current before rendering.
                if let Err(e) = state.gl.make_current() {
                    error!("make_current failed for {winit_id:?}: {e}");
                    return;
                }

                // Collect raw input, let app hook modify it, then run the frame.
                let mut raw_input = state.egui.take_egui_input(&state.window);
                app.raw_input_hook(window_id, &mut raw_input);

                let frame_output = state.egui.run_frame(
                    &state.window,
                    &state.gl,
                    clear_color,
                    raw_input,
                    |ctx, gl| {
                        app.update(window_id, ctx, gl, &handle);
                    },
                );

                // Process egui viewport commands.
                let mut should_close = false;
                for cmd in frame_output.commands {
                    process_viewport_command(&state.window, cmd, &mut should_close);
                }

                state.repaint_at = None;

                // Honour egui's repaint_delay but clamp to a minimum of 16ms
                // to prevent unbounded rendering from zero-delay requests
                // (hover state, tooltip updates).  This ensures layout-settling
                // frames still fire while keeping idle CPU near zero.
                if frame_output.repaint_delay < std::time::Duration::from_secs(3600) {
                    let min_delay = std::time::Duration::from_millis(16);
                    let delay = frame_output.repaint_delay.max(min_delay);
                    let deadline = Instant::now() + delay;
                    state.repaint_at = Some(deadline);
                }

                // Process any ops queued during update.
                self.process_pending_ops(event_loop);

                if should_close {
                    self.close_window(winit_id);
                    if self.windows.is_empty() {
                        event_loop.exit();
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
                    let deadline = Instant::now() + delay;
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
}

/// Process a single egui `ViewportCommand` by mapping it to the corresponding
/// winit `Window` API call.
///
/// Commands that require closing the window set `*should_close = true`; the
/// caller is responsible for actually closing the window after the frame
/// completes (to avoid mutating the window map during iteration).
fn process_viewport_command(window: &Window, cmd: egui::ViewportCommand, should_close: &mut bool) {
    match cmd {
        egui::ViewportCommand::Close => {
            *should_close = true;
        }
        egui::ViewportCommand::CancelClose => {
            // In our model, close is synchronous via on_close_requested.
            // CancelClose is a no-op since we don't queue deferred closes.
        }
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
    };

    event_loop
        .run_app(&mut handler)
        .map_err(|e| Error::EventLoopCreation(format!("event loop exited with error: {e}")))?;

    Ok(())
}
