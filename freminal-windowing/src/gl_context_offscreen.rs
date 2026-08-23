// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Offscreen GL context construction (Task 123, subtask 123.11).
//!
//! One concept: building a current GL context that renders into an
//! offscreen pbuffer instead of a window. This is the Phase 2 counterpart
//! to [`crate::gl_context`]'s windowed path, and is **additive** — it
//! shares no code with, and changes nothing about, how the real
//! application creates its context.
//!
//! # Why this exists
//!
//! `PROFILING.md` names the absence of a pixel-readback harness as the
//! single biggest hole in freminal's methodology: a regression that changes
//! *what* is drawn, rather than how often, is invisible to every existing
//! check. Phase 1's call-recording harness closed the "how often" half.
//! This closes the "what" half by giving tests a real GL context they can
//! draw into and read back from.
//!
//! # Requirements, and why they are Linux-only
//!
//! - **Mesa** (`swrast`/llvmpipe) supplies a software rasteriser, so no GPU
//!   is needed. `pkgs.libGL` alone is libglvnd — a dispatcher with no
//!   rendering backend — which is why `flake.nix` adds `mesa` explicitly.
//! - **An X server**, normally `Xvfb`. EGL still needs a display connection
//!   to enumerate configs on this platform.
//! - Both are wired up in the **`default`** Nix dev shell only (see
//!   `glPixelEnv` in `flake.nix`), following the same `stdenv.isLinux`
//!   precedent as `pkgs.perf`.
//!
//! Run anything built on this under `xvfb-run`:
//!
//! ```sh
//! xvfb-run -a cargo test -p freminal-windowing --features gl-offscreen
//! ```
//!
//! # Why there is no winit `Window` here
//!
//! 123.11 was specified as a pbuffer path, and it turns out not to need
//! winit at all: [`glutin::display::Display::new`] accepts a raw display
//! handle directly. That matters for testability rather than tidiness — a
//! winit `EventLoop` can only be constructed once per process and must be
//! on the main thread, which would make a context-per-test harness
//! impossible. Going straight to EGL sidesteps that entirely, so each test
//! can own an independent context.

use std::num::NonZeroU32;

use glow::HasContext;
use glutin::config::{Config, ConfigSurfaceTypes, ConfigTemplateBuilder};
use glutin::context::{ContextApi, ContextAttributesBuilder, PossiblyCurrentContext};
use glutin::display::{Display, DisplayApiPreference, GetGlDisplay};
use glutin::prelude::*;
use glutin::surface::{PbufferSurface, Surface, SurfaceAttributesBuilder};
use raw_window_handle::{RawDisplayHandle, XlibDisplayHandle};

use crate::error::Error;

/// A current GL context rendering into an offscreen pbuffer.
///
/// Holds the display, config, surface and context together because they
/// must outlive the [`glow::Context`] built from them — dropping any of
/// them first would leave the glow context pointing at freed driver state.
/// Field order is drop order, and it is deliberate: `context` is dropped
/// before `surface`, which is dropped before `display`.
pub struct OffscreenGl {
    /// The glow context callers actually draw through.
    gl: glow::Context,
    /// Kept alive for `gl`'s benefit; also the make-current token.
    _context: PossiblyCurrentContext,
    /// Kept alive for `gl`'s benefit.
    _surface: Surface<PbufferSurface>,
    /// Kept alive for `gl`'s benefit.
    _display: Display,
    /// Pixel width the pbuffer was created with.
    width: u32,
    /// Pixel height the pbuffer was created with.
    height: u32,
}

