// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GL shader programs, vertex buffers, and draw calls for the terminal renderer.
//!
//! [`TerminalRenderer`] owns all GPU state for the custom terminal rendering
//! pipeline: six shader programs (decoration, instanced background, foreground,
//! image, background image, post-processing), VAOs, double-buffered VBOs, the
//! atlas GL texture, an optional background image texture, and an optional
//! offscreen FBO for custom post-processing shaders.
//!
//! Rendering is triggered via egui's [`egui_glow::CallbackFn`] mechanism.
//! The CPU-side instance/vertex builders live in [`super::vertex`] and are pure
//! functions that are fully testable without a GL context.

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use glow::{self, HasContext};
use tracing::error;

use super::super::atlas::GlyphAtlas;
use super::errors::{BufferAllocError, GpuInitError, ShaderCompileError, TextureUploadError};
use super::shaders::{
    BG_IMG_FRAG_SRC, BG_IMG_VERT_SRC, BG_INST_FRAG_SRC, BG_INST_VERT_SRC, DECO_FRAG_SRC,
    DECO_VERT_SRC, FG_FRAG_SRC, FG_VERT_SRC, IMG_FRAG_SRC, IMG_VERT_SRC, POST_PASSTHROUGH_FRAG_SRC,
    POST_VERT_SRC,
};
use super::vertex::{
    BG_INSTANCE_FLOATS, CURSOR_QUAD_FLOATS, DECO_VERTEX_FLOATS, FG_INSTANCE_FLOATS,
    IMG_VERTEX_FLOATS, VERTS_PER_QUAD, extract_atlas_rect,
};
use freminal_terminal_emulator::InlineImage;

// ---------------------------------------------------------------------------
//  GL numeric conversion helpers
// ---------------------------------------------------------------------------
//
// The OpenGL API (`glow`) requires `i32` for vertex counts, strides, and byte
// offsets, and `f32` for viewport dimensions, coordinate math, and uniforms.
// These helpers centralise the checked conversions so call sites stay concise.
//
// Fallback rationale: terminal dimensions and buffer sizes cannot realistically
// overflow `i32` (max ≈ 2 billion) or lose significant `f32` precision (24-bit
// mantissa ≫ any terminal cell count), so the fallback paths are purely
// defensive.

/// Convert a `usize` to `i32` for OpenGL counts, strides, or byte offsets.
/// Returns `0` on overflow (astronomically unlikely for terminal dimensions)
/// and logs an error so the impossible is visible if it ever occurs.
#[inline]
fn gl_i32(val: usize) -> i32 {
    i32::value_from(val).unwrap_or_else(|_| {
        error!("gl_i32: usize {val} overflows i32");
        0
    })
}

/// Convert a `u32` to `i32` for OpenGL texture dimensions.
/// Returns `0` on overflow (texture sizes are always well within `i32` range)
/// and logs an error so the impossible is visible if it ever occurs.
#[inline]
fn gl_i32_u32(val: u32) -> i32 {
    i32::value_from(val).unwrap_or_else(|_| {
        error!("gl_i32_u32: u32 {val} overflows i32");
        0
    })
}

/// Convert a `u32` to `f32` for GPU cell-dimension math.
/// Returns `0.0` on precision loss (u32 values fit in f32 for all sane sizes)
/// and logs an error so the impossible is visible if it ever occurs.
#[inline]
#[allow(dead_code)]
fn gl_f32_u32(val: u32) -> f32 {
    f32::approx_from(val).unwrap_or_else(|_| {
        error!("gl_f32_u32: u32 {val} cannot be approximated as f32");
        0.0
    })
}

/// Convert an `i32` to `f32` for GPU viewport uniforms.
/// Returns `0.0` on precision loss (viewport sizes are always small)
/// and logs an error so the impossible is visible if it ever occurs.
#[inline]
fn gl_f32_i32(val: i32) -> f32 {
    f32::approx_from(val).unwrap_or_else(|_| {
        error!("gl_f32_i32: i32 {val} cannot be approximated as f32");
        0.0
    })
}

// ---------------------------------------------------------------------------
//  Constants
// ---------------------------------------------------------------------------

/// Static unit quad for instanced background rendering (2 triangles = 6 vertices).
///
/// Each vertex is `vec2 a_pos` in [0,1]² space.  The vertex shader scales by
/// `(u_cell_width, u_cell_height)` and offsets by the per-instance cell position.
const UNIT_QUAD: [f32; 12] = [
    0.0, 0.0, 1.0, 0.0, 0.0, 1.0, // triangle 1
    1.0, 0.0, 1.0, 1.0, 0.0, 1.0, // triangle 2
];

// ---------------------------------------------------------------------------
//  TerminalRenderer
// ---------------------------------------------------------------------------

/// Holds all GPU resources for the custom terminal rendering pipeline.
///
/// Call [`TerminalRenderer::init`] once (inside the first `PaintCallback`
/// invocation) to create shaders, VAOs, VBOs, and the atlas texture.  Then call
/// [`TerminalRenderer::draw_with_verts`] every frame.
pub struct TerminalRenderer {
    /// Whether GPU resources have been created.
    initialized: bool,

    // ---- instanced background pass ----
    bg_inst_program: Option<glow::Program>,
    bg_inst_vao: Option<glow::VertexArray>,
    /// Static unit-quad VBO (uploaded once, never changes).
    bg_unit_quad_vbo: Option<glow::Buffer>,
    /// Double-buffered instance VBOs for per-cell background data.
    bg_inst_vbo: [Option<glow::Buffer>; 2],

    // ---- decoration pass (underline, strikethrough, cursor, selection) ----
    deco_program: Option<glow::Program>,
    deco_vao: Option<glow::VertexArray>,
    deco_vbo: [Option<glow::Buffer>; 2],

    // ---- foreground pass ----
    fg_program: Option<glow::Program>,
    fg_vao: Option<glow::VertexArray>,
    fg_vbo: [Option<glow::Buffer>; 2],

    // ---- atlas texture ----
    atlas_texture: Option<glow::Texture>,

    // ---- image pass ----
    img_program: Option<glow::Program>,
    img_vao: Option<glow::VertexArray>,
    img_vbo: [Option<glow::Buffer>; 2],
    /// Per-image GL textures, keyed by `InlineImage::id`.
    ///
    /// Populated on first use and evicted when the image is no longer visible.
    image_textures: std::collections::HashMap<u64, glow::Texture>,

    // ---- background image pass ----
    /// Shader program for the background image quad.
    bg_img_program: Option<glow::Program>,
    /// VAO for the background image fullscreen quad.
    bg_img_vao: Option<glow::VertexArray>,
    /// VBO for the background image quad vertices (6 × vec2+vec2 = 24 f32s).
    bg_img_vbo: Option<glow::Buffer>,
    /// Uploaded background image GL texture.
    bg_img_texture: Option<glow::Texture>,
    /// Size (width, height) of the uploaded background image texture, in pixels.
    bg_img_size: Option<(u32, u32)>,
    // Background image uniform locations.
    bg_img_u_viewport: Option<glow::UniformLocation>,
    bg_img_u_image: Option<glow::UniformLocation>,
    bg_img_u_opacity: Option<glow::UniformLocation>,

    // ---- uniform locations ----
    // instanced background
    bg_inst_u_viewport: Option<glow::UniformLocation>,
    bg_inst_u_cell_width: Option<glow::UniformLocation>,
    bg_inst_u_cell_height: Option<glow::UniformLocation>,
    bg_inst_u_bg_opacity: Option<glow::UniformLocation>,
    // decorations
    deco_u_viewport: Option<glow::UniformLocation>,
    // foreground
    fg_u_viewport: Option<glow::UniformLocation>,
    fg_u_atlas: Option<glow::UniformLocation>,
    // images
    img_u_viewport: Option<glow::UniformLocation>,
    img_u_image: Option<glow::UniformLocation>,

