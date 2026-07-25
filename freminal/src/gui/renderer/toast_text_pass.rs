// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Self-contained OpenGL pass that draws toast label/icon TEXT (issue #433).
//!
//! Companion to [`super::toast_pass`], which draws the toast "pill"
//! background. That pass draws a rounded-rect SDF quad; this one draws the
//! label/detail/icon glyphs *through the same instanced foreground shader*
//! the terminal grid uses (`fg.vert` / `fg.frag`, see [`super::gpu`]'s
//! `init_fg_pass` / `draw_foreground`), so pill and text share one absolute
//! pixel coordinate system — no cell grid involved.
//!
//! [`ToastTextRenderer`] owns its own shader program, VAO, unit-quad VBO,
//! double-buffered instance VBOs, and its own dedicated [`GlyphAtlas`] (kept
//! separate from the terminal grid's atlas so toast text — which is
//! typically rasterised at a different pixel size than the terminal's cell
//! font — never evicts or is evicted by grid glyphs). It is driven by the
//! toast overlay's `PaintCallback` in [`crate::gui::toast`] (see
//! `ToastStack::show` / `paint_toasts`): text is shaped and instance data
//! built on the main thread, then [`ToastTextRenderer::upload_and_draw`] runs
//! on the GL thread.
//!
//! ## Main-thread / GL-thread split
//!
//! [`crate::gui::font_manager::FontManager`] is `!Sync` and must only be
//! touched on the main (GUI) thread — never from inside a `PaintCallback`,
//! which egui may invoke from a render thread. This module is split
//! accordingly:
//!
//! - [`ToastTextRenderer::build_instances`] — runs on the **main thread**.
//!   Takes `&mut FontManager`, shapes each [`ToastTextRun`] via
//!   `rustybuzz`, rasterises any new glyphs into `self.atlas`, and returns a
//!   flat `Vec<f32>` of `FG_INSTANCE_FLOATS`-per-glyph instance data. Issues
//!   **no GL calls**.
//! - [`ToastTextRenderer::upload_and_draw`] — runs from the **GL callback**.
//!   Takes only `&glow::Context` plus the prebuilt instance buffer: syncs
//!   the atlas texture, uploads instances, and draws. Touches no
//!   `FontManager`.
//!
//! ## Single-face-per-run assumption
//!
//! Each [`ToastTextRun`] is shaped as a single `rustybuzz` run against the
//! face resolved from its **first character**. This is correct for the
//! common toast cases (a label string, a detail string, or a single icon
//! glyph) but does **not** handle a run that mixes, say, a bundled
//! nerd-font icon codepoint with Latin text — those would all be shaped
//! against whichever face the first character resolved to, and the other
//! codepoints could come out as tofu. Callers must split icon glyphs into
//! their own single-glyph [`ToastTextRun`], exactly as the terminal grid's
//! run segmentation (see [`crate::gui::shaping`]) splits on face
//! boundaries — that per-character segmentation is deliberately not
//! reimplemented here because toast text is short and this simpler
//! contract is sufficient and easier to test.
//!
//! The CPU-side per-glyph float packing ([`pack_glyph_instance`]) is a pure
//! function, fully testable without a GL context; GL calls are confined to
//! [`ToastTextRenderer::init`], [`ToastTextRenderer::upload_and_draw`], and
//! [`ToastTextRenderer::destroy`].

use conv2::{ApproxFrom, RoundToNearest, ValueFrom};
use glow::{self, HasContext};
use tracing::{error, warn};

use super::super::atlas::{GlyphAtlas, GlyphKey};
use super::super::font_manager::{FontManager, GlyphStyle};
use super::errors::{BufferAllocError, GpuInitError, TextureUploadError};
use super::gpu::{
    compile_program, gl_f32_i32, gl_i32, gl_i32_u32, setup_fg_inst_attribs, upload_verts,
};
use super::shaders::{FG_FRAG_SRC, FG_VERT_SRC};
use super::vertex::{FG_INSTANCE_FLOATS, extract_atlas_rect};

// ---------------------------------------------------------------------------
//  Constants
// ---------------------------------------------------------------------------

