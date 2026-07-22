// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GL context management via glutin.
//!
//! Handles OpenGL display, surface, and context creation for each window.

use std::num::NonZeroU32;
use std::sync::Arc;

use glow::HasContext;
use glutin::config::{Config, ConfigTemplateBuilder};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{AsRawDisplay, GetGlDisplay, RawDisplay};
use glutin::prelude::*;
use glutin::surface::{
    AsRawSurface, RawSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface,
};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use tracing::{debug, warn};
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::DamageRect;
use crate::error::Error;

/// The EGL `eglSwapBuffersWithDamageKHR` / `EXT` entry point signature.
///
/// `EGLBoolean eglSwapBuffersWithDamage(EGLDisplay dpy, EGLSurface surface,
/// const EGLint *rects, EGLint n_rects)`. Each rect is 4 `EGLint`s
/// (`x, y, width, height`), bottom-left origin, in physical pixels.
type EglSwapBuffersWithDamageFn = unsafe extern "C" fn(
    dpy: *const std::ffi::c_void,
    surface: *const std::ffi::c_void,
    rects: *const std::ffi::c_int,
    n_rects: std::ffi::c_int,
) -> std::ffi::c_uint;

/// Optional EGL partial-present support probed at context-creation time.
///
/// Present only when the GL backend is EGL **and** the display advertises
/// `EGL_KHR_swap_buffers_with_damage` (or the `EXT` variant). On any other
/// backend (GLX, WGL on Windows, CGL on macOS) this stays `None` and the
/// windowing layer falls back to a full [`GlState::swap_buffers`].
struct EglDamageSupport {
    /// Raw `EGLDisplay` handle (borrowed from glutin; valid for the lifetime
    /// of the `GlState`'s display).
    egl_display: *const std::ffi::c_void,
    /// The resolved `eglSwapBuffersWithDamage{KHR,EXT}` function pointer.
    swap_with_damage: EglSwapBuffersWithDamageFn,
}

/// Holds all GL state for a single window.
pub struct GlState {
    pub(crate) surface: Surface<WindowSurface>,
    pub(crate) context: PossiblyCurrentContext,
    pub(crate) glow_context: Arc<glow::Context>,
    /// EGL partial-present support, or `None` when unavailable (non-EGL
    /// backend, or the damage extension is not advertised).
    egl_damage: Option<EglDamageSupport>,
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

        // Probe optional EGL partial-present support. Absent on GLX / WGL /
        // CGL and on EGL displays that do not advertise the damage extension;
        // in all those cases we fall back to a full `swap_buffers`.
        let egl_damage = probe_egl_damage_support(&gl_display, &surface);