    // ---- double-buffer index ----
    vbo_index: usize,
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalRenderer {
    /// Create a new, uninitialized renderer.
    ///
    /// GPU resources are created lazily on the first call to [`Self::init`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            initialized: false,
            // instanced background
            bg_inst_program: None,
            bg_inst_vao: None,
            bg_unit_quad_vbo: None,
            bg_inst_vbo: [None, None],
            // decoration
            deco_program: None,
            deco_vao: None,
            deco_vbo: [None, None],
            // foreground
            fg_program: None,
            fg_vao: None,
            fg_vbo: [None, None],
            atlas_texture: None,
            // images
            img_program: None,
            img_vao: None,
            img_vbo: [None, None],
            image_textures: std::collections::HashMap::new(),
            // background image
            bg_img_program: None,
            bg_img_vao: None,
            bg_img_vbo: None,
            bg_img_texture: None,
            bg_img_size: None,
            bg_img_u_viewport: None,
            bg_img_u_image: None,
            bg_img_u_opacity: None,
            // uniform locations
            bg_inst_u_viewport: None,
            bg_inst_u_cell_width: None,
            bg_inst_u_cell_height: None,
            bg_inst_u_bg_opacity: None,
            deco_u_viewport: None,
            fg_u_viewport: None,
            fg_u_atlas: None,
            img_u_viewport: None,
            img_u_image: None,
            vbo_index: 0,
        }
    }

    /// Return whether GPU resources have been created.
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Create all GPU resources.
    ///
    /// Must be called exactly once, from within a `glow` context (e.g. inside a
    /// `PaintCallback` or `CreationContext::gl`).
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if shader compilation/linking fails or if any
    /// GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        self.init_bg_inst_pass(gl)?;
        self.init_deco_pass(gl)?;
        self.init_fg_pass(gl)?;
        self.init_atlas_texture(gl)?;
        self.init_image_pass(gl)?;
        self.init_bg_image_pass(gl)?;

        self.initialized = true;
        Ok(())
    }

    /// Initialise the instanced background pass (shader, VAO, unit-quad VBO,
    /// double-buffered instance VBOs).
    fn init_bg_inst_pass(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, BG_INST_VERT_SRC, BG_INST_FRAG_SRC, "bg_instanced")?;

        self.bg_inst_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        self.bg_inst_u_cell_width = unsafe { gl.get_uniform_location(program, "u_cell_width") };
        self.bg_inst_u_cell_height = unsafe { gl.get_uniform_location(program, "u_cell_height") };
        self.bg_inst_u_bg_opacity = unsafe { gl.get_uniform_location(program, "u_bg_opacity") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("bg_inst VAO", e))?
        };
        let unit_quad_vbo = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("bg unit-quad VBO", e))?
        };
        let inst_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("bg instance VBO 0", e))?
        };
        let inst_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("bg instance VBO 1", e))?
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

        // Configure the instanced VAO.
        unsafe {
            gl.bind_vertex_array(Some(vao));
            setup_bg_inst_attribs(gl, unit_quad_vbo, inst_vbo0);
            gl.bind_vertex_array(None);
        }

        self.bg_inst_program = Some(program);
        self.bg_inst_vao = Some(vao);
        self.bg_unit_quad_vbo = Some(unit_quad_vbo);
        self.bg_inst_vbo = [Some(inst_vbo0), Some(inst_vbo1)];

        Ok(())
    }

    /// Initialise the decoration pass (shader, VAO, double-buffered VBOs).
    fn init_deco_pass(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, DECO_VERT_SRC, DECO_FRAG_SRC, "decoration")?;

        self.deco_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("deco VAO", e))?
        };
        let vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("deco VBO 0", e))?
        };
        let vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("deco VBO 1", e))?
        };

        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo0));
            setup_deco_attribs(gl);
            gl.bind_vertex_array(None);
        }

        self.deco_program = Some(program);
        self.deco_vao = Some(vao);
        self.deco_vbo = [Some(vbo0), Some(vbo1)];

        Ok(())
    }

    /// Initialise the foreground pass (shader, VAO, double-buffered instance VBOs).
    ///
    /// Reuses the shared unit-quad VBO from the instanced background pass
    /// (must be initialised first via [`init_bg_inst_pass`]).
    fn init_fg_pass(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, FG_VERT_SRC, FG_FRAG_SRC, "foreground")?;

        self.fg_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        self.fg_u_atlas = unsafe { gl.get_uniform_location(program, "u_atlas") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("foreground VAO", e))?
        };
        let vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("foreground instance VBO 0", e))?
        };
        let vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("foreground instance VBO 1", e))?
        };

        // The unit-quad VBO must already exist (created by init_bg_inst_pass).
        let unit_quad_vbo = self.bg_unit_quad_vbo.ok_or_else(|| {
            BufferAllocError::new(
                "foreground unit-quad VBO reference",
                "FG init: unit-quad VBO not yet created (call init_bg_inst_pass first)",
            )
        })?;

        unsafe {
            gl.bind_vertex_array(Some(vao));
            setup_fg_inst_attribs(gl, unit_quad_vbo, vbo0);
            gl.bind_vertex_array(None);
        }

        self.fg_program = Some(program);
        self.fg_vao = Some(vao);
        self.fg_vbo = [Some(vbo0), Some(vbo1)];

        Ok(())
    }

    /// Create and configure the glyph-atlas texture.
    fn init_atlas_texture(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let texture = unsafe {
            gl.create_texture()
                .map_err(|e| TextureUploadError::CreateTexture {
                    label: "atlas",
                    message: e,
                })?
        };

        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
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

        self.atlas_texture = Some(texture);

        Ok(())
    }

    /// Initialise the image-pass GL resources (shader, VAO, double-buffered VBOs).
    fn init_image_pass(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let img_program = compile_program(gl, IMG_VERT_SRC, IMG_FRAG_SRC, "image")?;

        let img_u_viewport = unsafe { gl.get_uniform_location(img_program, "u_viewport_size") };
        let img_u_image = unsafe { gl.get_uniform_location(img_program, "u_image") };

        let img_vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("image VAO", e))?
        };
        let img_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("image VBO 0", e))?
        };
        let img_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("image VBO 1", e))?
        };

        unsafe {
            gl.bind_vertex_array(Some(img_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(img_vbo0));
            setup_img_attribs(gl);
            gl.bind_vertex_array(None);
        }

        self.img_program = Some(img_program);
        self.img_vao = Some(img_vao);
        self.img_vbo = [Some(img_vbo0), Some(img_vbo1)];
        self.img_u_viewport = img_u_viewport;
        self.img_u_image = img_u_image;

        Ok(())
    }

    /// Initialise the background image pass (shader, VAO, VBO).
    ///
    /// No texture is uploaded here — images are loaded on demand via
    /// [`Self::update_background_image`].
    fn init_bg_image_pass(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, BG_IMG_VERT_SRC, BG_IMG_FRAG_SRC, "bg_image")?;

        self.bg_img_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        self.bg_img_u_image = unsafe { gl.get_uniform_location(program, "u_bg_image") };
        self.bg_img_u_opacity = unsafe { gl.get_uniform_location(program, "u_opacity") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("bg_image VAO", e))?
        };
        let vbo = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("bg_image VBO", e))?
        };

        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_img_attribs(gl);
            gl.bind_vertex_array(None);
        }

        self.bg_img_program = Some(program);
        self.bg_img_vao = Some(vao);
        self.bg_img_vbo = Some(vbo);

        Ok(())
    }

    /// Render a terminal frame from pre-built vertex buffers.
    ///
    /// Used when the vertex buffers were built on the main thread (where
    /// [`super::super::font_manager::FontManager`] is available) before being passed into the
    /// `PaintCallback` closure, which must be `Send + Sync` and therefore
    /// cannot capture a `FontManager`.
    ///
    /// # Safety
    ///
    /// This method calls `glow` functions which are marked `unsafe`.  The
    /// caller is responsible for ensuring a valid GL context exists.
    // All parameters are required GPU render inputs: vertex data, uniforms, dimensions, and
    // flags. Grouping into a struct would not reduce the OpenGL call complexity.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with_verts(
        &mut self,
        gl: &glow::Context,
        atlas: &mut GlyphAtlas,
        bg_instances: &[f32],
        deco_verts: &[f32],
        fg_instances: &[f32],
        image_verts: &[f32],
        snap_images: &std::collections::HashMap<u64, InlineImage>,
        viewport_width: i32,
        viewport_height: i32,
        cell_width: f32,
        cell_height: f32,
        bg_opacity: f32,
        bg_image_opacity: f32,
        bg_image_mode: freminal_common::config::BackgroundImageMode,
        intermediate_fbo: Option<glow::Framebuffer>,
    ) {
        if !self.initialized {
            error!("TerminalRenderer::draw_with_verts() called before init()");
            return;
        }

        // 1. Sync atlas texture to the GPU.
        self.sync_atlas(gl, atlas);

        // 1b. Sync image textures (upload new, evict stale).
        self.sync_image_textures(gl, snap_images);

        // 2. Upload pre-built vertex data using orphan-then-write.
        let buf_idx = self.vbo_index;
        self.upload_bg_instances(gl, bg_instances, buf_idx);
        self.upload_deco_verts(gl, deco_verts, buf_idx);
        self.upload_fg_instances(gl, fg_instances, buf_idx);
        self.upload_img_verts(gl, image_verts, buf_idx);

        // 3. Draw in order: bg image → cell backgrounds → decorations → foreground → images.
        let vp_w = gl_f32_i32(viewport_width);
        let vp_h = gl_f32_i32(viewport_height);

        // 3a. Background image (drawn behind everything).
        self.draw_background_image(gl, vp_w, vp_h, bg_image_opacity, bg_image_mode);

        // 3b. Cell backgrounds, decorations, foreground glyphs, inline images.
        self.draw_background_instanced(
            gl,
            bg_instances.len(),
            vp_w,
            vp_h,
            cell_width,
            cell_height,
            bg_opacity,
            buf_idx,
        );
        self.draw_decorations(gl, deco_verts.len(), vp_w, vp_h, buf_idx);
        self.draw_foreground(gl, fg_instances.len(), vp_w, vp_h, buf_idx);
        self.draw_images(gl, image_verts.len(), snap_images, vp_w, vp_h, buf_idx);

        // 4. Restore egui's framebuffer binding.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, intermediate_fbo);
        }

        // Advance double-buffer index.
        self.vbo_index = 1 - self.vbo_index;
    }

    /// Render a cursor-only update.
    ///
    /// When the terminal content has not changed but the cursor blink state
    /// has toggled (or the cursor moved), this method patches only the cursor
    /// quad region of the decoration VBO and redraws all passes.  The
    /// foreground VBO and instanced background VBO are untouched.
    ///
    /// `cursor_vert_byte_offset` is the byte offset into the decoration VBO
    /// where the cursor quad data begins.  `deco_total_floats` is the total
    /// float count of the most recently uploaded decoration VBO (needed to
    /// set the draw vertex count correctly).  `cursor_verts` contains exactly
    /// `CURSOR_QUAD_FLOATS` floats (or is empty when the cursor is hidden).
    ///
    /// # Safety
    ///
    /// Caller must ensure a valid GL context exists.
    // Same rationale as `draw_with_verts`: all parameters are required GPU render inputs.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with_cursor_only_update(
        &mut self,
        gl: &glow::Context,
        atlas: &mut GlyphAtlas,
        cursor_vert_byte_offset: usize,
        deco_total_floats: usize,
        bg_inst_total_floats: usize,
        cursor_verts: &[f32],
        fg_total_floats: usize,
        image_total_floats: usize,
        snap_images: &std::collections::HashMap<u64, InlineImage>,
        viewport_width: i32,
        viewport_height: i32,
        cell_width: f32,
        cell_height: f32,
        bg_opacity: f32,
        bg_image_opacity: f32,
        bg_image_mode: freminal_common::config::BackgroundImageMode,
        intermediate_fbo: Option<glow::Framebuffer>,
    ) {
        if !self.initialized {
            error!("TerminalRenderer::draw_with_cursor_only_update() called before init()");
            return;
        }

        // 1. Sync atlas (may have new glyphs from a previous frame).
        self.sync_atlas(gl, atlas);

        // Use the slot that was last fully written by `draw_with_verts`.
        // After a full frame, `draw_with_verts` advances `vbo_index` to the
        // *next* slot.  The cursor-only path patches and draws from the
        // *previous* slot (the one with valid data).
        let buf_idx = 1 - self.vbo_index;

        // 2. Patch just the cursor region of the deco VBO (no orphan).
        if cursor_verts.is_empty() {
            // Cursor is hidden: zero out the cursor quad region so no stale
            // cursor is painted.  We write CURSOR_QUAD_FLOATS zeros.
            if let Some(vbo) = self.deco_vbo[buf_idx] {
                let zeros = vec![0.0f32; CURSOR_QUAD_FLOATS];
                upload_verts_sub(gl, vbo, cursor_vert_byte_offset, &zeros);
            }
        } else if let Some(vbo) = self.deco_vbo[buf_idx] {
            upload_verts_sub(gl, vbo, cursor_vert_byte_offset, cursor_verts);
        }

        // 3. Draw in order: bg image → cell backgrounds → decorations → foreground → images.
        let vp_w = gl_f32_i32(viewport_width);
        let vp_h = gl_f32_i32(viewport_height);

        self.draw_background_image(gl, vp_w, vp_h, bg_image_opacity, bg_image_mode);
        self.draw_background_instanced(
            gl,
            bg_inst_total_floats,
            vp_w,
            vp_h,
            cell_width,
            cell_height,
            bg_opacity,
            buf_idx,
        );
        self.draw_decorations(gl, deco_total_floats, vp_w, vp_h, buf_idx);
        self.draw_foreground(gl, fg_total_floats, vp_w, vp_h, buf_idx);
        self.draw_images(gl, image_total_floats, snap_images, vp_w, vp_h, buf_idx);

        // 4. Restore egui's framebuffer binding.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, intermediate_fbo);
        }

        // Do NOT advance the double-buffer index: we reused the same buffer
        // slot this frame (no full orphan).  The next full-frame draw will
        // advance normally.
    }

    /// Synchronise the atlas CPU data to the GPU texture.
    fn sync_atlas(&self, gl: &glow::Context, atlas: &mut GlyphAtlas) {
        let Some(tex) = self.atlas_texture else {
            return;
        };

        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
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

                // Build the sub-image pixel slice for this rect.
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

    /// Upload decoration vertex data via orphan-then-write.
    fn upload_deco_verts(&self, gl: &glow::Context, verts: &[f32], buf_idx: usize) {
        let Some(vbo) = self.deco_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, verts);
    }

    /// Upload instanced background instance data via orphan-then-write.
    fn upload_bg_instances(&self, gl: &glow::Context, instances: &[f32], buf_idx: usize) {
        let Some(vbo) = self.bg_inst_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, instances);
    }

    /// Upload foreground instance data via orphan-then-write.
    fn upload_fg_instances(&self, gl: &glow::Context, instances: &[f32], buf_idx: usize) {
        let Some(vbo) = self.fg_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, instances);
    }

    /// Upload image vertex data via orphan-then-write.
    fn upload_img_verts(&self, gl: &glow::Context, verts: &[f32], buf_idx: usize) {
        let Some(vbo) = self.img_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, verts);
    }

    /// Synchronise the set of image GL textures with the current snapshot's
    /// image map.
    ///
    /// - New images (ID not yet in `self.image_textures`) are uploaded.
    /// - Stale images (ID in `self.image_textures` but not in `snap_images`)
    ///   are deleted.
    fn sync_image_textures(
        &mut self,
        gl: &glow::Context,
        snap_images: &std::collections::HashMap<u64, InlineImage>,
    ) {
        // Delete textures for images no longer in the visible snapshot.
        self.image_textures.retain(|id, tex| {
            if snap_images.contains_key(id) {
                true
            } else {
                unsafe { gl.delete_texture(*tex) };
                false
            }
        });

        // Upload textures for new images.
        for (id, img) in snap_images {
            if self.image_textures.contains_key(id) {
                continue; // Already uploaded.
            }
            let tex = unsafe {
                match gl.create_texture() {
                    Ok(t) => t,
                    Err(e) => {
                        error!("create image texture {id}: {e}");
                        continue;
                    }
                }
            };

            let w = gl_i32_u32(img.width_px);
            let h = gl_i32_u32(img.height_px);

            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
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
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA.cast_signed(),
                    w,
                    h,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&img.pixels)),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            }

            self.image_textures.insert(*id, tex);
        }
    }

    /// Execute the image draw call.
    ///
    /// Draws one textured quad per image that has vertices in `image_verts`.
    /// Iterates images in the order they appear (by ID from the map); each
    /// image is bound to `TEXTURE1` and drawn with the corresponding 6-vertex
    /// slab from `image_verts`.
    // All parameters are required GPU context and data. Image rendering requires separate
    // texture binding, program, and geometry inputs that cannot be sensibly grouped.
    #[allow(clippy::too_many_arguments)]
    fn draw_images(
        &self,
        gl: &glow::Context,
        vert_floats: usize,
        snap_images: &std::collections::HashMap<u64, InlineImage>,
        vp_w: f32,
        vp_h: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(vbo)) =
            (self.img_program, self.img_vao, self.img_vbo[buf_idx])
        else {
            return;
        };

        if vert_floats == 0 {
            return;
        }

        // How many quads do we have in the buffer?
        let total_quads = vert_floats / (VERTS_PER_QUAD * IMG_VERTEX_FLOATS);
        if total_quads == 0 {
            return;
        }

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.img_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.img_u_image {
                gl.uniform_1_i32(Some(loc), 1); // TEXTURE1
            }
            gl.active_texture(glow::TEXTURE1);
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_img_attribs(gl);
        }

        // Draw one quad (6 vertices) per image, in snap_images iteration order.
        // `build_image_verts` emits quads in the same order (sorted by image ID).
        let mut quad_idx: i32 = 0;
        let mut sorted_ids: Vec<u64> = snap_images.keys().copied().collect();
        sorted_ids.sort_unstable();
        for id in &sorted_ids {
            let Some(tex) = self.image_textures.get(id) else {
                // Texture not uploaded yet (race) — skip.
                quad_idx += 1;
                continue;
            };
            if quad_idx >= gl_i32(total_quads) {
                break;
            }
            let first_vertex = quad_idx * gl_i32(VERTS_PER_QUAD);
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(*tex));
                gl.draw_arrays(glow::TRIANGLES, first_vertex, gl_i32(VERTS_PER_QUAD));
            }
            quad_idx += 1;
        }

        unsafe {
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);
        }
    }

    /// Execute the instanced background draw call.
    ///
    /// Each instance is one cell-sized quad; the instance buffer provides the
    /// grid position and resolved RGBA color.  `u_bg_opacity` is applied in the
    /// fragment shader.
    // All parameters are required GPU render context: program, VAO, instance data, uniforms,
    // and dimensions. No subset forms a coherent intermediate abstraction.
    #[allow(clippy::too_many_arguments)]
    fn draw_background_instanced(
        &self,
        gl: &glow::Context,
        instance_floats: usize,
        vp_w: f32,
        vp_h: f32,
        cell_width: f32,
        cell_height: f32,
        bg_opacity: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(unit_vbo), Some(inst_vbo)) = (
            self.bg_inst_program,
            self.bg_inst_vao,
            self.bg_unit_quad_vbo,
            self.bg_inst_vbo[buf_idx],
        ) else {
            return;
        };

        let instance_count = instance_floats / BG_INSTANCE_FLOATS;
        if instance_count == 0 {
            return;
        }

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.bg_inst_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.bg_inst_u_cell_width {
                gl.uniform_1_f32(Some(loc), cell_width);
            }
            if let Some(loc) = &self.bg_inst_u_cell_height {
                gl.uniform_1_f32(Some(loc), cell_height);
            }
            if let Some(loc) = &self.bg_inst_u_bg_opacity {
                gl.uniform_1_f32(Some(loc), bg_opacity);
            }
            gl.bind_vertex_array(Some(vao));
            // Re-bind both buffers into the VAO for this draw call.
            setup_bg_inst_attribs(gl, unit_vbo, inst_vbo);
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, gl_i32(instance_count));
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Execute the decoration draw call (underlines, strikethrough, cursor,
    /// selection highlights).
    fn draw_decorations(
        &self,
        gl: &glow::Context,
        vert_floats: usize,
        vp_w: f32,
        vp_h: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(vbo)) =
            (self.deco_program, self.deco_vao, self.deco_vbo[buf_idx])
        else {
            return;
        };

        if vert_floats == 0 {
            return;
        }

        let vertex_count = gl_i32(vert_floats / DECO_VERTEX_FLOATS);

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.deco_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_deco_attribs(gl);
            gl.draw_arrays(glow::TRIANGLES, 0, vertex_count);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Execute the instanced foreground draw call.
    fn draw_foreground(
        &self,
        gl: &glow::Context,
        instance_floats: usize,
        vp_w: f32,
        vp_h: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(unit_vbo), Some(inst_vbo), Some(tex)) = (
            self.fg_program,
            self.fg_vao,
            self.bg_unit_quad_vbo,
            self.fg_vbo[buf_idx],
            self.atlas_texture,
        ) else {
            return;
        };

        let instance_count = instance_floats / FG_INSTANCE_FLOATS;
        if instance_count == 0 {
            return;
        }

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.fg_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.fg_u_atlas {
                gl.uniform_1_i32(Some(loc), 0); // TEXTURE0
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.bind_vertex_array(Some(vao));
            // Re-bind both buffers into the VAO for this draw call.
            setup_fg_inst_attribs(gl, unit_vbo, inst_vbo);
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, gl_i32(instance_count));
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }

    /// Load (or replace) the background image texture from a file path.
    ///
    /// Decodes the image with the `image` crate (supports PNG, JPEG, WebP,
    /// and GIF), converts to RGBA8, and uploads to a GL texture.
    /// Any previously loaded texture is deleted first.
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if the file cannot be read or decoded, or if
    /// a GL texture object cannot be created.
    pub fn update_background_image(
        &mut self,
        gl: &glow::Context,
        path: &std::path::Path,
    ) -> Result<(), GpuInitError> {
        // Delete any previously-loaded texture.
        self.clear_background_image(gl);

        // Decode the image file.
        let img = image::open(path)
            .map_err(|e| TextureUploadError::ImageDecode {
                path: path.to_path_buf(),
                source: e,
            })?
            .into_rgba8();
        let (w, h) = (img.width(), img.height());
        let pixels = img.into_raw();

        // Upload to GL.
        let tex = unsafe {
            gl.create_texture()
                .map_err(|e| TextureUploadError::CreateTexture {
                    label: "bg_img",
                    message: e,
                })?
        };

        let wi = gl_i32_u32(w);
        let hi = gl_i32_u32(h);

        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
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
            // REPEAT so tile mode works; clamped modes stay in [0,1] UV anyway.
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::REPEAT.cast_signed(),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::REPEAT.cast_signed(),
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA.cast_signed(),
                wi,
                hi,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&pixels)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        self.bg_img_texture = Some(tex);
        self.bg_img_size = Some((w, h));
        Ok(())
    }

    /// Delete the current background image texture.
    ///
    /// After this call, [`draw_background_image`](Self::draw_background_image)
    /// is a no-op until a new image is loaded.
    pub fn clear_background_image(&mut self, gl: &glow::Context) {
        if let Some(tex) = self.bg_img_texture.take() {
            unsafe { gl.delete_texture(tex) };
        }
        self.bg_img_size = None;
    }

    /// Draw the background image quad.
    ///
    /// UV coordinates are computed from the fit mode:
    /// - **Fill**: stretch to fill the viewport (UV [0,1]² — may distort).
    /// - **Fit**: letterbox/pillarbox to preserve aspect ratio.
    /// - **Cover**: crop to fill viewport without distortion (center-crop).
    /// - **Tile**: repeat at native pixel scale.
    ///
    /// Skips if no texture is loaded (`bg_img_texture` is `None`).
    fn draw_background_image(
        &self,
        gl: &glow::Context,
        vp_w: f32,
        vp_h: f32,
        opacity: f32,
        mode: freminal_common::config::BackgroundImageMode,
    ) {
        let (Some(prog), Some(vao), Some(vbo), Some(tex), Some((img_w, img_h))) = (
            self.bg_img_program,
            self.bg_img_vao,
            self.bg_img_vbo,
            self.bg_img_texture,
            self.bg_img_size,
        ) else {
            return;
        };

        // Compute quad position and UV rectangle based on fit mode.
        let geom = compute_bg_uvs(vp_w, vp_h, img_w, img_h, mode);
        let (x0, y0, x1, y1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;

        // Background image quad (pixel coords for position, texture coords for UV).
        // Layout: vec2 pos (pixels), vec2 uv — 6 vertices (2 triangles).
        #[rustfmt::skip]
        let quad: [f32; 24] = [
            // pos (pixels)      uv
            x0,  y0,             u0, v0,
            x1,  y0,             u1, v0,
            x0,  y1,             u0, v1,
            x1,  y0,             u1, v0,
            x1,  y1,             u1, v1,
            x0,  y1,             u0, v1,
        ];

        let bytes = unsafe {
            std::slice::from_raw_parts(quad.as_ptr().cast::<u8>(), std::mem::size_of_val(&quad))
        };

        unsafe {
            // Upload quad vertices.
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            gl.use_program(Some(prog));
            if let Some(loc) = &self.bg_img_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.bg_img_u_opacity {
                gl.uniform_1_f32(Some(loc), opacity);
            }
            if let Some(loc) = &self.bg_img_u_image {
                gl.uniform_1_i32(Some(loc), 2); // TEXTURE2
            }

            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_img_attribs(gl);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);
        }
    }

    /// Free all GPU resources.
    ///
    /// Should be called when the widget is destroyed.
    pub fn destroy(&mut self, gl: &glow::Context) {
        if !self.initialized {
            return;
        }

        unsafe {
            // Instanced background resources.
            if let Some(p) = self.bg_inst_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.bg_inst_vao.take() {
                gl.delete_vertex_array(v);
            }
            if let Some(b) = self.bg_unit_quad_vbo.take() {
                gl.delete_buffer(b);
            }
            for slot in &mut self.bg_inst_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
            // Decoration resources.
            if let Some(p) = self.deco_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.deco_vao.take() {
                gl.delete_vertex_array(v);
            }
            for slot in &mut self.deco_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
            // Foreground resources.
            if let Some(p) = self.fg_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.fg_vao.take() {
                gl.delete_vertex_array(v);
            }
            for slot in &mut self.fg_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
            if let Some(t) = self.atlas_texture.take() {
                gl.delete_texture(t);
            }
            // Image resources.
            if let Some(p) = self.img_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.img_vao.take() {
                gl.delete_vertex_array(v);
            }
            for slot in &mut self.img_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
            for tex in self.image_textures.drain() {
                gl.delete_texture(tex.1);
            }
            // Background image resources.
            if let Some(p) = self.bg_img_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.bg_img_vao.take() {
                gl.delete_vertex_array(v);
            }
            if let Some(b) = self.bg_img_vbo.take() {
                gl.delete_buffer(b);
            }
            if let Some(t) = self.bg_img_texture.take() {
                gl.delete_texture(t);
            }
        }

        self.initialized = false;
    }
}

