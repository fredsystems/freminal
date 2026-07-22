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
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;
use raw_window_handle::HasWindowHandle;
use tracing::warn;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

use crate::DamageRect;
use crate::error::Error;

// EGL partial-present is only available on backends where glutin compiles its
// EGL path — i.e. everywhere except the Apple platforms (which are CGL-only,
// and where `glutin::{surface::RawSurface, display::RawDisplay}` do not even
// have an `Egl` variant). Mirror glutin's own `egl_backend` cfg with an
// OS-based alias so the entire EGL FFI compiles out on macOS/iOS and the code
// falls back to a full `swap_buffers` there.
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod egl_damage {
    use glutin::display::{AsRawDisplay, GlDisplay, RawDisplay};
    use glutin::surface::{AsRawSurface, RawSurface, Surface, WindowSurface};

    use crate::DamageRect;

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

    /// `const char *eglQueryString(EGLDisplay dpy, EGLint name)`.
    type EglQueryStringFn = unsafe extern "C" fn(
        dpy: *const std::ffi::c_void,
        name: std::ffi::c_int,
    ) -> *const std::ffi::c_char;

    /// EGL constant `EGL_EXTENSIONS` for `eglQueryString`.
    const EGL_EXTENSIONS: std::ffi::c_int = 0x3055;

    /// Optional EGL partial-present support probed at context-creation time.
    ///
    /// Present only when the display advertises
    /// `EGL_KHR_swap_buffers_with_damage` (or the `EXT` variant). On any
    /// backend without it this stays `None` and the caller falls back to a
    /// full swap.
    pub(super) struct EglDamageSupport {
        /// Raw `EGLDisplay` handle (borrowed from glutin; valid for the
        /// lifetime of the `GlState`'s display).
        egl_display: *const std::ffi::c_void,
        /// The resolved `eglSwapBuffersWithDamage{KHR,EXT}` function pointer.
        swap_with_damage: EglSwapBuffersWithDamageFn,
    }

    /// Present only the damaged rectangles via `eglSwapBuffersWithDamage`.
    ///
    /// Returns `true` when the partial present succeeded, `false` when the
    /// caller should fall back to a full swap (EGL returned false, the surface
    /// backend was not EGL, or an absurd rect count). Never presents on its
    /// own on the fallback path — the caller does the full swap.
    pub(super) fn swap_with_damage(
        support: &EglDamageSupport,
        surface: &Surface<WindowSurface>,
        rects: &[DamageRect],
    ) -> bool {
        // Flatten to the EGL `EGLint[4 * n]` layout: x, y, width, height.
        let mut flat: Vec<std::ffi::c_int> = Vec::with_capacity(rects.len() * 4);
        for r in rects {
            flat.push(r.x);
            flat.push(r.y);
            flat.push(r.width);
            flat.push(r.height);
        }

        // `support` is only constructed for an EGL surface, so this always
        // matches; the `else` is a defensive fallback should the backend
        // ever differ.
        let RawSurface::Egl(raw_surface) = surface.raw_surface() else {
            return false;
        };

        let count: std::ffi::c_int = match (flat.len() / 4).try_into() {
            Ok(n) => n,
            // Absurd rect count; a full present is always correct.
            Err(_) => return false,
        };
        // SAFETY: `support.swap_with_damage` was resolved from this display's
        // `get_proc_address` at creation time and only exists when the EGL
        // damage extension is advertised. `support.egl_display` and
        // `raw_surface` are the live EGL display/surface handles for this
        // context (the surface handle is re-fetched here, not cached). `flat`
        // outlives the call and `count` fits an `EGLint`. The context is
        // current (callers `make_current` before rendering).
        let ok = unsafe {
            (support.swap_with_damage)(support.egl_display, raw_surface, flat.as_ptr(), count)
        };
        // `EGL_FALSE` -> tell the caller to do a normal swap.
        ok != 0
    }

    /// Probe whether the given display+surface support EGL partial present.
    ///
    /// Returns `Some` only when (1) the backend is EGL, (2) the display
    /// advertises `EGL_KHR_swap_buffers_with_damage` or the `EXT` variant, and
    /// (3) the corresponding function pointer resolves.
    pub(super) fn probe(
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
        // `EGLDisplay`. `EGL_EXTENSIONS` returns a static, NUL-terminated
        // string owned by EGL (not freed by us).
        let query_string: EglQueryStringFn = unsafe { std::mem::transmute(query_string_ptr) };
        let ext_ptr = unsafe { query_string(egl_display, EGL_EXTENSIONS) };
        if ext_ptr.is_null() {
            return None;
        }
        // SAFETY: EGL guarantees a NUL-terminated string here.
        let extensions = unsafe { std::ffi::CStr::from_ptr(ext_ptr) }
            .to_str()
            .unwrap_or("");

        // The EGL extension string is space-separated; match whole tokens
        // rather than substrings so a longer extension name that merely
        // contains one of ours as a substring can't produce a false positive.
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

        Some(EglDamageSupport {
            egl_display,
            swap_with_damage,
        })
    }
}