/// Static unit quad in `[0,1]²` space (2 triangles = 6 vertices), matching
/// the layout [`setup_fg_inst_attribs`] binds to location 0 with divisor 0.
///
/// Identical topology (and identical values) to the terminal grid's own
/// `UNIT_QUAD` in `gpu.rs` — duplicated here rather than reused because that
/// constant is private to `gpu.rs` and this pass owns its own VBO for it (it
/// cannot share the terminal grid's `bg_unit_quad_vbo`, which belongs to a
/// different `TerminalRenderer` instance entirely).
const UNIT_QUAD: [f32; 12] = [
    0.0, 0.0, 1.0, 0.0, 0.0, 1.0, // triangle 1
    1.0, 0.0, 1.0, 1.0, 0.0, 1.0, // triangle 2
];

/// Initial size (pixels, square) of the dedicated toast-text glyph atlas.
///
/// Toast text is tiny compared to a full terminal grid — a handful of short
/// label/detail strings and icon glyphs, typically one alphabet's worth of
/// distinct glyph shapes. 256px comfortably holds dozens of glyphs before
/// the atlas needs to grow.
const ATLAS_INITIAL_SIZE_PX: u32 = 256;

/// Maximum size (pixels, square) the toast-text atlas is allowed to grow to.
///
/// Far more than any realistic toast stack needs (the terminal grid's own
/// atlas, which must hold every glyph shape in active use across the whole
/// visible buffer, caps at 4096px by comparison).
const ATLAS_MAX_SIZE_PX: u32 = 1024;

// ---------------------------------------------------------------------------
//  Public CPU-side input
// ---------------------------------------------------------------------------

/// One run of text to draw as part of a toast, positioned in the toast
/// `PaintCallback`'s viewport-local physical pixel space (origin top-left).
///
/// See the module-level docs for the single-face-per-run assumption: split
/// icon glyphs into their own run rather than mixing them with label text.
#[derive(Debug, Clone)]
pub struct ToastTextRun {
    /// The string to shape and draw (label, detail, or a single-icon string).
    pub text: String,
    /// Baseline origin X in physical pixels (left edge of the run).
    pub origin_x: f32,
    /// Baseline origin Y in physical pixels (text baseline, not top).
    pub baseline_y: f32,
    /// Font size in pixels to rasterize at.
    pub size_px: f32,
    /// Straight (non-premultiplied) RGBA 0..=1.
    pub color: [f32; 4],
}

/// The pixel footprint a run of text occupies when shaped at a given size.
///
/// Measured without rasterising it into any atlas. Returned by
/// [`ToastTextRenderer::measure`] and consumed by the toast layout model
/// (`crate::gui::toast::layout_toasts`) to size pills to their content.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastTextMetrics {
    /// Total advance width in physical pixels.
    pub width: f32,
    /// Line height (ascent + descent) in physical pixels at this size.
    pub height: f32,
    /// Ascent in physical pixels (baseline offset from the top).
    pub ascent: f32,
}

impl ToastTextMetrics {
    /// The zero-size metrics returned for empty text or an unresolvable face.
    const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
        ascent: 0.0,
    };
}

// ---------------------------------------------------------------------------
//  ToastTextRenderer
// ---------------------------------------------------------------------------

/// Holds all GPU resources, plus the dedicated glyph atlas, for the toast
/// text pass.
///
/// Call [`Self::init`] once (inside a `PaintCallback`) to create the shader
/// program, VAO, and buffers. Then, once per frame: call
/// [`Self::build_instances`] on the **main thread** with the current toast
/// text runs, and [`Self::upload_and_draw`] from the **GL callback** with
/// the returned instance buffer.
pub struct ToastTextRenderer {
    /// Whether GPU resources have been created.
    initialized: bool,
    /// Compiled + linked foreground shader program (shared source with the
    /// terminal grid's foreground pass; see `fg.vert` / `fg.frag`).
    program: Option<glow::Program>,
    /// VAO configured via [`setup_fg_inst_attribs`].
    vao: Option<glow::VertexArray>,
    /// This pass's own static unit-quad VBO (not shared with the terminal
    /// grid's `TerminalRenderer`, which is a different GL object entirely).
    unit_quad_vbo: Option<glow::Buffer>,
    /// Double-buffered instance VBOs (orphan-then-write upload pattern).
    inst_vbo: [Option<glow::Buffer>; 2],
    /// Which of the two `inst_vbo` slots to write into next.
    inst_index: usize,
    /// This pass's own glyph atlas texture (not shared with the terminal
    /// grid's atlas texture).
    atlas_texture: Option<glow::Texture>,
    /// CPU-side glyph atlas dedicated to toast text.
    atlas: GlyphAtlas,
    /// `u_viewport_size` uniform location.
    u_viewport: Option<glow::UniformLocation>,
    /// `u_atlas` uniform location.
    u_atlas: Option<glow::UniformLocation>,
}