// ---------------------------------------------------------------------------
//  Vertex attribute setup helpers
// ---------------------------------------------------------------------------

/// Configure vertex attributes for the instanced background shader.
///
/// Binds the static unit-quad VBO to location 0 (per-vertex, divisor 0)
/// and the instance VBO to locations 1–2 (per-instance, divisor 1).
///
/// - Location 0: `vec2 a_pos`       (unit quad)      — divisor 0
/// - Location 1: `vec2 a_cell_pos`  (col, row)       — divisor 1
/// - Location 2: `vec4 a_bg_color`  (r, g, b, a)     — divisor 1
unsafe fn setup_bg_inst_attribs(
    gl: &glow::Context,
    unit_quad_vbo: glow::Buffer,
    instance_vbo: glow::Buffer,
) {
    let f = gl_i32(size_of::<f32>());
    let inst_stride = gl_i32(BG_INSTANCE_FLOATS * size_of::<f32>());

    unsafe {
        // Location 0: unit-quad vertex position (per-vertex).
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(unit_quad_vbo));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 2 * f, 0);
        gl.vertex_attrib_divisor(0, 0);

        // Locations 1–2: instance data (per-instance).
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(instance_vbo));
        // Location 1: vec2 a_cell_pos (col, row).
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, inst_stride, 0);
        gl.vertex_attrib_divisor(1, 1);
        // Location 2: vec4 a_bg_color (r, g, b, a).
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, inst_stride, 2 * f);
        gl.vertex_attrib_divisor(2, 1);
    }
}