/// Holds all GL state for a single window.
pub struct GlState {
    pub(crate) surface: Surface<WindowSurface>,
    pub(crate) context: PossiblyCurrentContext,
    pub(crate) glow_context: Arc<glow::Context>,
    /// EGL partial-present support, or `None` when unavailable (extension not
    /// advertised). Absent entirely on Apple platforms (CGL, no EGL backend).
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    egl_damage: Option<egl_damage::EglDamageSupport>,
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

        tracing::debug!(
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

        // Probe optional EGL partial-present support. Absent on GLX / WGL and
        // on EGL displays that do not advertise the damage extension; in all
        // those cases we fall back to a full `swap_buffers`. On Apple
        // platforms (CGL) there is no EGL backend at all, so the field and the
        // probe are compiled out.
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        let egl_damage = egl_damage::probe(&gl_display, &surface);

        // Report which present path was selected (#435). On Linux/EGL with a
        // compositor advertising the swap-with-damage extension we get the
        // fast path (skip-clear + partial present for cursor-only frames);
        // everywhere else we do a full clear + full present every frame.
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        if egl_damage.is_some() {
            tracing::info!(
                "Present path: damage-aware (EGL swap-with-damage) — cursor-only \
                 frames will skip the full clear and present only the changed region"
            );
        } else {
            tracing::info!(
                "Present path: full-frame (EGL swap-with-damage unavailable) — \
                 every frame does a full clear + present"
            );
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        tracing::info!(
            "Present path: full-frame (no EGL backend on this platform) — every \
             frame does a full clear + present"
        );

        Ok(Self {
            surface,
            context,
            glow_context,
            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
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
    /// [`GlState::swap_buffers`]. Always `false` on Apple platforms (CGL).
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    pub(crate) const fn supports_partial_present(&self) -> bool {
        self.egl_damage.is_some()
    }

    /// Always `false` on Apple platforms — there is no EGL backend.
    // `&self` is kept for call-site symmetry with the non-Apple variant (the
    // caller does `self.supports_partial_present()`); the Apple answer is a
    // constant, so `self` is genuinely unused here.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[allow(clippy::unused_self)]
    pub(crate) const fn supports_partial_present(&self) -> bool {
        false
    }

    /// Present only the damaged rectangles via `eglSwapBuffersWithDamage`.
    ///
    /// `rects` are in physical pixels with a bottom-left origin (EGL
    /// convention). Falls back to a full [`GlState::swap_buffers`] when
    /// partial present is unsupported (extension absent, EGL returns false,
    /// or — on Apple platforms — there is no EGL backend at all), so callers
    /// may invoke it unconditionally.
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    pub(crate) fn swap_buffers_with_damage(&self, rects: &[DamageRect]) -> Result<(), Error> {
        let Some(support) = &self.egl_damage else {
            return self.swap_buffers();
        };
        if egl_damage::swap_with_damage(support, &self.surface, rects) {
            // Partial present succeeded.
            Ok(())
        } else {
            // Extension said no / non-EGL surface / absurd rect count — do a
            // normal full swap so the frame still presents.
            self.swap_buffers()
        }
    }

    /// On Apple platforms there is no EGL partial present; always full swap.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub(crate) fn swap_buffers_with_damage(&self, _rects: &[DamageRect]) -> Result<(), Error> {
        self.swap_buffers()
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