impl Default for ToastTextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastTextRenderer {
    /// Create a new, uninitialized renderer with an empty dedicated atlas.
    ///
    /// GPU resources are created lazily on the first call to [`Self::init`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            initialized: false,
            program: None,
            vao: None,
            unit_quad_vbo: None,
            inst_vbo: [None, None],
            inst_index: 0,
            atlas_texture: None,
            atlas: GlyphAtlas::new(ATLAS_INITIAL_SIZE_PX, ATLAS_MAX_SIZE_PX),
            u_viewport: None,
            u_atlas: None,
        }
    }

    /// Return whether GPU resources have been created.
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Create all GPU resources for the toast text pass.
    ///
    /// Must be called exactly once, from within a `glow` context (e.g.
    /// inside a `PaintCallback`).
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if shader compilation/linking fails or if
    /// any GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, FG_VERT_SRC, FG_FRAG_SRC, "toast_text")?;

        let u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        let u_atlas = unsafe { gl.get_uniform_location(program, "u_atlas") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("toast_text VAO", e))?
        };
        let unit_quad_vbo = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("toast_text unit-quad VBO", e))?
        };
        let inst_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("toast_text instance VBO 0", e))?
        };
        let inst_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("toast_text instance VBO 1", e))?
        };

        // Upload the static unit quad (never changes).
        let unit_quad_bytes = unsafe {
            std::slice::from_raw_parts(
                UNIT_QUAD.as_ptr().cast::<u8>(),
                std::mem::size_of_val(&UNIT_QUAD),
            )
        };
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(unit_quad_vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, unit_quad_bytes, glow::STATIC_DRAW);
        }

        unsafe {
            gl.bind_vertex_array(Some(vao));
            setup_fg_inst_attribs(gl, unit_quad_vbo, inst_vbo0);
            gl.bind_vertex_array(None);
        }

        // Create and configure this pass's own atlas texture (mirrors
        // `TerminalRenderer::init_atlas_texture`, duplicated here because
        // that method is bound to `TerminalRenderer`'s own texture field).
        let atlas_texture = unsafe {
            gl.create_texture()
                .map_err(|e| TextureUploadError::CreateTexture {
                    label: "toast_text_atlas",
                    message: e,
                })?
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas_texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR.cast_signed(),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR.cast_signed(),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE.cast_signed(),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE.cast_signed(),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        self.program = Some(program);
        self.vao = Some(vao);
        self.unit_quad_vbo = Some(unit_quad_vbo);
        self.inst_vbo = [Some(inst_vbo0), Some(inst_vbo1)];
        self.atlas_texture = Some(atlas_texture);
        self.u_viewport = u_viewport;
        self.u_atlas = u_atlas;
        self.initialized = true;

        Ok(())
    }

    /// Measure the pixel width and (ascent+descent) height a run of text
    /// will occupy when shaped at `size_px`, **without** rasterising it into
    /// the atlas.
    ///
    /// Used by the toast layout model to size the pill to its content before
    /// any glyph is drawn. Shapes `text` the same way [`Self::build_instances`]
    /// does — resolving the whole run against the face of its first
    /// character (see the module-level single-face-per-run docs) — but reads
    /// only [`rustybuzz::GlyphPosition::x_advance`] and the face's scaled
    /// `swash` metrics, issuing no atlas insertions and no GL calls.
    ///
    /// **Must be called on the main thread** — like [`Self::build_instances`],
    /// it takes `&mut FontManager` (glyph-face resolution populates a cache)
    /// and must never be touched from a `PaintCallback`.
    ///
    /// Returns [`ToastTextMetrics::ZERO`] for empty text or a face that
    /// cannot be shaped/resolved.
    #[must_use]
    pub fn measure(
        &self,
        text: &str,
        size_px: f32,
        font_manager: &mut FontManager,
    ) -> ToastTextMetrics {
        let Some(first_char) = text.chars().next() else {
            return ToastTextMetrics::ZERO;
        };

        // Resolve the whole run against the first character's face, exactly
        // as `emit_run` does.
        let (face_id, _) = font_manager.resolve_glyph(first_char, GlyphStyle::new(false, false));

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();

        let Some(shaped) = font_manager.shape_cached(face_id, false, &[], buffer) else {
            warn!("toast_text_pass: face {face_id:?} could not shape measured run {text:?}");
            return ToastTextMetrics::ZERO;
        };

        // Font-unit → pixel scale, identical derivation to `emit_run`.
        let units_per_em_f = font_manager
            .swash_font_ref(face_id)
            .map(|font_ref| font_ref.metrics(&[]).units_per_em)
            .filter(|&upem| upem != 0)
            .map_or(1.0, f32::from);
        let font_unit_scale = size_px / units_per_em_f;

        let width: f32 = shaped
            .glyph_positions()
            .iter()
            .map(|pos| gl_f32_i32(pos.x_advance) * font_unit_scale)
            .sum();

        // Face-level ascent/descent, scaled to `size_px` directly via swash's
        // own `Metrics::scale` (handles the `units_per_em == 0` fallback
        // internally, unlike the font-unit scale derived above for advances).
        let (ascent, descent) =
            font_manager
                .swash_font_ref(face_id)
                .map_or((0.0, 0.0), |font_ref| {
                    let scaled = font_ref.metrics(&[]).scale(size_px);
                    (scaled.ascent, scaled.descent.abs())
                });

        ToastTextMetrics {
            width,
            height: ascent + descent,
            ascent,
        }
    }

    /// Shape and rasterise `runs`, returning a flat instance buffer.
    ///
    /// **Must be called on the main thread** — it takes `&mut FontManager`,
    /// which is `!Sync` and must never be touched from a `PaintCallback`.
    /// Mutates `self.atlas` (rasterising any glyphs not already cached) but
    /// issues **no GL calls**; the returned buffer is later handed to
    /// [`Self::upload_and_draw`] from the GL callback.
    ///
    /// Runs with empty text, or whose text shapes to no visible glyphs
    /// (e.g. all-whitespace), contribute no instances.
    #[must_use]
    pub fn build_instances(
        &mut self,
        runs: &[ToastTextRun],
        font_manager: &mut FontManager,
    ) -> Vec<f32> {
        let mut instances = Vec::new();
        for run in runs {
            self.emit_run(&mut instances, run, font_manager);
        }
        instances
    }

    /// Shape one [`ToastTextRun`] and append its visible glyphs' instance
    /// data to `instances`. See the module-level docs for the
    /// single-face-per-run assumption.
    fn emit_run(
        &mut self,
        instances: &mut Vec<f32>,
        run: &ToastTextRun,
        font_manager: &mut FontManager,
    ) {
        let Some(first_char) = run.text.chars().next() else {
            return;
        };

        // Resolve the whole run against the first character's face. See the
        // module-level "single-face-per-run" docs.
        let (face_id, _) = font_manager.resolve_glyph(first_char, GlyphStyle::new(false, false));

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(&run.text);
        buffer.guess_segment_properties();

        let Some(shaped) = font_manager.shape_cached(face_id, false, &[], buffer) else {
            warn!(
                "toast_text_pass: face {face_id:?} could not shape toast run {:?}",
                run.text
            );
            return;
        };

        // Rasterise at the run's own requested pixel size — toast text is
        // proportional (not locked to the terminal cell grid), so this is
        // independent of `FontManager::rasterization_ppem`.
        let size_px: u16 =
            <u16 as ApproxFrom<f32, RoundToNearest>>::approx_from(run.size_px).unwrap_or(u16::MAX);

        // `rustybuzz::Face` positions are in raw font design units (the
        // face's own `units_per_em`), not pixels — `FontManager::shape_cached`
        // builds the `rustybuzz::Face` with no ppem/scale applied (see
        // `FontManager::build_cached_face`). Recover `units_per_em` from the
        // same swash `FontRef` the atlas rasterises through, mirroring
        // `compute_cell_metrics`'s identical lookup (and its `1.0` fallback
        // for the pathological case of a font reporting `units_per_em == 0`,
        // which should not occur in practice for a valid font).
        let units_per_em_f = font_manager
            .swash_font_ref(face_id)
            .map(|font_ref| font_ref.metrics(&[]).units_per_em)
            .filter(|&upem| upem != 0)
            .map_or(1.0, f32::from);
        let font_unit_scale = run.size_px / units_per_em_f;

        let mut pen_x = run.origin_x;

        for (info, pos) in shaped
            .glyph_infos()
            .iter()
            .zip(shaped.glyph_positions().iter())
        {
            // Font-unit offsets/advances scaled into pixels via the same
            // `font_unit_scale` derived above.
            let x_offset_px = gl_f32_i32(pos.x_offset) * font_unit_scale;
            let y_offset_px = gl_f32_i32(pos.y_offset) * font_unit_scale;
            let x_advance_px = gl_f32_i32(pos.x_advance) * font_unit_scale;

            // `info.glyph_id` is a `u32` guaranteed by rustybuzz to fit in
            // `u16` (a post-shaping glyph ID, not a pre-shaping codepoint).
            let glyph_id: u16 = u16::value_from(info.glyph_id).unwrap_or(0);
            let key = GlyphKey {
                glyph_id,
                face_id,
                size_px,
            };

            if let Some(entry) = self.atlas.get_or_insert(key, font_manager)
                && entry.width != 0
                && entry.height != 0
            {
                // Pixel position: pen + font-unit offset + atlas bearing.
                // `x_offset`/`y_offset` move the pen before drawing without
                // affecting the advance (see `rustybuzz::GlyphPosition`
                // docs); `y_offset` is up-positive in font space, so it is
                // subtracted to convert into this module's top-left-origin
                // pixel space (matching `baseline_y - bearing_y` below).
                let x0 = pen_x + x_offset_px + f32::from(entry.bearing_x);
                let y0 = run.baseline_y - y_offset_px - f32::from(entry.bearing_y);
                let w = f32::from(entry.width);
                let h = f32::from(entry.height);

                instances.extend_from_slice(&pack_glyph_instance(
                    x0,
                    y0,
                    w,
                    h,
                    entry.uv_rect,
                    run.color,
                    entry.is_color,
                ));
            }

            pen_x += x_advance_px;
        }
    }

    /// Upload `instances` and draw them.
    ///
    /// **Must be called from the GL callback** (touches only `&glow::Context`
    /// plus prebuilt CPU data — no `FontManager` access). Syncs the atlas
    /// texture to the GPU (uploading any newly-rasterised glyphs from the
    /// most recent [`Self::build_instances`] call), uploads `instances` into
    /// the next double-buffer slot (orphan-then-write), and issues a single
    /// `glDrawArraysInstanced(TRIANGLES, ...)` call — matching exactly how
    /// the terminal grid's own foreground pass draws (see
    /// `TerminalRenderer::draw_foreground`).
    ///
    /// No-op (with a logged error) if [`Self::init`] has not been called
    /// yet. No-op (silently) if `instances` is empty.
    pub fn upload_and_draw(
        &mut self,
        gl: &glow::Context,
        instances: &[f32],
        viewport_w: i32,
        viewport_h: i32,
    ) {
        if !self.initialized {
            error!("ToastTextRenderer::upload_and_draw() called before init()");
            return;
        }
        if instances.is_empty() {
            return;
        }

        let (Some(prog), Some(vao), Some(unit_vbo), Some(tex)) = (
            self.program,
            self.vao,
            self.unit_quad_vbo,
            self.atlas_texture,
        ) else {
            return;
        };

        sync_toast_atlas(gl, tex, &mut self.atlas);

        let buf_idx = self.inst_index;
        let Some(inst_vbo) = self.inst_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, inst_vbo, instances);

        let instance_count = gl_i32(instances.len() / FG_INSTANCE_FLOATS);
        let vp_w = gl_f32_i32(viewport_w);
        let vp_h = gl_f32_i32(viewport_h);

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.u_atlas {
                gl.uniform_1_i32(Some(loc), 0); // TEXTURE0
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.bind_vertex_array(Some(vao));
            // Re-bind both buffers into the VAO for this draw call.
            setup_fg_inst_attribs(gl, unit_vbo, inst_vbo);
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, instance_count);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }

        self.inst_index = 1 - self.inst_index;
    }

    /// Free all GPU resources.
    ///
    /// Should be called when the widget/renderer is destroyed. Mirrors
    /// [`super::gpu::TerminalRenderer::destroy`]'s shape.
    pub fn destroy(&mut self, gl: &glow::Context) {
        if !self.initialized {
            return;
        }

        unsafe {
            if let Some(p) = self.program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.vao.take() {
                gl.delete_vertex_array(v);
            }
            if let Some(b) = self.unit_quad_vbo.take() {
                gl.delete_buffer(b);
            }
            for slot in &mut self.inst_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
            if let Some(t) = self.atlas_texture.take() {
                gl.delete_texture(t);
            }
        }

        self.initialized = false;
    }
}