/// Configure vertex attributes for the decoration shader.
///
/// Layout: `location 0 = vec2 pos, location 1 = vec4 color`.
/// Stride = `DECO_VERTEX_FLOATS * 4` bytes.
///
/// Used for underlines, strikethrough, cursor, and selection highlight quads.
unsafe fn setup_deco_attribs(gl: &glow::Context) {
    let stride = gl_i32(DECO_VERTEX_FLOATS * size_of::<f32>());
    let offset_c2 = gl_i32(2 * size_of::<f32>());
    unsafe {
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, stride, offset_c2);
    }
}

/// Configure vertex attributes for the instanced foreground shader.
///
/// Binds the static unit-quad VBO to location 0 (per-vertex, divisor 0)
/// and the instance VBO to locations 1–5 (per-instance, divisor 1).
///
/// - Location 0: `vec2  a_pos`          (unit quad)       — divisor 0
/// - Location 1: `vec2  a_glyph_origin` (pixel position)  — divisor 1
/// - Location 2: `vec2  a_glyph_size`   (pixel size)      — divisor 1
/// - Location 3: `vec4  a_uv_rect`      (u0, v0, u1, v1)  — divisor 1
/// - Location 4: `vec4  a_fg_color`     (r, g, b, a)      — divisor 1
/// - Location 5: `float a_is_color`     (1.0 or 0.0)      — divisor 1
unsafe fn setup_fg_inst_attribs(
    gl: &glow::Context,
    unit_quad_vbo: glow::Buffer,
    instance_vbo: glow::Buffer,
) {
    let f = gl_i32(size_of::<f32>());
    let inst_stride = gl_i32(FG_INSTANCE_FLOATS * size_of::<f32>());

    unsafe {
        // Location 0: unit-quad vertex position (per-vertex).
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(unit_quad_vbo));
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 2 * f, 0);
        gl.vertex_attrib_divisor(0, 0);

        // Locations 1–5: instance data (per-instance).
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(instance_vbo));
        // Location 1: vec2 a_glyph_origin (glyph_x, glyph_y).
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, inst_stride, 0);
        gl.vertex_attrib_divisor(1, 1);
        // Location 2: vec2 a_glyph_size (glyph_w, glyph_h).
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, inst_stride, 2 * f);
        gl.vertex_attrib_divisor(2, 1);
        // Location 3: vec4 a_uv_rect (u0, v0, u1, v1).
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_f32(3, 4, glow::FLOAT, false, inst_stride, 4 * f);
        gl.vertex_attrib_divisor(3, 1);
        // Location 4: vec4 a_fg_color (r, g, b, a).
        gl.enable_vertex_attrib_array(4);
        gl.vertex_attrib_pointer_f32(4, 4, glow::FLOAT, false, inst_stride, 8 * f);
        gl.vertex_attrib_divisor(4, 1);
        // Location 5: float a_is_color.
        gl.enable_vertex_attrib_array(5);
        gl.vertex_attrib_pointer_f32(5, 1, glow::FLOAT, false, inst_stride, 12 * f);
        gl.vertex_attrib_divisor(5, 1);
    }
}

