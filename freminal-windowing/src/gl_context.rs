//! GL context management via glutin.
//!
//! Handles OpenGL display, surface, and context creation for each window.

use std::num::NonZeroU32;
use std::sync::Arc;

use glow::HasContext;
use glutin::config::{Config, ConfigTemplateBuilder};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use tracing::{debug, warn};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::error::Error;

/// Holds all GL state for a single window.
pub(crate) struct GlState {
    pub(crate) surface: Surface<WindowSurface>,
    pub(crate) context: PossiblyCurrentContext,
    pub(crate) glow_context: Arc<glow::Context>,
}

impl GlState {
    /// Create GL state for the given window, using the active event loop for display creation.
    pub(crate) fn new(
        event_loop: &ActiveEventLoop,
        window: &Window,
        transparent: bool,
    ) -> Result<Self, Error> {
        let window_handle = window
            .window_handle()
            .map_err(|e| Error::GlContextCreation(format!("window handle: {e}")))?;

        // Build config template
        let mut config_template = ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(Some(true))
            .with_depth_size(0)
            .with_stencil_size(0)
            .compatible_with_native_window(window_handle.as_raw());

        if transparent {
            config_template = config_template.with_transparency(true).with_alpha_size(8);
        }

        // Use DisplayBuilder without a window (window already created) to get a display + config
        let display_builder = DisplayBuilder::new().with_window_attributes(None);

        let (_no_window, gl_config) = display_builder
            .build(event_loop, config_template, pick_best_config)
            .map_err(|e| Error::GlContextCreation(format!("display builder: {e}")))?;

        let gl_display = gl_config.display();

        debug!(
            "Selected GL config: samples={}, alpha={}",
            gl_config.num_samples(),
            gl_config.alpha_size()
        );

        // Create context
        let context_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(None))
            .build(Some(window_handle.as_raw()));

        let fallback_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(Some(window_handle.as_raw()));

        let not_current_context = unsafe {
            gl_display
                .create_context(&gl_config, &context_attributes)
                .or_else(|_| gl_display.create_context(&gl_config, &fallback_attributes))
                .map_err(|e| Error::GlContextCreation(format!("context creation: {e}")))?
        };

        // Create surface
        let size = window.inner_size();
        let width = NonZeroU32::new(size.width.max(1))
            .ok_or_else(|| Error::SurfaceCreation("zero width".to_owned()))?;
        let height = NonZeroU32::new(size.height.max(1))
            .ok_or_else(|| Error::SurfaceCreation("zero height".to_owned()))?;

        let surface_attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            window_handle.as_raw(),
            width,
            height,
        );

        let surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attributes)
                .map_err(|e| Error::SurfaceCreation(format!("{e}")))?
        };

        // Make context current
        let context = not_current_context
            .make_current(&surface)
            .map_err(|e| Error::MakeCurrent(format!("{e}")))?;

        // Disable vsync — demand-driven rendering handles pacing
        if surface
            .set_swap_interval(&context, SwapInterval::DontWait)
            .is_err()
        {
            warn!("Failed to set swap interval");
        }

        // Create glow context
        let glow_context = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|name| gl_display.get_proc_address(name))
        });

        Ok(Self {
            surface,
            context,
            glow_context,
        })
    }

    /// Make this window's GL context current.
    ///
    /// Must be called before any GL operations (clear, paint, swap) when
    /// multiple windows share the same thread — only one context can be
    /// current at a time.
    pub(crate) fn make_current(&self) -> Result<(), Error> {
        self.context
            .make_current(&self.surface)
            .map_err(|e| Error::MakeCurrent(format!("{e}")))
    }

    /// Resize the GL surface.
    pub(crate) fn resize(&self, width: NonZeroU32, height: NonZeroU32) {
        self.surface.resize(&self.context, width, height);
    }

    /// Swap buffers.
    pub(crate) fn swap_buffers(&self) -> Result<(), Error> {
        self.surface
            .swap_buffers(&self.context)
            .map_err(|e| Error::SwapBuffers(format!("{e}")))
    }

    /// Clear the framebuffer.
    pub(crate) fn clear(&self, color: [f32; 4]) {
        unsafe {
            self.glow_context
                .clear_color(color[0], color[1], color[2], color[3]);
            self.glow_context.clear(glow::COLOR_BUFFER_BIT);
        }
    }
}

/// Pick the GL config with the highest multisample count.
///
/// If the iterator is empty — which should be impossible given that a prior
/// `DisplayBuilder` step succeeded — this logs a diagnostic and exits the
/// process rather than panicking, because the glutin API forces the closure
/// to return a `Config` (no error channel available).
fn pick_best_config(mut configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    let Some(first) = configs.next() else {
        // Unreachable under glutin's documented contract (successful display
        // build guarantees at least one config). If the invariant ever breaks,
        // exit with a diagnostic rather than panicking.
        tracing::error!("glutin returned zero GL configs; cannot continue");
        std::process::exit(1);
    };
    configs.fold(first, |accum, c| {
        if c.num_samples() > accum.num_samples() {
            c
        } else {
            accum
        }
    })
}