impl OffscreenGl {
    /// Create a current offscreen GL context of `width` x `height` pixels.
    ///
    /// # Errors
    ///
    /// Returns [`Error::GlContextCreation`] if no EGL display or
    /// pbuffer-capable config is available (most often: no `DISPLAY`, i.e.
    /// not running under `xvfb-run`, or Mesa is absent from the
    /// environment), and [`Error::SurfaceCreation`] /
    /// [`Error::MakeCurrent`] for the corresponding later steps.
    pub fn new(width: u32, height: u32) -> Result<Self, Error> {
        let nz_width = NonZeroU32::new(width)
            .ok_or_else(|| Error::SurfaceCreation("zero width".to_owned()))?;
        let nz_height = NonZeroU32::new(height)
            .ok_or_else(|| Error::SurfaceCreation("zero height".to_owned()))?;

        // A null Xlib display makes EGL use the platform default, which is
        // whatever `$DISPLAY` points at — the virtual server under
        // `xvfb-run`. Passing the handle rather than opening an X
        // connection ourselves keeps this free of any Xlib dependency.
        let raw = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));

        // EGL specifically, not `GlxThenEgl`: GLX is phasing out, and the
        // pbuffer path is better supported on EGL with Mesa.
        let display = unsafe { Display::new(raw, DisplayApiPreference::Egl) }
            .map_err(|e| Error::GlContextCreation(format!("offscreen EGL display: {e}")))?;

        let config = Self::pick_config(&display)?;

        let context_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::OpenGl(None))
            .build(None);
        // Mesa's llvmpipe offers desktop GL, but ask for GLES as a fallback
        // so this still works on a stack that only advertises GLES.
        let fallback_attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(None);

        let not_current = unsafe {
            display
                .create_context(&config, &context_attributes)
                .or_else(|_| display.create_context(&config, &fallback_attributes))
                .map_err(|e| Error::GlContextCreation(format!("offscreen context: {e}")))?
        };

        let surface_attributes =
            SurfaceAttributesBuilder::<PbufferSurface>::new().build(nz_width, nz_height);
        let surface = unsafe {
            config
                .display()
                .create_pbuffer_surface(&config, &surface_attributes)
                .map_err(|e| Error::SurfaceCreation(format!("pbuffer: {e}")))?
        };

        let context = not_current
            .make_current(&surface)
            .map_err(|e| Error::MakeCurrent(format!("offscreen: {e}")))?;

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        };

        Ok(Self {
            gl,
            _context: context,
            _surface: surface,
            _display: display,
            width,
            height,
        })
    }

    /// Choose a pbuffer-capable RGBA config.
    ///
    /// `ConfigSurfaceTypes::PBUFFER` is the load-bearing part: the default
    /// template asks for a window-capable config, which a headless EGL
    /// display may not offer at all.
    fn pick_config(display: &Display) -> Result<Config, Error> {
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_surface_type(ConfigSurfaceTypes::PBUFFER)
            .build();

        unsafe { display.find_configs(template) }
            .map_err(|e| Error::GlContextCreation(format!("offscreen config search: {e}")))?
            .next()
            .ok_or_else(|| {
                Error::GlContextCreation(
                    "no pbuffer-capable EGL config (is Mesa present and $DISPLAY set?)".to_owned(),
                )
            })
    }

    /// The glow context to draw through.
    pub const fn gl(&self) -> &glow::Context {
        &self.gl
    }

    /// Pixel dimensions of the pbuffer, as `(width, height)`.
    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The GL renderer string, for reporting which rasteriser produced a
    /// result.
    ///
    /// Worth recording alongside any pixel comparison: a golden image is
    /// only meaningful relative to the rasteriser that produced it, and
    /// llvmpipe's output can shift between Mesa releases.
    pub fn renderer(&self) -> String {
        unsafe { self.gl.get_parameter_string(glow::RENDERER) }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::OffscreenGl;
    use glow::HasContext;

    /// 123.11's required smoke test: clear to a known colour, read the
    /// framebuffer back, and assert the pixel matches.
    ///
    /// **Requires `xvfb-run` (or any live `$DISPLAY`) plus Mesa.** Outside
    /// the Linux `default` dev shell this cannot pass, so it skips rather
    /// than fails when no context can be created — a test that hard-failed
    /// on a developer's machine for want of an X server would teach people
    /// to ignore it.
    ///
    /// **In CI it does not skip, it fails.** The Phase 2 job (123.13)
    /// guarantees Mesa and Xvfb, so a missing context there means a broken
    /// runner, and silently skipping would turn that into a false green —
    /// the exact failure the skip path is otherwise designed to avoid.
    #[test]
    fn clear_and_readback_round_trips() {
        let off = match OffscreenGl::new(64, 32) {
            Ok(off) => off,
            Err(e) => {
                // GitHub Actions sets `CI=true`.
                let in_ci = std::env::var("CI").is_ok_and(|v| v != "false" && !v.is_empty());
                assert!(
                    !in_ci,
                    "no offscreen GL context in CI ({e}) -- the gl-pixel job \
                     guarantees Mesa and Xvfb, so this is a broken runner, \
                     not a reason to skip"
                );
                eprintln!(
                    "skipping: no offscreen GL context ({e}) (needs Mesa + \
                     $DISPLAY, e.g. `xvfb-run -a cargo test -p \
                     freminal-windowing --features gl-offscreen`)"
                );
                return;
            }
        };

        assert_eq!(off.size(), (64, 32));
        eprintln!("offscreen renderer: {}", off.renderer());

        let gl = off.gl();
        let mut pixel = [0u8; 4];
        unsafe {
            // 0.25/0.5/0.75 are chosen to land on exact 8-bit values
            // (64/128/191 after rounding), so an exact assertion is
            // legitimate rather than a rounding coincidence.
            gl.clear_color(0.25, 0.5, 0.75, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.finish();
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
        }

        assert_eq!(
            pixel,
            [64, 128, 191, 255],
            "offscreen clear + readback must round-trip exactly on llvmpipe"
        );
    }
}