/// Configure vertex attributes for the image shader.
///
/// Layout: `location 0 = vec2 pos, location 1 = vec2 uv`.
/// Stride = `IMG_VERTEX_FLOATS * 4` bytes.
unsafe fn setup_img_attribs(gl: &glow::Context) {
    let stride = gl_i32(IMG_VERTEX_FLOATS * size_of::<f32>());
    let f = gl_i32(size_of::<f32>());
    unsafe {
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 2 * f);
    }
}

// ---------------------------------------------------------------------------
//  Shader compilation helper
// ---------------------------------------------------------------------------

/// Compile and link a GLSL program from vertex and fragment source strings.
fn compile_program(
    gl: &glow::Context,
    vert_src: &str,
    frag_src: &str,
    label: &'static str,
) -> Result<glow::Program, ShaderCompileError> {
    unsafe {
        let vert = compile_shader(gl, glow::VERTEX_SHADER, vert_src, label)?;
        let frag = compile_shader(gl, glow::FRAGMENT_SHADER, frag_src, label)?;

        let program = gl
            .create_program()
            .map_err(|e| ShaderCompileError::CreateProgram { label, message: e })?;
        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);

        if !gl.get_program_link_status(program) {
            let info = gl.get_program_info_log(program);
            gl.delete_program(program);
            gl.delete_shader(vert);
            gl.delete_shader(frag);
            return Err(ShaderCompileError::Link { label, info });
        }

        gl.delete_shader(vert);
        gl.delete_shader(frag);
        Ok(program)
    }
}