// ---------------------------------------------------------------------------
//  GL upload helper (standalone, not a `TerminalRenderer` method)
// ---------------------------------------------------------------------------

/// Synchronise `atlas`'s CPU-side pixel data to `texture` on the GPU.
///
/// A standalone free function mirroring
/// [`super::gpu::TerminalRenderer::sync_atlas`], reproduced here (rather
/// than reused) because that method is private to `TerminalRenderer` and
/// bound to its own `atlas_texture` field — this pass owns a separate
/// texture and atlas entirely.
fn sync_toast_atlas(gl: &glow::Context, texture: glow::Texture, atlas: &mut GlyphAtlas) {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
    }

    let size = gl_i32_u32(atlas.size());

    if atlas.needs_full_reupload() {
        // Full upload — create or replace the entire texture.
        unsafe {
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA.cast_signed(),
                size,
                size,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(atlas.pixels())),
            );
        }
    } else {
        // Delta upload — only upload modified regions.
        for rect in atlas.take_dirty_rects() {
            let rx = gl_i32_u32(rect.x);
            let ry = gl_i32_u32(rect.y);
            let rw = gl_i32_u32(rect.width);
            let rh = gl_i32_u32(rect.height);

            let sub_pixels = extract_atlas_rect(atlas.pixels(), atlas.size(), &rect);

            unsafe {
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    rx,
                    ry,
                    rw,
                    rh,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&sub_pixels)),
                );
            }
        }
    }

    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