        Ok(Self {
            surface,
            context,
            glow_context,
            egl_damage,
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

    /// Age of the current back buffer.
    ///
    /// Returns `1` when the back buffer still holds the **previous** frame's
    /// contents (so a skip-clear + partial redraw is safe), and `0` when the
    /// buffer is new or its age is unknown (so the whole buffer must be
    /// redrawn). Values `> 1` mean the buffer holds an older frame and is
    /// likewise unsafe to treat as "last frame".
    ///
    /// On non-EGL backends and platforms without `EGL_EXT_buffer_age` this
    /// returns `0`, which correctly forces the full-frame path.
    pub(crate) fn buffer_age(&self) -> u32 {
        self.surface.buffer_age()
    }

    /// Whether this surface can present only a damaged sub-region
    /// (`eglSwapBuffersWithDamage`). When `false`, callers must use the full
    /// [`GlState::swap_buffers`].
    pub(crate) const fn supports_partial_present(&self) -> bool {
        self.egl_damage.is_some()
    }

    /// Present only the damaged rectangles via `eglSwapBuffersWithDamage`.
    ///
    /// `rects` are in physical pixels with a bottom-left origin (EGL
    /// convention). An empty slice damages the entire surface (per the EGL
    /// spec), matching a normal swap.
    ///
    /// Falls back to a full [`GlState::swap_buffers`] when partial present is
    /// unsupported, so callers may invoke it unconditionally.
    pub(crate) fn swap_buffers_with_damage(&self, rects: &[DamageRect]) -> Result<(), Error> {
        let Some(damage) = &self.egl_damage else {
            return self.swap_buffers();
        };

        // Flatten to the EGL `EGLint[4 * n]` layout: x, y, width, height.
        let mut flat: Vec<std::ffi::c_int> = Vec::with_capacity(rects.len() * 4);
        for r in rects {
            flat.push(r.x);
            flat.push(r.y);
            flat.push(r.width);
            flat.push(r.height);
        }

        // `egl_damage` is only `Some` for an EGL surface, so this always
        // matches; the `else` is a defensive fallback should the backend ever
        // differ.
        let RawSurface::Egl(raw_surface) = self.surface.raw_surface() else {
            return self.swap_buffers();
        };

        // SAFETY: `damage.swap_with_damage` was resolved from this display's
        // `get_proc_address` at creation time and is only `Some` when the EGL
        // damage extension is advertised. `damage.egl_display` and
        // `raw_surface` are the live EGL display/surface handles for this
        // context (the surface handle is re-fetched here, not cached). `flat`
        // outlives the call and its length in rects (`flat.len() / 4`) fits an
        // `EGLint` for any realistic rect count. The context is current
        // (callers `make_current` before rendering), as `swap_buffers` also
        // requires.
        let count: std::ffi::c_int = match (flat.len() / 4).try_into() {
            Ok(n) => n,
            // Absurd rect count; a full present is always correct.
            Err(_) => return self.swap_buffers(),
        };
        let ok = unsafe {
            (damage.swap_with_damage)(damage.egl_display, raw_surface, flat.as_ptr(), count)
        };
        if ok == 0 {
            // EGL_FALSE — fall back to a normal swap so the frame still
            // presents. (An eglGetError diagnostic is not worth a per-frame
            // FFI call on the hot path.)
            return self.swap_buffers();
        }
        Ok(())
    }
}

/// EGL constant `EGL_EXTENSIONS` for `eglQueryString`.
const EGL_EXTENSIONS: std::ffi::c_int = 0x3055;

/// `const char *eglQueryString(EGLDisplay dpy, EGLint name)`.
type EglQueryStringFn = unsafe extern "C" fn(
    dpy: *const std::ffi::c_void,
    name: std::ffi::c_int,
) -> *const std::ffi::c_char;

/// Probe whether the given display+surface support EGL partial present.
///
/// Returns `Some` only when (1) the backend is EGL, (2) the display
/// advertises `EGL_KHR_swap_buffers_with_damage` or
/// `EGL_EXT_swap_buffers_with_damage`, and (3) the corresponding function
/// pointer resolves. On GLX / WGL / CGL the `RawDisplay`/`RawSurface`
/// `Egl` variants are absent (or the match falls through), so this returns
/// `None` and the caller uses the full-swap path.
fn probe_egl_damage_support(
    display: &glutin::display::Display,
    surface: &Surface<WindowSurface>,
) -> Option<EglDamageSupport> {
    // Must be an EGL display and an EGL surface.
    let RawDisplay::Egl(egl_display) = display.raw_display() else {
        return None;
    };
    if !matches!(surface.raw_surface(), RawSurface::Egl(_)) {
        return None;
    }

    // Query the display extension string.
    let query_string_ptr = display.get_proc_address(c"eglQueryString");
    if query_string_ptr.is_null() {
        return None;
    }
    // SAFETY: `eglQueryString` resolved to a non-null pointer with the
    // documented EGL signature; `egl_display` is this display's live
    // `EGLDisplay`. `EGL_EXTENSIONS` returns a static, NUL-terminated string
    // owned by EGL (not freed by us).
    let query_string: EglQueryStringFn = unsafe { std::mem::transmute(query_string_ptr) };
    let ext_ptr = unsafe { query_string(egl_display, EGL_EXTENSIONS) };
    if ext_ptr.is_null() {
        return None;
    }
    // SAFETY: EGL guarantees a NUL-terminated string here.
    let extensions = unsafe { std::ffi::CStr::from_ptr(ext_ptr) }
        .to_str()
        .unwrap_or("");

    // The EGL extension string is space-separated; match whole tokens rather
    // than substrings so a longer extension name that merely contains one of
    // ours as a substring can't produce a false positive.
    let has_ext = |name: &str| extensions.split(' ').any(|tok| tok == name);
    // Prefer KHR, then EXT; both share the same signature.
    let symbol = if has_ext("EGL_KHR_swap_buffers_with_damage") {
        Some(c"eglSwapBuffersWithDamageKHR")
    } else if has_ext("EGL_EXT_swap_buffers_with_damage") {
        Some(c"eglSwapBuffersWithDamageEXT")
    } else {
        None
    };
    let symbol = symbol?;

    let fn_ptr = display.get_proc_address(symbol);
    if fn_ptr.is_null() {
        return None;
    }
    // SAFETY: the pointer is non-null and resolved for a symbol whose
    // advertised extension guarantees the documented signature.
    let swap_with_damage: EglSwapBuffersWithDamageFn = unsafe { std::mem::transmute(fn_ptr) };

    debug!("EGL partial present enabled ({})", symbol.to_string_lossy());
    Some(EglDamageSupport {
        egl_display,
        swap_with_damage,
    })
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