/// Compile a single GLSL shader stage.
unsafe fn compile_shader(
    gl: &glow::Context,
    shader_type: u32,
    src: &str,
    label: &'static str,
) -> Result<glow::Shader, ShaderCompileError> {
    unsafe {
        let shader = gl
            .create_shader(shader_type)
            .map_err(|e| ShaderCompileError::CreateShader { label, message: e })?;
        gl.shader_source(shader, src);
        gl.compile_shader(shader);

        if !gl.get_shader_compile_status(shader) {
            let info = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(ShaderCompileError::Compile { label, info });
        }
        Ok(shader)
    }
}

// ---------------------------------------------------------------------------
//  GPU upload helpers
// ---------------------------------------------------------------------------

/// Upload a `&[f32]` to a VBO using the orphan-then-write pattern.
fn upload_verts(gl: &glow::Context, vbo: glow::Buffer, verts: &[f32]) {
    if verts.is_empty() {
        return;
    }

    // SAFETY: we reinterpret `&[f32]` as `&[u8]` for the GL call.
    let bytes = unsafe {
        std::slice::from_raw_parts(verts.as_ptr().cast::<u8>(), std::mem::size_of_val(verts))
    };

    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        // Orphan the buffer first to avoid sync stalls.
        gl.buffer_data_size(glow::ARRAY_BUFFER, gl_i32(bytes.len()), glow::STREAM_DRAW);
        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
    }
}

/// Upload a sub-range of a VBO **without orphaning** the whole buffer.
///
/// Used for cursor-only partial updates: the caller has already orphaned the
/// buffer (or it is large enough) and just wants to patch a specific byte
/// range.
///
/// `byte_offset` is the byte offset into the existing VBO data.
fn upload_verts_sub(gl: &glow::Context, vbo: glow::Buffer, byte_offset: usize, verts: &[f32]) {
    if verts.is_empty() {
        return;
    }

    // SAFETY: we reinterpret `&[f32]` as `&[u8]` for the GL call.
    let bytes = unsafe {
        std::slice::from_raw_parts(verts.as_ptr().cast::<u8>(), std::mem::size_of_val(verts))
    };

    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, gl_i32(byte_offset), bytes);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
    }
}

// ---------------------------------------------------------------------------
//  Background image UV computation
// ---------------------------------------------------------------------------

/// Geometry for the background-image fullscreen quad.
struct BgQuadGeometry {
    /// Pixel-space position: (x0, y0, x1, y1).
    pos: (f32, f32, f32, f32),
    /// Texture coordinates: (u0, v0, u1, v1).
    uv: (f32, f32, f32, f32),
}

/// Compute UV rectangle `(u0, v0, u1, v1)` for the background image quad.
///
/// The UV range describes what portion of the image is displayed and how it
/// maps to the viewport. The renderer uses a top-left image-space convention
/// for this path: the origin is the top-left corner and V increases
/// downward, matching images uploaded top-row-first via
/// `image::DynamicImage::into_rgba8`.
///
/// # Return value
///
/// Returns a [`BgQuadGeometry`] containing the pixel-space quad rectangle
/// and the UV rectangle.  For most modes the quad covers the full viewport;
/// for `Fit` mode the quad is shrunk to maintain the image's aspect ratio
/// (letterbox / pillarbox).
///
/// # Modes
///
/// | Mode   | Description                                                   |
/// |--------|---------------------------------------------------------------|
/// | Fill   | Stretch to fill the viewport; may distort the image.         |
/// | Fit    | Letterbox / pillarbox: show the full image without distortion.|
/// | Cover  | Crop to fill the viewport; center-crop; no distortion.       |
/// | Tile   | Repeat at native image resolution.                           |
fn compute_bg_uvs(
    vp_w: f32,
    vp_h: f32,
    img_w: u32,
    img_h: u32,
    mode: freminal_common::config::BackgroundImageMode,
) -> BgQuadGeometry {
    use freminal_common::config::BackgroundImageMode;

    match mode {
        BackgroundImageMode::Fill => {
            // UV covers the entire image regardless of aspect ratio.
            BgQuadGeometry {
                pos: (0.0, 0.0, vp_w, vp_h),
                uv: (0.0, 0.0, 1.0, 1.0),
            }
        }
        BackgroundImageMode::Fit => {
            // Scale uniformly to fit the smaller dimension.
            // The quad is shrunk to maintain the image aspect ratio,
            // producing letterbox (top/bottom bars) or pillarbox
            // (left/right bars).  UVs always span the full image (0..1).
            let img_aspect = img_w.approx_as::<f32>().unwrap_or(0.0)
                / img_h.max(1).approx_as::<f32>().unwrap_or(1.0);
            let vp_aspect = vp_w / vp_h.max(1.0);
            if img_aspect > vp_aspect {
                // Image is wider than viewport: letterbox (top/bottom bars).
                // Width fills viewport; height is scaled down.
                let drawn_h = vp_w / img_aspect;
                let margin = (vp_h - drawn_h) / 2.0;
                BgQuadGeometry {
                    pos: (0.0, margin, vp_w, vp_h - margin),
                    uv: (0.0, 0.0, 1.0, 1.0),
                }
            } else {
                // Image is taller than viewport: pillarbox (left/right bars).
                // Height fills viewport; width is scaled down.
                let drawn_w = vp_h * img_aspect;
                let margin = (vp_w - drawn_w) / 2.0;
                BgQuadGeometry {
                    pos: (margin, 0.0, vp_w - margin, vp_h),
                    uv: (0.0, 0.0, 1.0, 1.0),
                }
            }
        }
        BackgroundImageMode::Cover => {
            // Scale uniformly to fill the larger dimension; center-crop.
            let img_aspect = img_w.approx_as::<f32>().unwrap_or(0.0)
                / img_h.max(1).approx_as::<f32>().unwrap_or(1.0);
            let vp_aspect = vp_w / vp_h.max(1.0);
            if img_aspect > vp_aspect {
                // Image is wider: crop left/right.
                let scale = vp_aspect / img_aspect;
                let margin = (1.0 - scale) / 2.0;
                BgQuadGeometry {
                    pos: (0.0, 0.0, vp_w, vp_h),
                    uv: (margin, 0.0, 1.0 - margin, 1.0),
                }
            } else {
                // Image is taller: crop top/bottom.
                let scale = img_aspect / vp_aspect;
                let margin = (1.0 - scale) / 2.0;
                BgQuadGeometry {
                    pos: (0.0, 0.0, vp_w, vp_h),
                    uv: (0.0, margin, 1.0, 1.0 - margin),
                }
            }
        }
        BackgroundImageMode::Tile => {
            // Repeat at native pixel density.
            let u_repeat = vp_w / img_w.max(1).approx_as::<f32>().unwrap_or(1.0);
            let v_repeat = vp_h / img_h.max(1).approx_as::<f32>().unwrap_or(1.0);
            BgQuadGeometry {
                pos: (0.0, 0.0, vp_w, vp_h),
                uv: (0.0, 0.0, u_repeat, v_repeat),
            }
        }
    }
}

// ---------------------------------------------------------------------------
//  WindowPostRenderer — window-level post-processing pass
// ---------------------------------------------------------------------------

/// Manages a single window-level post-processing FBO and shader program.
///
/// All panes share one `WindowPostRenderer` (via `Arc<Mutex<…>>`).  When a
/// user GLSL shader is active, each pane's `PaintCallback` renders terminal
/// content into the shared window FBO.  A single window-level `PaintCallback`
/// (registered after the pane loop) then draws the FBO texture through the
/// user shader to egui's framebuffer.
///
/// When no shader is configured the window FBO is `None` and panes render
/// directly to egui's framebuffer (unchanged behaviour).
pub struct WindowPostRenderer {
    /// Whether GPU resources (program, VAO, VBO) have been created.
    initialized: bool,

    // ---- post-processing shader + fullscreen quad ----
    /// Passthrough-or-user post-processing program.
    program: Option<glow::Program>,
    /// VAO for the fullscreen NDC quad.
    vao: Option<glow::VertexArray>,
    /// VBO for the fullscreen NDC quad.
    vbo: Option<glow::Buffer>,
    // Uniform locations.
    u_terminal: Option<glow::UniformLocation>,
    u_resolution: Option<glow::UniformLocation>,
    u_time: Option<glow::UniformLocation>,

    // ---- window FBO (created on demand; None = no active shader) ----
    /// Offscreen framebuffer for the full window area; `None` when inactive.
    fbo: Option<glow::Framebuffer>,
    /// Color attachment texture for the FBO.
    fbo_texture: Option<glow::Texture>,
    /// Cached FBO dimensions to detect resizes.
    fbo_size: Option<(i32, i32)>,

