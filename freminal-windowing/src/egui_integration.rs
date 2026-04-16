//! egui integration: input translation and rendering via `egui-winit` and `egui_glow`.

use std::sync::Arc;

use winit::window::Window;

use crate::error::Error;
use crate::gl_context::GlState;

/// Per-window egui state.
pub(crate) struct EguiState {
    pub(crate) ctx: egui::Context,
    pub(crate) winit_state: egui_winit::State,
    pub(crate) painter: egui_glow::Painter,
}

impl EguiState {
    /// Create egui state for a window.
    pub(crate) fn new(window: &Window, gl_state: &GlState) -> Result<Self, Error> {
        let ctx = egui::Context::default();

        let winit_state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        let painter = egui_glow::Painter::new(Arc::clone(&gl_state.glow_context), "", None, false)
            .map_err(|e| Error::GlContextCreation(format!("egui painter creation failed: {e}")))?;

        Ok(Self {
            ctx,
            winit_state,
            painter,
        })
    }

    /// Run a single egui frame and paint.
    pub(crate) fn run_frame<F>(
        &mut self,
        window: &Window,
        gl_state: &GlState,
        clear_color: [f32; 4],
        ui_fn: F,
    ) where
        F: FnMut(&egui::Context, &glow::Context),
    {
        let mut ui_fn = ui_fn;
        let raw_input = self.winit_state.take_egui_input(window);

        #[expect(
            deprecated,
            reason = "run_ui takes &mut Ui, we need &Context for App trait"
        )]
        let full_output = self.ctx.run(raw_input, |ctx| {
            ui_fn(ctx, &gl_state.glow_context);
        });

        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        let pixels_per_point = self.ctx.pixels_per_point();
        let clipped_primitives = self.ctx.tessellate(full_output.shapes, pixels_per_point);

        let size = window.inner_size();
        gl_state.clear(clear_color);

        self.painter.paint_and_update_textures(
            [size.width, size.height],
            pixels_per_point,
            &clipped_primitives,
            &full_output.textures_delta,
        );

        // Pre-present notify for Wayland frame pacing
        window.pre_present_notify();

        if let Err(e) = gl_state.swap_buffers() {
            tracing::error!("swap_buffers failed: {e}");
        }

        // Schedule repaint if egui needs it
        let repaint_delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|vo| vo.repaint_delay)
            .unwrap_or(std::time::Duration::MAX);

        if repaint_delay.is_zero() {
            window.request_redraw();
        }
    }

    /// Pass a winit `WindowEvent` to egui.
    pub(crate) fn on_window_event(
        &mut self,
        window: &Window,
        event: &winit::event::WindowEvent,
    ) -> egui_winit::EventResponse {
        self.winit_state.on_window_event(window, event)
    }
}