// ---------------------------------------------------------------------------
//  Pure CPU instance packing
// ---------------------------------------------------------------------------

/// Pack one foreground-shader glyph instance (`FG_INSTANCE_FLOATS` floats),
/// in the exact attribute order `setup_fg_inst_attribs` binds:
/// `[glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1, r, g, b, a,
/// is_color]`.
///
/// Pure and GL-free so it is fully unit-testable in isolation from atlas
/// rasterisation and shaping.
const fn pack_glyph_instance(
    x0: f32,
    y0: f32,
    w: f32,
    h: f32,
    uv_rect: [f32; 4],
    color: [f32; 4],
    is_color: bool,
) -> [f32; FG_INSTANCE_FLOATS] {
    [
        x0,
        y0,
        w,
        h,
        uv_rect[0],
        uv_rect[1],
        uv_rect[2],
        uv_rect[3],
        color[0],
        color[1],
        color[2],
        color[3],
        if is_color { 1.0 } else { 0.0 },
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    #[allow(unused_imports)] // referenced in the `measure` tests below.
    use super::{
        FG_INSTANCE_FLOATS, ToastTextMetrics, ToastTextRenderer, ToastTextRun, pack_glyph_instance,
    };
    use crate::gui::font_manager::FontManager;
    use freminal_common::config::Config;

    /// Helper: create a default `FontManager` for tests, exactly as
    /// `crate::gui::shaping`'s test suite does.
    fn test_font_manager() -> FontManager {
        FontManager::new(&Config::default(), 1.0).unwrap()
    }

    fn sample_run(text: &str) -> ToastTextRun {
        ToastTextRun {
            text: text.to_string(),
            origin_x: 5.0,
            baseline_y: 20.0,
            size_px: 16.0,
            color: [1.0, 1.0, 1.0, 1.0],
        }
    }

    // -- Renderer construction --

    #[test]
    fn new_renderer_starts_uninitialized_with_empty_atlas() {
        let renderer = ToastTextRenderer::new();
        assert!(!renderer.initialized());
        assert_eq!(renderer.atlas.entry_count(), 0);
    }

    #[test]
    fn default_matches_new() {
        let renderer = ToastTextRenderer::default();
        assert!(!renderer.initialized());
    }

    // -- pack_glyph_instance: pure float packing --

    #[test]
    fn pack_glyph_instance_orders_floats_per_fg_instance_layout() {
        let uv = [0.1, 0.2, 0.3, 0.4];
        let color = [0.5, 0.6, 0.7, 0.8];
        let packed = pack_glyph_instance(10.0, 20.0, 30.0, 40.0, uv, color, true);

        assert_eq!(packed.len(), FG_INSTANCE_FLOATS);
        assert_eq!(&packed[0..4], &[10.0, 20.0, 30.0, 40.0]);
        assert_eq!(&packed[4..8], &uv);
        assert_eq!(&packed[8..12], &color);
        assert!((packed[12] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn pack_glyph_instance_monochrome_flag_is_zero() {
        let packed = pack_glyph_instance(0.0, 0.0, 1.0, 1.0, [0.0; 4], [1.0; 4], false);
        assert!(packed[12].abs() < f32::EPSILON);
    }

    // -- build_instances: real shaping + atlas, no GL --

    #[test]
    fn build_instances_empty_run_slice_produces_no_instances() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        assert!(renderer.build_instances(&[], &mut fm).is_empty());
    }

    #[test]
    fn build_instances_empty_text_run_is_skipped() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let instances = renderer.build_instances(&[sample_run("")], &mut fm);
        assert!(instances.is_empty());
    }

    #[test]
    fn build_instances_ascii_run_produces_whole_instances() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let instances = renderer.build_instances(&[sample_run("Hi")], &mut fm);

        assert_eq!(instances.len() % FG_INSTANCE_FLOATS, 0);
        assert!(
            !instances.is_empty(),
            "'H' and 'i' should both rasterize to visible glyphs"
        );
    }

    #[test]
    fn build_instances_advances_pen_left_to_right() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        // Two identical glyphs: any nonzero advance places the second
        // strictly to the right of the first.
        let instances = renderer.build_instances(&[sample_run("II")], &mut fm);

        let n = instances.len() / FG_INSTANCE_FLOATS;
        assert_eq!(n, 2, "expected one instance per visible glyph");
        let x0 = instances[0];
        let x1 = instances[FG_INSTANCE_FLOATS];
        assert!(
            x1 > x0,
            "second glyph (x={x1}) must be to the right of the first (x={x0})"
        );
    }

    #[test]
    fn build_instances_populates_the_dedicated_atlas() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        assert_eq!(renderer.atlas.entry_count(), 0);

        let _ = renderer.build_instances(&[sample_run("A")], &mut fm);

        assert!(
            renderer.atlas.entry_count() > 0,
            "shaping a visible glyph must rasterise it into this pass's own atlas"
        );
    }

    #[test]
    fn build_instances_multiple_runs_all_contribute() {
        let mut renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let runs = [sample_run("Hi"), sample_run("Bye")];
        let instances = renderer.build_instances(&runs, &mut fm);

        assert_eq!(instances.len() % FG_INSTANCE_FLOATS, 0);
        // At least one instance per run, generously.
        assert!(instances.len() / FG_INSTANCE_FLOATS >= 2);
    }

    // -- measure: pure metrics, no atlas insertion --

    #[test]
    fn measure_empty_text_is_zero() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let m = renderer.measure("", 16.0, &mut fm);
        assert_eq!(m, ToastTextMetrics::ZERO);
    }

    #[test]
    fn measure_longer_string_is_wider() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let short = renderer.measure("Hi", 16.0, &mut fm);
        let long = renderer.measure("Hello, world!", 16.0, &mut fm);
        assert!(
            long.width > short.width,
            "longer string ({}) must measure wider than shorter string ({})",
            long.width,
            short.width
        );
    }

    #[test]
    fn measure_does_not_populate_the_atlas() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        assert_eq!(renderer.atlas.entry_count(), 0);

        let m = renderer.measure("Hello", 16.0, &mut fm);

        assert!(
            m.width > 0.0,
            "non-empty text should measure a nonzero width"
        );
        assert_eq!(
            renderer.atlas.entry_count(),
            0,
            "measure() must not rasterise glyphs into the atlas"
        );
    }

    #[test]
    fn measure_reports_positive_height_and_ascent_for_visible_text() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let m = renderer.measure("A", 16.0, &mut fm);
        assert!(m.height > 0.0);
        assert!(m.ascent > 0.0);
        assert!(m.ascent <= m.height);
    }

    #[test]
    fn measure_scales_with_size() {
        let renderer = ToastTextRenderer::new();
        let mut fm = test_font_manager();
        let small = renderer.measure("Hi", 12.0, &mut fm);
        let large = renderer.measure("Hi", 24.0, &mut fm);
        assert!(large.width > small.width);
        assert!(large.height > small.height);
    }
}