    /// Elapsed time accumulator for the `u_time` uniform (seconds).
    time: f32,

    /// Pending shader source to compile on the next `PaintCallback`.
    ///
    /// `Some(Some(src))` → compile `src` as the user fragment shader.
    /// `Some(None)` → revert to passthrough and destroy the FBO.
    /// `None` → no change pending.
    pub pending_shader: Option<Option<String>>,
}

impl Default for WindowPostRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowPostRenderer {
    /// Create a new, uninitialized `WindowPostRenderer`.
    ///
    /// GPU resources are created lazily on the first call to [`Self::init`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            initialized: false,
            program: None,
            vao: None,
            vbo: None,
            u_terminal: None,
            u_resolution: None,
            u_time: None,
            fbo: None,
            fbo_texture: None,
            fbo_size: None,
            time: 0.0,
            pending_shader: None,
        }
    }

    /// Return `true` if GPU resources have been created.
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Return `true` if a user shader is active (the window FBO is in use).
    ///
    /// When this returns `true`, each pane's `PaintCallback` should render
    /// into the window FBO rather than egui's framebuffer.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.fbo.is_some()
    }

    /// Return the window FBO handle, if active.
    #[must_use]
    pub const fn fbo(&self) -> Option<glow::Framebuffer> {
        self.fbo
    }

    /// Initialise the GPU resources (passthrough shader program + fullscreen quad).
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if shader compilation or any GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(
            gl,
            POST_VERT_SRC,
            POST_PASSTHROUGH_FRAG_SRC,
            "wpr_passthrough",
        )?;

        self.u_terminal = unsafe { gl.get_uniform_location(program, "u_terminal") };
        self.u_resolution = unsafe { gl.get_uniform_location(program, "u_resolution") };
        self.u_time = unsafe { gl.get_uniform_location(program, "u_time") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("wpr VAO", e))?
        };
        let vbo = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("wpr VBO", e))?
        };

        // Fullscreen NDC quad: two triangles covering [-1,1]².
        // Vertex layout: vec2 pos (NDC), vec2 uv.
        #[rustfmt::skip]
        let quad: [f32; 24] = [
            // pos (NDC)    uv
            -1.0, -1.0,    0.0, 0.0,
             1.0, -1.0,    1.0, 0.0,
            -1.0,  1.0,    0.0, 1.0,
             1.0, -1.0,    1.0, 0.0,
             1.0,  1.0,    1.0, 1.0,
            -1.0,  1.0,    0.0, 1.0,
        ];
        let bytes = unsafe {
            std::slice::from_raw_parts(quad.as_ptr().cast::<u8>(), std::mem::size_of_val(&quad))
        };
        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            setup_img_attribs(gl); // same layout: vec2 pos, vec2 uv
            gl.bind_vertex_array(None);
        }

        self.program = Some(program);
        self.vao = Some(vao);
        self.vbo = Some(vbo);
        self.initialized = true;
        Ok(())
    }

    /// Ensure the window FBO exists and matches `(w, h)` in physical pixels.
    ///
    /// Creates or recreates the FBO when missing or resized.  No-op when the
    /// size has not changed.
    pub fn ensure_fbo(&mut self, gl: &glow::Context, w: i32, h: i32) {
        if self.fbo_size == Some((w, h)) {
            return;
        }

        // Delete stale FBO + texture.
        unsafe {
            if let Some(fbo) = self.fbo.take() {
                gl.delete_framebuffer(fbo);
            }
            if let Some(tex) = self.fbo_texture.take() {
                gl.delete_texture(tex);
            }
        }

        // Create the color texture.
        let tex = unsafe {
            match gl.create_texture() {
                Ok(t) => t,
                Err(e) => {
                    error!("WindowPostRenderer::ensure_fbo: create texture: {e}");
                    return;
                }
            }
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
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
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA.cast_signed(),
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        // Create the FBO and attach the texture.
        let fbo = unsafe {
            match gl.create_framebuffer() {
                Ok(f) => f,
                Err(e) => {
                    error!("WindowPostRenderer::ensure_fbo: create framebuffer: {e}");
                    gl.delete_texture(tex);
                    return;
                }
            }
        };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(tex),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                error!("WindowPostRenderer::ensure_fbo: FBO incomplete (status {status:#x})");
                gl.delete_framebuffer(fbo);
                gl.delete_texture(tex);
                return;
            }
        }

        self.fbo = Some(fbo);
        self.fbo_texture = Some(tex);
        self.fbo_size = Some((w, h));
    }

    /// Compile and install a user fragment shader.
    ///
    /// On success, replaces the current program and ensures the FBO is created
    /// for the given viewport dimensions.  On failure, keeps the existing
    /// program and returns an error.
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if shader compilation or linking fails.
    pub fn update_shader(
        &mut self,
        gl: &glow::Context,
        frag_src: &str,
        vp_w: i32,
        vp_h: i32,
    ) -> Result<(), GpuInitError> {
        let new_prog = compile_program(gl, POST_VERT_SRC, frag_src, "wpr_user")?;

        let u_terminal = unsafe { gl.get_uniform_location(new_prog, "u_terminal") };
        let u_resolution = unsafe { gl.get_uniform_location(new_prog, "u_resolution") };
        let u_time = unsafe { gl.get_uniform_location(new_prog, "u_time") };

        if let Some(old) = self.program.take() {
            unsafe { gl.delete_program(old) };
        }
        self.program = Some(new_prog);
        self.u_terminal = u_terminal;
        self.u_resolution = u_resolution;
        self.u_time = u_time;

        self.ensure_fbo(gl, vp_w, vp_h);
        Ok(())
    }

    /// Revert to passthrough shader and destroy the window FBO.
    ///
    /// After this call [`is_active`](Self::is_active) returns `false` and panes
    /// render directly to egui's framebuffer.
    pub fn clear_shader(&mut self, gl: &glow::Context) {
        // Destroy FBO + texture.
        if let Some(fbo) = self.fbo.take() {
            unsafe { gl.delete_framebuffer(fbo) };
        }
        if let Some(tex) = self.fbo_texture.take() {
            unsafe { gl.delete_texture(tex) };
        }
        self.fbo_size = None;
        self.time = 0.0;

        // Recompile the passthrough shader.
        match compile_program(
            gl,
            POST_VERT_SRC,
            POST_PASSTHROUGH_FRAG_SRC,
            "wpr_passthrough",
        ) {
            Ok(new_prog) => {
                let u_terminal = unsafe { gl.get_uniform_location(new_prog, "u_terminal") };
                let u_resolution = unsafe { gl.get_uniform_location(new_prog, "u_resolution") };
                let u_time = unsafe { gl.get_uniform_location(new_prog, "u_time") };
                if let Some(old) = self.program.take() {
                    unsafe { gl.delete_program(old) };
                }
                self.program = Some(new_prog);
                self.u_terminal = u_terminal;
                self.u_resolution = u_resolution;
                self.u_time = u_time;
            }
            Err(e) => {
                error!("WindowPostRenderer::clear_shader: recompile passthrough failed: {e}");
            }
        }
    }

    /// Apply the post-processing pass to the currently bound framebuffer.
    ///
    /// Samples the window FBO texture (`self.fbo_texture`) through the user
    /// shader.  The caller is responsible for binding egui's framebuffer
    /// before this call and restoring state afterwards.
    ///
    /// Advances `self.time` by `delta_seconds`.
    pub fn draw_post_pass(&mut self, gl: &glow::Context, vp_w: f32, vp_h: f32, delta_seconds: f32) {
        let (Some(prog), Some(vao), Some(tex)) = (self.program, self.vao, self.fbo_texture) else {
            return;
        };

        self.time += delta_seconds;

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.u_terminal {
                gl.uniform_1_i32(Some(loc), 3); // TEXTURE3
            }
            if let Some(loc) = &self.u_resolution {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            if let Some(loc) = &self.u_time {
                gl.uniform_1_f32(Some(loc), self.time);
            }

            gl.active_texture(glow::TEXTURE3);
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.bind_vertex_array(Some(vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.active_texture(glow::TEXTURE0);
            gl.use_program(None);
        }
    }

    /// Free all GPU resources.
    ///
    /// Should be called when the application exits or when the GL context is
    /// destroyed.
    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(p) = self.program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.vao.take() {
                gl.delete_vertex_array(v);
            }
            if let Some(b) = self.vbo.take() {
                gl.delete_buffer(b);
            }
            if let Some(fbo) = self.fbo.take() {
                gl.delete_framebuffer(fbo);
            }
            if let Some(t) = self.fbo_texture.take() {
                gl.delete_texture(t);
            }
        }
        self.initialized = false;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::compute_bg_uvs;
    use freminal_common::config::BackgroundImageMode;

    // Helper: assert two f32 values are approximately equal.
    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    // ── BackgroundImageMode::Fill ─────────────────────────────────────

    #[test]
    fn compute_bg_uvs_fill_returns_full_0_to_1() {
        // Fill always maps UV (0,0)–(1,1) and the quad covers the full viewport.
        let geom = compute_bg_uvs(1920.0, 1080.0, 800, 600, BackgroundImageMode::Fill);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, 1920.0) && approx_eq(py1, 1080.0));
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 1.0) && approx_eq(v1, 1.0));
    }

    // ── BackgroundImageMode::Tile ─────────────────────────────────────

    #[test]
    fn compute_bg_uvs_tile_exact_repeat_count() {
        // Viewport exactly 3× the image in both dimensions → repeats = 3.
        let img_w = 100_u32;
        let img_h = 50_u32;
        let vp_w = 300.0_f32;
        let vp_h = 150.0_f32;
        let geom = compute_bg_uvs(vp_w, vp_h, img_w, img_h, BackgroundImageMode::Tile);
        let (u0, v0, u1, v1) = geom.uv;
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 3.0), "u repeat should be 3.0, got {u1}");
        assert!(approx_eq(v1, 3.0), "v repeat should be 3.0, got {v1}");
        // Quad covers the full viewport.
        let (px0, py0, px1, py1) = geom.pos;
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, vp_w) && approx_eq(py1, vp_h));
    }

    #[test]
    fn compute_bg_uvs_tile_fractional_repeat() {
        // Viewport is 1.5× the image.
        let img_w = 200_u32;
        let img_h = 200_u32;
        let vp_w = 300.0_f32;
        let vp_h = 300.0_f32;
        let geom = compute_bg_uvs(vp_w, vp_h, img_w, img_h, BackgroundImageMode::Tile);
        let (_, _, u1, v1) = geom.uv;
        assert!(approx_eq(u1, 1.5), "u repeat should be 1.5, got {u1}");
        assert!(approx_eq(v1, 1.5), "v repeat should be 1.5, got {v1}");
    }

    // ── BackgroundImageMode::Fit ──────────────────────────────────────

    #[test]
    fn compute_bg_uvs_fit_square_image_square_viewport() {
        // Same aspect ratio → no letterbox/pillarbox; quad fills viewport.
        let geom = compute_bg_uvs(400.0, 400.0, 400, 400, BackgroundImageMode::Fit);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, 400.0) && approx_eq(py1, 400.0));
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 1.0) && approx_eq(v1, 1.0));
    }

    #[test]
    fn compute_bg_uvs_fit_wide_image_narrow_viewport_letterboxes() {
        // Image is 2:1; viewport is 1:1.  Image is wider than viewport →
        // letterbox: quad is inset vertically (top/bottom pixel-space margins).
        // UVs span the full image.
        let geom = compute_bg_uvs(400.0, 400.0, 800, 400, BackgroundImageMode::Fit);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        // img_aspect = 2.0; vp_aspect = 1.0 → img_aspect > vp_aspect branch.
        // drawn_h = 400 / 2.0 = 200; margin = (400 - 200) / 2 = 100.
        assert!(approx_eq(px0, 0.0), "x0 should be 0, got {px0}");
        assert!(approx_eq(px1, 400.0), "x1 should be 400, got {px1}");
        assert!(
            approx_eq(py0, 100.0),
            "top margin should be 100px, got {py0}"
        );
        assert!(
            approx_eq(py1, 300.0),
            "bottom edge should be 300px, got {py1}"
        );
        // UVs always span full image for Fit mode.
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 1.0) && approx_eq(v1, 1.0));
    }

    #[test]
    fn compute_bg_uvs_fit_tall_image_wide_viewport_pillarboxes() {
        // Image is 1:2 (tall); viewport is 2:1 (wide) → pillarbox:
        // quad is inset horizontally (left/right pixel-space margins).
        let geom = compute_bg_uvs(800.0, 400.0, 400, 800, BackgroundImageMode::Fit);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        // img_aspect = 0.5; vp_aspect = 2.0 → else branch (pillarbox).
        // drawn_w = 400 * 0.5 = 200; margin = (800 - 200) / 2 = 300.
        assert!(approx_eq(py0, 0.0), "y0 should be 0, got {py0}");
        assert!(approx_eq(py1, 400.0), "y1 should be 400, got {py1}");
        assert!(
            approx_eq(px0, 300.0),
            "left margin should be 300px, got {px0}"
        );
        assert!(
            approx_eq(px1, 500.0),
            "right edge should be 500px, got {px1}"
        );
        // UVs always span full image for Fit mode.
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 1.0) && approx_eq(v1, 1.0));
    }

    // ── BackgroundImageMode::Cover ────────────────────────────────────

    #[test]
    fn compute_bg_uvs_cover_square_image_square_viewport() {
        // Same aspect → no crop; quad fills viewport, UVs span full image.
        let geom = compute_bg_uvs(400.0, 400.0, 400, 400, BackgroundImageMode::Cover);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, 400.0) && approx_eq(py1, 400.0));
        assert!(approx_eq(u0, 0.0) && approx_eq(v0, 0.0));
        assert!(approx_eq(u1, 1.0) && approx_eq(v1, 1.0));
    }

    #[test]
    fn compute_bg_uvs_cover_wide_image_narrow_viewport_crops_sides() {
        // Image is 2:1; viewport is 1:1.  Cover fills height → crop left/right.
        // Quad covers full viewport; UVs are inset horizontally.
        let geom = compute_bg_uvs(400.0, 400.0, 800, 400, BackgroundImageMode::Cover);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        // img_aspect=2.0, vp_aspect=1.0 → img wider → crop u (left/right).
        // scale = 1.0/2.0 = 0.5; margin = 0.25.
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, 400.0) && approx_eq(py1, 400.0));
        assert!(
            approx_eq(v0, 0.0) && approx_eq(v1, 1.0),
            "v should span 0–1"
        );
        assert!(approx_eq(u0, 0.25), "left crop should be 0.25, got {u0}");
        assert!(approx_eq(u1, 0.75), "right crop should be 0.75, got {u1}");
    }

    #[test]
    fn compute_bg_uvs_cover_tall_image_wide_viewport_crops_top_bottom() {
        // Image is 1:2 (tall); viewport is 2:1 (wide) → crop top/bottom.
        // Quad covers full viewport; UVs are inset vertically.
        let geom = compute_bg_uvs(800.0, 400.0, 400, 800, BackgroundImageMode::Cover);
        let (px0, py0, px1, py1) = geom.pos;
        let (u0, v0, u1, v1) = geom.uv;
        // img_aspect=0.5; vp_aspect=2.0 → tall → crop v.
        // scale = 0.5/2.0 = 0.25; margin = 0.375.
        assert!(approx_eq(px0, 0.0) && approx_eq(py0, 0.0));
        assert!(approx_eq(px1, 800.0) && approx_eq(py1, 400.0));
        assert!(
            approx_eq(u0, 0.0) && approx_eq(u1, 1.0),
            "u should span 0–1"
        );
        assert!(approx_eq(v0, 0.375), "top crop should be 0.375, got {v0}");
        assert!(
            approx_eq(v1, 0.625),
            "bottom crop should be 0.625, got {v1}"
        );
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn compute_bg_uvs_fill_zero_image_does_not_panic() {
        // img_w = img_h = 0 → max(1) guard prevents divide-by-zero.
        let result = std::panic::catch_unwind(|| {
            compute_bg_uvs(800.0, 600.0, 0, 0, BackgroundImageMode::Fill);
        });
        assert!(result.is_ok(), "should not panic with zero-size image");
    }

    #[test]
    fn compute_bg_uvs_tile_zero_image_does_not_panic() {
        let result = std::panic::catch_unwind(|| {
            compute_bg_uvs(800.0, 600.0, 0, 0, BackgroundImageMode::Tile);
        });
        assert!(
            result.is_ok(),
            "should not panic with zero-size image in tile mode"
        );
    }
}
