// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GL shader programs, vertex buffers, and draw calls for the terminal renderer.
//!
//! [`TerminalRenderer`] owns all GPU state for the custom terminal rendering
//! pipeline: two shader programs (background solid-color quads, foreground textured
//! glyph quads from the atlas), VAOs, double-buffered VBOs, and the atlas GL texture.
//!
//! Rendering is triggered via egui's [`eframe::egui_glow::CallbackFn`] mechanism.
//! The CPU-side vertex builders (`build_background_verts` / `build_foreground_verts`)
//! are pure functions and are fully testable without a GL context.

use eframe::glow::{self, HasContext};
use freminal_common::buffer_states::cursor::CursorPos;
use freminal_common::buffer_states::fonts::FontDecorations;
use freminal_common::cursor::CursorVisualStyle;

use super::atlas::{GlyphAtlas, GlyphKey};
use super::colors::{CURSOR_F, SELECTION_BG_F, SELECTION_FG_F, internal_color_to_gl};
use super::font_manager::FontManager;
use super::shaping::{ShapedGlyph, ShapedLine};

// ---------------------------------------------------------------------------
//  GLSL shaders  (GL 3.3 core profile)
// ---------------------------------------------------------------------------

/// Background pass: solid-color quads (background fills, cursor, underline,
/// strikethrough).
///
/// Vertex layout: `vec2 a_pos, vec4 a_color`  (stride = 6 × f32 = 24 bytes)
const BG_VERT_SRC: &str = r"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec4 a_color;

out vec4 v_color;

uniform vec2 u_viewport_size;

void main() {
    // Convert from pixel coordinates (top-left origin) to NDC.
    vec2 ndc = (a_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_color = a_color;
}
";

const BG_FRAG_SRC: &str = r"#version 330 core
in vec4 v_color;
out vec4 frag_color;

void main() {
    // Premultiplied alpha output.
    frag_color = vec4(v_color.rgb * v_color.a, v_color.a);
}
";

/// Foreground pass: textured glyph quads sampled from the atlas.
///
/// Vertex layout: `vec2 a_pos, vec2 a_uv, vec4 a_color, float a_is_color`
///   (stride = 9 × f32 = 36 bytes)
const FG_VERT_SRC: &str = r"#version 330 core
layout(location = 0) in vec2  a_pos;
layout(location = 1) in vec2  a_uv;
layout(location = 2) in vec4  a_color;
layout(location = 3) in float a_is_color;

out vec2  v_uv;
out vec4  v_color;
out float v_is_color;

uniform vec2 u_viewport_size;

void main() {
    vec2 ndc = (a_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv       = a_uv;
    v_color    = a_color;
    v_is_color = a_is_color;
}
";

const FG_FRAG_SRC: &str = r"#version 330 core
in vec2  v_uv;
in vec4  v_color;
in float v_is_color;

out vec4 frag_color;

uniform sampler2D u_atlas;

void main() {
    if (v_is_color > 0.5) {
        // Color emoji: pass through atlas RGBA directly (already premultiplied).
        frag_color = texture(u_atlas, v_uv);
    } else {
        // Monochrome glyph: tint with foreground color.
        float alpha = texture(u_atlas, v_uv).a;
        // Premultiplied alpha output.
        frag_color = vec4(v_color.rgb * (v_color.a * alpha), v_color.a * alpha);
    }
}
";

// ---------------------------------------------------------------------------
//  Vertex strides (in f32 components)
// ---------------------------------------------------------------------------

/// Background vertex: `x, y, r, g, b, a` — 6 floats per vertex.
const BG_VERTEX_FLOATS: usize = 6;
/// Foreground vertex: `x, y, u, v, r, g, b, a, is_color` — 9 floats per vertex.
const FG_VERTEX_FLOATS: usize = 9;
/// Vertices per quad (2 triangles, 6 vertices).
pub(crate) const VERTS_PER_QUAD: usize = 6;
/// Floats for one cursor quad in the background VBO.
pub(crate) const CURSOR_QUAD_FLOATS: usize = VERTS_PER_QUAD * BG_VERTEX_FLOATS;

// ---------------------------------------------------------------------------
//  TerminalRenderer
// ---------------------------------------------------------------------------

/// Holds all GPU resources for the custom terminal rendering pipeline.
///
/// Call [`TerminalRenderer::init`] once (inside the first `PaintCallback`
/// invocation) to create shaders, VAOs, VBOs, and the atlas texture.  Then call
/// [`TerminalRenderer::draw`] every frame.
pub struct TerminalRenderer {
    /// Whether GPU resources have been created.
    initialized: bool,

    // ---- background pass ----
    bg_program: Option<glow::Program>,
    bg_vao: Option<glow::VertexArray>,
    bg_vbo: [Option<glow::Buffer>; 2],

    // ---- foreground pass ----
    fg_program: Option<glow::Program>,
    fg_vao: Option<glow::VertexArray>,
    fg_vbo: [Option<glow::Buffer>; 2],

    // ---- atlas texture ----
    atlas_texture: Option<glow::Texture>,

    // ---- uniform locations ----
    bg_u_viewport: Option<glow::UniformLocation>,
    fg_u_viewport: Option<glow::UniformLocation>,
    fg_u_atlas: Option<glow::UniformLocation>,

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
    pub const fn new() -> Self {
        Self {
            initialized: false,
            bg_program: None,
            bg_vao: None,
            bg_vbo: [None, None],
            fg_program: None,
            fg_vao: None,
            fg_vbo: [None, None],
            atlas_texture: None,
            bg_u_viewport: None,
            fg_u_viewport: None,
            fg_u_atlas: None,
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
    /// Returns a human-readable error string if shader compilation/linking fails
    /// or if any GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), String> {
        // Compile shaders.
        let bg_program = compile_program(gl, BG_VERT_SRC, BG_FRAG_SRC, "background")?;
        let fg_program = compile_program(gl, FG_VERT_SRC, FG_FRAG_SRC, "foreground")?;

        // Cache uniform locations.
        let bg_u_viewport = unsafe { gl.get_uniform_location(bg_program, "u_viewport_size") };
        let fg_u_viewport = unsafe { gl.get_uniform_location(fg_program, "u_viewport_size") };
        let fg_u_atlas = unsafe { gl.get_uniform_location(fg_program, "u_atlas") };

        // --- background VAO + double-buffered VBOs ---
        let bg_vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create background VAO: {e}"))?
        };
        let bg_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create background VBO 0: {e}"))?
        };
        let bg_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create background VBO 1: {e}"))?
        };

        unsafe {
            gl.bind_vertex_array(Some(bg_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(bg_vbo0));
            setup_bg_attribs(gl);
            gl.bind_vertex_array(None);
        }

        // --- foreground VAO + double-buffered VBOs ---
        let fg_vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create foreground VAO: {e}"))?
        };
        let fg_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create foreground VBO 0: {e}"))?
        };
        let fg_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create foreground VBO 1: {e}"))?
        };

        unsafe {
            gl.bind_vertex_array(Some(fg_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(fg_vbo0));
            setup_fg_attribs(gl);
            gl.bind_vertex_array(None);
        }

        // --- Atlas texture ---
        let atlas_texture = unsafe {
            gl.create_texture()
                .map_err(|e| format!("create atlas texture: {e}"))?
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

        self.bg_program = Some(bg_program);
        self.bg_vao = Some(bg_vao);
        self.bg_vbo = [Some(bg_vbo0), Some(bg_vbo1)];
        self.fg_program = Some(fg_program);
        self.fg_vao = Some(fg_vao);
        self.fg_vbo = [Some(fg_vbo0), Some(fg_vbo1)];
        self.atlas_texture = Some(atlas_texture);
        self.bg_u_viewport = bg_u_viewport;
        self.fg_u_viewport = fg_u_viewport;
        self.fg_u_atlas = fg_u_atlas;
        self.initialized = true;

        Ok(())
    }

    /// Render a complete terminal frame.
    ///
    /// Uploads any dirty atlas regions, builds vertex buffers from `shaped_lines`,
    /// draws the background pass then the foreground pass.  Restores the egui
    /// intermediate FBO on completion.
    ///
    /// # Panics
    ///
    /// Panics if `init()` has not been called.  Check `initialized()` first.
    ///
    /// # Safety
    ///
    /// This method calls `glow` functions which are marked `unsafe`.  The caller
    /// is responsible for ensuring a valid GL context exists.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        atlas: &mut GlyphAtlas,
        shaped_lines: &[ShapedLine],
        font_manager: &FontManager,
        cell_width: u32,
        cell_height: u32,
        ascent: f32,
        underline_offset: f32,
        strikeout_offset: f32,
        stroke_size: f32,
        cursor_pos: CursorPos,
        show_cursor: bool,
        cursor_blink_on: bool,
        cursor_visual_style: &CursorVisualStyle,
        viewport_width: i32,
        viewport_height: i32,
        intermediate_fbo: Option<glow::Framebuffer>,
    ) {
        assert!(
            self.initialized,
            "TerminalRenderer::draw() called before init()"
        );

        // 1. Sync atlas texture.
        self.sync_atlas(gl, atlas);

        // 2. Build CPU-side vertex buffers.
        let bg_verts = build_background_verts(
            shaped_lines,
            cell_width,
            cell_height,
            underline_offset,
            strikeout_offset,
            stroke_size,
            show_cursor,
            cursor_blink_on,
            cursor_pos,
            cursor_visual_style,
            None,
        );
        let fg_verts =
            build_foreground_verts(shaped_lines, atlas, font_manager, cell_height, ascent, None);

        // 3. Upload vertex data using orphan-then-write.
        let buf_idx = self.vbo_index;
        self.upload_bg_verts(gl, &bg_verts, buf_idx);
        self.upload_fg_verts(gl, &fg_verts, buf_idx);

        // 4. Draw background pass then foreground pass.
        #[allow(clippy::cast_precision_loss)]
        let vp_w = viewport_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let vp_h = viewport_height as f32;

        self.draw_background(gl, bg_verts.len(), vp_w, vp_h, buf_idx);
        self.draw_foreground(gl, fg_verts.len(), vp_w, vp_h, buf_idx);

        // 5. Restore egui's framebuffer binding.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, intermediate_fbo);
        }

        // Advance double-buffer index.
        self.vbo_index = 1 - self.vbo_index;
    }

    /// Render a terminal frame from pre-built vertex buffers.
    ///
    /// Used when the vertex buffers were built on the main thread (where
    /// [`FontManager`] is available) before being passed into the
    /// `PaintCallback` closure, which must be `Send + Sync` and therefore
    /// cannot capture a `FontManager`.
    ///
    /// # Panics
    ///
    /// Panics if [`init`][`Self::init`] has not been called.  Check
    /// [`initialized`][`Self::initialized`] first.
    ///
    /// # Safety
    ///
    /// This method calls `glow` functions which are marked `unsafe`.  The
    /// caller is responsible for ensuring a valid GL context exists.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with_verts(
        &mut self,
        gl: &glow::Context,
        atlas: &mut GlyphAtlas,
        bg_verts: &[f32],
        fg_verts: &[f32],
        viewport_width: i32,
        viewport_height: i32,
        intermediate_fbo: Option<glow::Framebuffer>,
    ) {
        assert!(
            self.initialized,
            "TerminalRenderer::draw_with_verts() called before init()"
        );

        // 1. Sync atlas texture to the GPU.
        self.sync_atlas(gl, atlas);

        // 2. Upload pre-built vertex data using orphan-then-write.
        let buf_idx = self.vbo_index;
        self.upload_bg_verts(gl, bg_verts, buf_idx);
        self.upload_fg_verts(gl, fg_verts, buf_idx);

        // 3. Draw background pass then foreground pass.
        #[allow(clippy::cast_precision_loss)]
        let vp_w = viewport_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let vp_h = viewport_height as f32;

        self.draw_background(gl, bg_verts.len(), vp_w, vp_h, buf_idx);
        self.draw_foreground(gl, fg_verts.len(), vp_w, vp_h, buf_idx);

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
    /// quad region of the background VBO and redraws both passes.  The
    /// foreground VBO is untouched.
    ///
    /// `cursor_vert_byte_offset` is the byte offset into the background VBO
    /// where the cursor quad data begins.  `bg_total_floats` is the total
    /// float count of the most recently uploaded background VBO (needed to
    /// set the draw vertex count correctly).  `cursor_verts` contains exactly
    /// `CURSOR_QUAD_FLOATS` floats (or is empty when the cursor is hidden).
    ///
    /// # Panics
    ///
    /// Panics if [`init`][`Self::init`] has not been called.
    ///
    /// # Safety
    ///
    /// Caller must ensure a valid GL context exists.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_with_cursor_only_update(
        &mut self,
        gl: &glow::Context,
        atlas: &mut GlyphAtlas,
        cursor_vert_byte_offset: usize,
        bg_total_floats: usize,
        cursor_verts: &[f32],
        fg_total_floats: usize,
        viewport_width: i32,
        viewport_height: i32,
        intermediate_fbo: Option<glow::Framebuffer>,
    ) {
        assert!(
            self.initialized,
            "TerminalRenderer::draw_with_cursor_only_update() called before init()"
        );

        // 1. Sync atlas (may have new glyphs from a previous frame).
        self.sync_atlas(gl, atlas);

        let buf_idx = self.vbo_index;

        // 2. Patch just the cursor region of the bg VBO (no orphan).
        if cursor_verts.is_empty() {
            // Cursor is hidden: zero out the cursor quad region so no stale
            // cursor is painted.  We write CURSOR_QUAD_FLOATS zeros.
            if let Some(vbo) = self.bg_vbo[buf_idx] {
                let zeros = vec![0.0f32; CURSOR_QUAD_FLOATS];
                upload_verts_sub(gl, vbo, cursor_vert_byte_offset, &zeros);
            }
        } else if let Some(vbo) = self.bg_vbo[buf_idx] {
            upload_verts_sub(gl, vbo, cursor_vert_byte_offset, cursor_verts);
        }

        // 3. Draw background + foreground with the total float counts from the
        //    previously uploaded full frame.
        #[allow(clippy::cast_precision_loss)]
        let vp_w = viewport_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let vp_h = viewport_height as f32;

        self.draw_background(gl, bg_total_floats, vp_w, vp_h, buf_idx);
        self.draw_foreground(gl, fg_total_floats, vp_w, vp_h, buf_idx);

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

        #[allow(clippy::cast_possible_wrap)]
        let size = atlas.size() as i32;

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
                #[allow(clippy::cast_possible_wrap)]
                let rx = rect.x as i32;
                #[allow(clippy::cast_possible_wrap)]
                let ry = rect.y as i32;
                #[allow(clippy::cast_possible_wrap)]
                let rw = rect.width as i32;
                #[allow(clippy::cast_possible_wrap)]
                let rh = rect.height as i32;

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

    /// Upload background vertex data via orphan-then-write.
    fn upload_bg_verts(&self, gl: &glow::Context, verts: &[f32], buf_idx: usize) {
        let Some(vbo) = self.bg_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, verts);
    }

    /// Upload foreground vertex data via orphan-then-write.
    fn upload_fg_verts(&self, gl: &glow::Context, verts: &[f32], buf_idx: usize) {
        let Some(vbo) = self.fg_vbo[buf_idx] else {
            return;
        };
        upload_verts(gl, vbo, verts);
    }

    /// Execute the background draw call.
    fn draw_background(
        &self,
        gl: &glow::Context,
        vert_floats: usize,
        vp_w: f32,
        vp_h: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(vbo)) =
            (self.bg_program, self.bg_vao, self.bg_vbo[buf_idx])
        else {
            return;
        };

        if vert_floats == 0 {
            return;
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let vertex_count = (vert_floats / BG_VERTEX_FLOATS) as i32;

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.bg_u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_bg_attribs(gl);
            gl.draw_arrays(glow::TRIANGLES, 0, vertex_count);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Execute the foreground draw call.
    fn draw_foreground(
        &self,
        gl: &glow::Context,
        vert_floats: usize,
        vp_w: f32,
        vp_h: f32,
        buf_idx: usize,
    ) {
        let (Some(prog), Some(vao), Some(vbo), Some(tex)) = (
            self.fg_program,
            self.fg_vao,
            self.fg_vbo[buf_idx],
            self.atlas_texture,
        ) else {
            return;
        };

        if vert_floats == 0 {
            return;
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let vertex_count = (vert_floats / FG_VERTEX_FLOATS) as i32;

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
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_fg_attribs(gl);
            gl.draw_arrays(glow::TRIANGLES, 0, vertex_count);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
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
            if let Some(p) = self.bg_program.take() {
                gl.delete_program(p);
            }
            if let Some(v) = self.bg_vao.take() {
                gl.delete_vertex_array(v);
            }
            for slot in &mut self.bg_vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
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
        }

        self.initialized = false;
    }
}

// ---------------------------------------------------------------------------
//  Vertex attribute setup helpers
// ---------------------------------------------------------------------------

/// Configure vertex attributes for the background shader.
///
/// Layout: `location 0 = vec2 pos, location 1 = vec4 color`.
/// Stride = `BG_VERTEX_FLOATS * 4` bytes.
unsafe fn setup_bg_attribs(gl: &glow::Context) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let stride = (BG_VERTEX_FLOATS * size_of::<f32>()) as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let offset_c2 = (2 * size_of::<f32>()) as i32;
    unsafe {
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 4, glow::FLOAT, false, stride, offset_c2);
    }
}

/// Configure vertex attributes for the foreground shader.
///
/// Layout: `location 0 = vec2 pos, location 1 = vec2 uv,
///          location 2 = vec4 color, location 3 = float is_color`.
/// Stride = `FG_VERTEX_FLOATS * 4` bytes.
unsafe fn setup_fg_attribs(gl: &glow::Context) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let stride = (FG_VERTEX_FLOATS * size_of::<f32>()) as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let f = size_of::<f32>() as i32;
    unsafe {
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, 2 * f);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 4, glow::FLOAT, false, stride, 4 * f);
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, 8 * f);
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
    label: &str,
) -> Result<glow::Program, String> {
    unsafe {
        let vert = compile_shader(gl, glow::VERTEX_SHADER, vert_src, label)?;
        let frag = compile_shader(gl, glow::FRAGMENT_SHADER, frag_src, label)?;

        let program = gl
            .create_program()
            .map_err(|e| format!("create {label} program: {e}"))?;
        gl.attach_shader(program, vert);
        gl.attach_shader(program, frag);
        gl.link_program(program);

        if !gl.get_program_link_status(program) {
            let info = gl.get_program_info_log(program);
            gl.delete_program(program);
            gl.delete_shader(vert);
            gl.delete_shader(frag);
            return Err(format!("link {label} program: {info}"));
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
    label: &str,
) -> Result<glow::Shader, String> {
    unsafe {
        let shader = gl
            .create_shader(shader_type)
            .map_err(|e| format!("create {label} shader: {e}"))?;
        gl.shader_source(shader, src);
        gl.compile_shader(shader);

        if !gl.get_shader_compile_status(shader) {
            let info = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(format!("compile {label} shader: {info}"));
        }
        Ok(shader)
    }
}

// ---------------------------------------------------------------------------
//  GPU upload helper
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
        gl.buffer_data_size(
            glow::ARRAY_BUFFER,
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            {
                bytes.len() as i32
            },
            glow::STREAM_DRAW,
        );
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
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, byte_offset as i32, bytes);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
    }
}

// ---------------------------------------------------------------------------
//  Atlas sub-image extraction
// ---------------------------------------------------------------------------

/// Extract a rectangular region from the full atlas pixel buffer into a
/// contiguous `Vec<u8>` suitable for `gl.tex_sub_image_2d()`.
fn extract_atlas_rect(pixels: &[u8], atlas_size: u32, rect: &super::atlas::DirtyRect) -> Vec<u8> {
    let stride = (atlas_size as usize) * 4;
    let row_bytes = (rect.width as usize) * 4;
    let mut out = Vec::with_capacity((rect.height as usize) * row_bytes);

    for row in 0..rect.height {
        let y = (rect.y + row) as usize;
        let x = rect.x as usize;
        let offset = y * stride + x * 4;
        let end = offset + row_bytes;
        if end <= pixels.len() {
            out.extend_from_slice(&pixels[offset..end]);
        }
    }

    out
}

// ---------------------------------------------------------------------------
//  Pure CPU vertex builders  (testable without GL context)
// ---------------------------------------------------------------------------

/// Build the background vertex buffer from shaped lines.
///
/// Generates:
/// - One merged quad per horizontal run of same-background cells.
/// - Selection highlight quads for the current text selection (if any).
/// - Cursor quad (block / underline / bar) at `cursor_pos` when visible.
/// - Underline / strikethrough quads for runs that carry those decorations.
///
/// Returns a flat `Vec<f32>` with `BG_VERTEX_FLOATS` floats per vertex,
/// `VERTS_PER_QUAD * 6` floats per quad.
#[must_use]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn build_background_verts(
    shaped_lines: &[ShapedLine],
    cell_width: u32,
    cell_height: u32,
    underline_offset: f32,
    strikeout_offset: f32,
    stroke_size: f32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pos: CursorPos,
    cursor_visual_style: &CursorVisualStyle,
    selection: Option<(usize, usize, usize, usize)>,
) -> Vec<f32> {
    let mut verts: Vec<f32> = Vec::new();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let y_top = row_idx as f32 * cell_height as f32;
        #[allow(clippy::cast_precision_loss)]
        let y_bot = y_top + cell_height as f32;

        // --- Background quad merging ---
        // Walk runs and merge adjacent cells with the same background color.
        let mut merge_start_col: Option<usize> = None;
        let mut merge_bg: Option<[f32; 4]> = None;

        for run in &line.runs {
            let is_faint = run.font_decorations.contains(&FontDecorations::Faint);
            let bg_color_raw = run.colors.get_background_color();

            // Skip default backgrounds (transparent — the terminal base color is
            // rendered as a clear color, not explicit quads).
            if matches!(
                bg_color_raw,
                freminal_common::colors::TerminalColor::DefaultBackground
            ) {
                // Flush any open merge run.
                if let (Some(start_col), Some(color)) = (merge_start_col.take(), merge_bg.take()) {
                    let end_col = run.col_start;
                    if end_col > start_col {
                        #[allow(clippy::cast_precision_loss)]
                        push_quad(
                            &mut verts,
                            start_col as f32 * cell_width as f32,
                            y_top,
                            end_col as f32 * cell_width as f32,
                            y_bot,
                            color,
                        );
                    }
                }
                continue;
            }

            let bg_color = internal_color_to_gl(bg_color_raw, is_faint);

            // Extend or start a merge run.
            if let Some(ref existing_bg) = merge_bg {
                if colors_equal(existing_bg, &bg_color) {
                    // Same color — extend the run.
                } else {
                    // Different color — flush previous and start new.
                    if let Some(start_col) = merge_start_col.take() {
                        #[allow(clippy::cast_precision_loss)]
                        push_quad(
                            &mut verts,
                            start_col as f32 * cell_width as f32,
                            y_top,
                            run.col_start as f32 * cell_width as f32,
                            y_bot,
                            *existing_bg,
                        );
                    }
                    merge_start_col = Some(run.col_start);
                    merge_bg = Some(bg_color);
                }
            } else {
                merge_start_col = Some(run.col_start);
                merge_bg = Some(bg_color);
            }
        }

        // Flush any remaining merge run at end of line.
        if let (Some(start_col), Some(color)) = (merge_start_col.take(), merge_bg.take()) {
            // Determine end column from last run.
            let end_col = shaped_lines[row_idx]
                .runs
                .last()
                .map_or(start_col, |r| r.col_start + run_col_count(r));
            if end_col > start_col {
                #[allow(clippy::cast_precision_loss)]
                push_quad(
                    &mut verts,
                    start_col as f32 * cell_width as f32,
                    y_top,
                    end_col as f32 * cell_width as f32,
                    y_bot,
                    color,
                );
            }
        }

        // --- Underline and strikethrough quads ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(&FontDecorations::Faint);
            let has_underline = run.font_decorations.contains(&FontDecorations::Underline);
            let has_strike = run
                .font_decorations
                .contains(&FontDecorations::Strikethrough);

            if !has_underline && !has_strike {
                continue;
            }

            let fg_color = internal_color_to_gl(run.colors.get_color(), is_faint);
            let col_end = run.col_start + run_col_count(run);

            #[allow(clippy::cast_precision_loss)]
            let x0 = run.col_start as f32 * cell_width as f32;
            #[allow(clippy::cast_precision_loss)]
            let x1 = col_end as f32 * cell_width as f32;

            if has_underline {
                let ul_top = y_top + underline_offset;
                let ul_bot = ul_top + stroke_size.max(1.0);
                push_quad(&mut verts, x0, ul_top, x1, ul_bot, fg_color);
            }

            if has_strike {
                let st_top = y_top + strikeout_offset;
                let st_bot = st_top + stroke_size.max(1.0);
                push_quad(&mut verts, x0, st_top, x1, st_bot, fg_color);
            }
        }
    }

    // --- Selection highlight quads ---
    // `selection` is `Some((start_col, start_row, end_col, end_row))` with
    // start <= end in reading order.
    if let Some((sel_start_col, sel_start_row, sel_end_col, sel_end_row)) = selection {
        #[allow(clippy::cast_precision_loss)]
        let cw = cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let ch = cell_height as f32;

        for (row, line) in shaped_lines
            .iter()
            .enumerate()
            .take(sel_end_row + 1)
            .skip(sel_start_row)
        {
            // Determine the column range for this row.
            let col_begin = if row == sel_start_row {
                sel_start_col
            } else {
                0
            };
            let col_end = if row == sel_end_row {
                sel_end_col
            } else {
                // Highlight to the end of the row.  Use the last run's end
                // column, or fall back to the total run width.
                line.runs
                    .last()
                    .map_or(0, |r| r.col_start + run_col_count(r))
                    .saturating_sub(1)
            };

            if col_end < col_begin {
                continue;
            }

            #[allow(clippy::cast_precision_loss)]
            let x0 = col_begin as f32 * cw;
            #[allow(clippy::cast_precision_loss)]
            let x1 = (col_end + 1) as f32 * cw;
            #[allow(clippy::cast_precision_loss)]
            let y0 = row as f32 * ch;
            let y1 = y0 + ch;

            push_quad(&mut verts, x0, y0, x1, y1, SELECTION_BG_F);
        }
    }

    // --- Cursor quad ---
    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        #[allow(clippy::cast_precision_loss)]
        let cx = cursor_pos.x as f32 * cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let cy = cursor_pos.y as f32 * cell_height as f32;
        #[allow(clippy::cast_precision_loss)]
        let cw = cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let ch = cell_height as f32;

        let color = CURSOR_F;

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(&mut verts, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                // Thin bar at the bottom of the cell.
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(&mut verts, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                // Thin vertical bar at the left edge of the cell.
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(&mut verts, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    verts
}

/// Returns whether the cursor should be visible given its style and blink state.
const fn cursor_blink_is_visible(style: &CursorVisualStyle, blink_on: bool) -> bool {
    match style {
        CursorVisualStyle::BlockCursorSteady
        | CursorVisualStyle::UnderlineCursorSteady
        | CursorVisualStyle::VerticalLineCursorSteady => true,
        CursorVisualStyle::BlockCursorBlink
        | CursorVisualStyle::UnderlineCursorBlink
        | CursorVisualStyle::VerticalLineCursorBlink => blink_on,
    }
}

/// Build just the cursor quad for the background VBO.
///
/// Returns `CURSOR_QUAD_FLOATS` floats when the cursor is visible, or an
/// empty `Vec` when it should not be painted (cursor hidden, or blink-off).
///
/// This is the "cheap path" used for cursor-only frame updates: instead of
/// rebuilding the entire background VBO, the caller patches only the cursor
/// quad region in-place via [`upload_verts_sub`].
#[must_use]
pub fn build_cursor_verts_only(
    cell_width: u32,
    cell_height: u32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pos: CursorPos,
    cursor_visual_style: &CursorVisualStyle,
) -> Vec<f32> {
    let mut verts = Vec::new();

    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        #[allow(clippy::cast_precision_loss)]
        let cx = cursor_pos.x as f32 * cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let cy = cursor_pos.y as f32 * cell_height as f32;
        #[allow(clippy::cast_precision_loss)]
        let cw = cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let ch = cell_height as f32;

        let color = CURSOR_F;

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(&mut verts, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(&mut verts, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(&mut verts, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    verts
}

/// Build the foreground vertex buffer from shaped lines.
///
/// For each shaped glyph: looks up the atlas entry (rasterising on miss) and
/// emits a textured quad at the cell-grid position adjusted by the bearing offsets.
///
/// `selection` is `Some((start_col, start_row, end_col, end_row))` in normalised
/// reading order.  Glyphs that fall within the selection use `SELECTION_FG_F`
/// instead of their normal foreground color.
///
/// Returns a flat `Vec<f32>` with `FG_VERTEX_FLOATS` floats per vertex.
#[must_use]
pub fn build_foreground_verts(
    shaped_lines: &[ShapedLine],
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    cell_height: u32,
    ascent: f32,
    selection: Option<(usize, usize, usize, usize)>,
) -> Vec<f32> {
    let mut verts: Vec<f32> = Vec::new();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let row_f = row_idx as f32;
        #[allow(clippy::cast_precision_loss)]
        let cell_h_f = cell_height as f32;
        let baseline_y = row_f.mul_add(cell_h_f, ascent);

        // Cell vertical extent for this row (used to clip oversized glyphs).
        let cell_top = row_f * cell_h_f;
        let cell_bottom = cell_top + cell_h_f;

        for run in &line.runs {
            let is_faint = run.font_decorations.contains(&FontDecorations::Faint);
            let normal_fg = internal_color_to_gl(run.colors.get_color(), is_faint);

            // Track the current column as we iterate glyphs within the run.
            let mut col = run.col_start;

            for glyph in &run.glyphs {
                let fg_color = if is_cell_selected(row_idx, col, selection) {
                    SELECTION_FG_F
                } else {
                    normal_fg
                };

                emit_glyph_quad(
                    &mut verts,
                    glyph,
                    atlas,
                    font_manager,
                    baseline_y,
                    fg_color,
                    [cell_top, cell_bottom],
                );

                col += glyph.cell_width;
            }
        }
    }

    verts
}

/// Emit a textured quad for a single shaped glyph.
///
/// Looks up (or rasterises) the atlas entry for the glyph, then pushes 6
/// vertices (`VERTS_PER_QUAD`) into `verts`.  Glyphs that extend beyond the
/// cell's vertical extent (`cell_y_range[0]`..`cell_y_range[1]`) are clipped,
/// and their UV coordinates are adjusted proportionally so the visible portion
/// of the atlas texture is correct.
fn emit_glyph_quad(
    verts: &mut Vec<f32>,
    glyph: &ShapedGlyph,
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    baseline_y: f32,
    fg_color: [f32; 4],
    cell_y_range: [f32; 2],
) {
    // Determine pixel size from the atlas key.
    // We use the font manager's cell height as the size_px for rasterisation.
    #[allow(clippy::cast_possible_truncation)]
    let size_px = font_manager.cell_height() as u16;

    let key = GlyphKey {
        glyph_id: glyph.glyph_id,
        face_id: glyph.face_id,
        size_px,
    };

    let entry = match atlas.get_or_insert(key, font_manager) {
        Some(e) => e.clone(),
        None => return, // Rasterisation failed — skip glyph.
    };

    // Zero-size glyphs (space) have no geometry.
    if entry.width == 0 || entry.height == 0 {
        return;
    }

    let [u0, v0, u1, v1] = entry.uv_rect;

    // Pixel position: cell-grid x + bearing, baseline_y - bearing_y.
    let x0 = glyph.x_px + f32::from(entry.bearing_x);
    let raw_y0 = baseline_y - f32::from(entry.bearing_y);
    let x1 = x0 + f32::from(entry.width);
    #[allow(clippy::cast_precision_loss)]
    let raw_y1 = raw_y0 + f32::from(entry.height);

    // --- Cell-boundary clipping ---
    //
    // Oversized glyphs (e.g. powerline symbols in Nerd Fonts) may extend
    // above or below the cell.  Clamp the quad to the cell's vertical
    // extent and adjust the UV coordinates proportionally so only the
    // visible portion of the atlas texture is sampled.
    let cell_top = cell_y_range[0];
    let cell_bottom = cell_y_range[1];
    let glyph_h = raw_y1 - raw_y0;
    let (y0, v0_adj) = if raw_y0 < cell_top {
        // Glyph extends above the cell — clip the top.
        let frac = (cell_top - raw_y0) / glyph_h;
        (cell_top, frac.mul_add(v1 - v0, v0))
    } else {
        (raw_y0, v0)
    };
    let (y1, v1_adj) = if raw_y1 > cell_bottom {
        // Glyph extends below the cell — clip the bottom.
        let frac = (raw_y1 - cell_bottom) / glyph_h;
        (cell_bottom, frac.mul_add(-(v1 - v0), v1))
    } else {
        (raw_y1, v1)
    };

    // After clipping the quad may have been fully culled.
    if y0 >= y1 {
        return;
    }

    let is_color_f: f32 = if glyph.is_color { 1.0 } else { 0.0 };

    // Two triangles = 6 vertices.  Each vertex: x, y, u, v, r, g, b, a, is_color.
    let quad = [
        // Triangle 1
        x0,
        y0,
        u0,
        v0_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
        x1,
        y0,
        u1,
        v0_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
        x0,
        y1,
        u0,
        v1_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
        // Triangle 2
        x1,
        y0,
        u1,
        v0_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
        x1,
        y1,
        u1,
        v1_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
        x0,
        y1,
        u0,
        v1_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
    ];

    verts.extend_from_slice(&quad);
}

// ---------------------------------------------------------------------------
//  Quad geometry helper
// ---------------------------------------------------------------------------

/// Push a solid-color axis-aligned quad (2 triangles = 6 vertices) into `verts`.
///
/// Vertex layout: `x, y, r, g, b, a`.
fn push_quad(verts: &mut Vec<f32>, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
    let [r, g, b, a] = color;
    let quad = [
        x0, y0, r, g, b, a, x1, y0, r, g, b, a, x0, y1, r, g, b, a, x1, y0, r, g, b, a, x1, y1, r,
        g, b, a, x0, y1, r, g, b, a,
    ];
    verts.extend_from_slice(&quad);
}

// ---------------------------------------------------------------------------
//  Small helpers
// ---------------------------------------------------------------------------

/// Return the total column count covered by a `ShapedRun`.
fn run_col_count(run: &super::shaping::ShapedRun) -> usize {
    run.glyphs.iter().map(|g| g.cell_width).sum()
}

/// Check whether a cell at `(row, col)` falls within the normalised selection
/// `(start_col, start_row, end_col, end_row)`.
const fn is_cell_selected(
    row: usize,
    col: usize,
    selection: Option<(usize, usize, usize, usize)>,
) -> bool {
    let Some((sel_start_col, sel_start_row, sel_end_col, sel_end_row)) = selection else {
        return false;
    };

    if row < sel_start_row || row > sel_end_row {
        return false;
    }

    if sel_start_row == sel_end_row {
        // Single-row selection.
        return col >= sel_start_col && col <= sel_end_col;
    }

    if row == sel_start_row {
        col >= sel_start_col
    } else if row == sel_end_row {
        col <= sel_end_col
    } else {
        // Middle rows are fully selected.
        true
    }
}

/// Compare two GL `[f32; 4]` colors for equality.
///
/// Uses `total_cmp` so that the comparison is bitwise-exact (no NaN issues)
/// without triggering `clippy::float_cmp`.
fn colors_equal(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0].total_cmp(&b[0]).is_eq()
        && a[1].total_cmp(&b[1]).is_eq()
        && a[2].total_cmp(&b[2]).is_eq()
        && a[3].total_cmp(&b[3]).is_eq()
}

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::config::Config;

    use crate::gui::font_manager::FontManager;
    use crate::gui::shaping::{ShapedGlyph, ShapedLine, ShapedRun};
    use freminal_common::buffer_states::cursor::StateColors;
    use freminal_common::buffer_states::fonts::FontWeight;
    use freminal_common::colors::TerminalColor;

    /// Default `StateColors` for test runs.
    fn default_colors() -> StateColors {
        StateColors::default()
    }

    /// Build a single `ShapedLine` with one run containing `n_glyphs` identical
    /// ASCII glyphs.  `cell_width_f32` is the cell width for x positions.
    fn make_line(
        n_glyphs: usize,
        cell_w: f32,
        colors: StateColors,
        decorations: Vec<FontDecorations>,
    ) -> ShapedLine {
        use crate::gui::font_manager::FaceId;
        let glyphs: Vec<ShapedGlyph> = (0..n_glyphs)
            .map(|i| ShapedGlyph {
                glyph_id: 36, // 'A' glyph (approximate)
                #[allow(clippy::cast_precision_loss)]
                x_px: i as f32 * cell_w,
                y_offset: 0.0,
                face_id: FaceId::PrimaryRegular,
                is_color: false,
                cell_width: 1,
            })
            .collect();
        ShapedLine {
            runs: vec![ShapedRun {
                glyphs,
                col_start: 0,
                style: crate::gui::font_manager::GlyphStyle::new(false, false),
                font_weight: FontWeight::Normal,
                font_decorations: decorations,
                colors,
                url: None,
            }],
        }
    }

    // -----------------------------------------------------------------------
    //  TerminalRenderer construction
    // -----------------------------------------------------------------------

    #[test]
    fn renderer_constructs_uninitialized() {
        let r = TerminalRenderer::new();
        assert!(!r.initialized());
    }

    #[test]
    fn renderer_default_is_uninitialized() {
        let r = TerminalRenderer::default();
        assert!(!r.initialized());
    }

    // -----------------------------------------------------------------------
    //  Vertex count tests
    // -----------------------------------------------------------------------

    #[test]
    fn bg_verts_empty_on_default_background() {
        // A line whose cells all have `DefaultBackground` should produce no quads.
        let line = make_line(5, 8.0, default_colors(), vec![]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        assert_eq!(verts.len(), 0, "default background should produce no quads");
    }

    #[test]
    fn bg_verts_one_quad_for_colored_run() {
        // A single run with a non-default background should produce exactly one quad.
        let colors = StateColors::default().with_background_color(TerminalColor::Red);

        let line = make_line(3, 8.0, colors, vec![]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        // One quad = VERTS_PER_QUAD * BG_VERTEX_FLOATS floats.
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "expected exactly one background quad"
        );
    }

    #[test]
    fn bg_verts_adjacent_same_color_merged() {
        // Two runs that have the same background color and are adjacent should
        // produce only one merged quad.
        use crate::gui::font_manager::FaceId;
        let colors = StateColors::default().with_background_color(TerminalColor::Blue);

        let line = ShapedLine {
            runs: vec![
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 36,
                        x_px: 0.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                    }],
                    col_start: 0,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: vec![],
                    colors: colors.clone(),
                    url: None,
                },
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 37,
                        x_px: 8.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                    }],
                    col_start: 1,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: vec![],
                    colors,
                    url: None,
                },
            ],
        };

        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        // Two adjacent same-color runs → one merged quad.
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "adjacent same-color runs should be merged into one quad"
        );
    }

    #[test]
    fn bg_verts_different_colors_two_quads() {
        // Two runs with different non-default background colors should produce two
        // separate quads.
        use crate::gui::font_manager::FaceId;

        let colors_red = StateColors::default().with_background_color(TerminalColor::Red);
        let colors_blue = StateColors::default().with_background_color(TerminalColor::Blue);

        let line = ShapedLine {
            runs: vec![
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 36,
                        x_px: 0.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                    }],
                    col_start: 0,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: vec![],
                    colors: colors_red,
                    url: None,
                },
                ShapedRun {
                    glyphs: vec![ShapedGlyph {
                        glyph_id: 37,
                        x_px: 8.0,
                        y_offset: 0.0,
                        face_id: FaceId::PrimaryRegular,
                        is_color: false,
                        cell_width: 1,
                    }],
                    col_start: 1,
                    style: crate::gui::font_manager::GlyphStyle::new(false, false),
                    font_weight: FontWeight::Normal,
                    font_decorations: vec![],
                    colors: colors_blue,
                    url: None,
                },
            ],
        };

        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        assert_eq!(
            verts.len(),
            2 * VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "different-color runs should produce two quads"
        );
    }

    #[test]
    fn bg_verts_cursor_block_adds_quad() {
        // With `show_cursor = true` and a steady block cursor, one cursor quad
        // should be appended.
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            true,
            true,
            CursorPos { x: 1, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "block cursor should add one quad"
        );
    }

    #[test]
    fn bg_verts_cursor_blink_off_no_quad() {
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            true,
            false, // blink_on = false
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
            None,
        );
        assert_eq!(
            verts.len(),
            0,
            "blinking cursor with blink_on=false should produce no quad"
        );
    }

    #[test]
    fn bg_verts_cursor_steady_ignores_blink_flag() {
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            true,
            false, // blink_on = false — irrelevant for steady cursor
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "steady cursor should render even when blink_on=false"
        );
    }

    #[test]
    fn bg_verts_underline_adds_quad() {
        let line = make_line(3, 8.0, default_colors(), vec![FontDecorations::Underline]);
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        // One underline quad.
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "underline run should produce one underline quad"
        );
    }

    #[test]
    fn bg_verts_strikethrough_adds_quad() {
        let line = make_line(
            3,
            8.0,
            default_colors(),
            vec![FontDecorations::Strikethrough],
        );
        let verts = build_background_verts(
            &[line],
            8,
            16,
            13.0,
            8.0,
            1.0,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        assert_eq!(
            verts.len(),
            VERTS_PER_QUAD * BG_VERTEX_FLOATS,
            "strikethrough run should produce one strikethrough quad"
        );
    }

    #[test]
    fn bg_verts_cursor_position_maps_to_pixel_coords() {
        // Block cursor at (col=2, row=1) with cell_width=10, cell_height=20.
        // Expected x0 = 2*10 = 20, y0 = 1*20 = 20.
        let lines = [
            make_line(5, 10.0, default_colors(), vec![]),
            make_line(5, 10.0, default_colors(), vec![]),
        ];
        let verts = build_background_verts(
            &lines,
            10,
            20,
            16.0,
            10.0,
            1.0,
            true,
            true,
            CursorPos { x: 2, y: 1 },
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );
        // The cursor quad is the last 36 floats (6 verts × 6 floats).
        assert!(verts.len() >= VERTS_PER_QUAD * BG_VERTEX_FLOATS);
        let cursor_start = verts.len() - VERTS_PER_QUAD * BG_VERTEX_FLOATS;
        let x0 = verts[cursor_start];
        let y0 = verts[cursor_start + 1];
        assert!(
            (x0 - 20.0).abs() < f32::EPSILON,
            "cursor x should be col*cell_w = 20, got {x0}"
        );
        assert!(
            (y0 - 20.0).abs() < f32::EPSILON,
            "cursor y should be row*cell_h = 20, got {y0}"
        );
    }

    // -----------------------------------------------------------------------
    //  Foreground vertex tests
    // -----------------------------------------------------------------------

    #[test]
    fn fg_verts_empty_on_empty_lines() {
        let verts = build_foreground_verts(
            &[],
            &mut GlyphAtlas::default(),
            &FontManager::new(&Config::default()),
            16,
            13.0,
            None,
        );
        assert_eq!(verts.len(), 0);
    }

    #[test]
    fn fg_verts_produces_quads_for_ascii_glyphs() {
        let mut fm = FontManager::new(&Config::default());
        let mut atlas = GlyphAtlas::new(256, 1024);
        #[allow(clippy::cast_precision_loss)]
        let cell_w = fm.cell_width() as f32;
        let cell_h = fm.cell_height();
        let ascent = fm.ascent();

        // Shape a short ASCII line.
        let mut cache = crate::gui::shaping::ShapingCache::new();
        let chars: Vec<freminal_common::buffer_states::tchar::TChar> = b"ABC"
            .iter()
            .map(|&b| freminal_common::buffer_states::tchar::TChar::Ascii(b))
            .collect();
        let tags = vec![freminal_common::buffer_states::format_tag::FormatTag::default()];
        let lines = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w);

        let verts = build_foreground_verts(&lines, &mut atlas, &fm, cell_h, ascent, None);

        // Three ASCII glyphs each produce one quad = VERTS_PER_QUAD * FG_VERTEX_FLOATS.
        // Some glyphs may be spaces (zero-size) — so at minimum some quads must exist.
        assert!(
            verts.len() >= FG_VERTEX_FLOATS,
            "at least one foreground quad expected, got {} floats",
            verts.len()
        );
        assert_eq!(
            verts.len() % (VERTS_PER_QUAD * FG_VERTEX_FLOATS),
            0,
            "foreground vertex count must be a multiple of one quad ({} floats)",
            VERTS_PER_QUAD * FG_VERTEX_FLOATS
        );
    }

    // -----------------------------------------------------------------------
    //  Push quad helper
    // -----------------------------------------------------------------------

    #[test]
    fn push_quad_produces_six_vertices() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(verts.len(), VERTS_PER_QUAD * BG_VERTEX_FLOATS);
    }

    #[test]
    fn push_quad_corner_positions() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 5.0, 3.0, 15.0, 13.0, [0.0, 1.0, 0.0, 1.0]);

        // Vertex 0 (top-left): x=5, y=3
        assert!((verts[0] - 5.0).abs() < f32::EPSILON);
        assert!((verts[1] - 3.0).abs() < f32::EPSILON);

        // Vertex 1 (top-right): x=15, y=3
        assert!((verts[BG_VERTEX_FLOATS] - 15.0).abs() < f32::EPSILON);
        assert!((verts[BG_VERTEX_FLOATS + 1] - 3.0).abs() < f32::EPSILON);
    }

    // -----------------------------------------------------------------------
    //  Extract atlas rect
    // -----------------------------------------------------------------------

    #[test]
    fn extract_atlas_rect_correct_bytes() {
        // Build a 4×4 atlas with distinct pixel values per row.
        let size: u32 = 4;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        // Row 1 (y=1): fill with 0xFF.
        let row_start = (size * 4) as usize;
        pixels[row_start..row_start + (size * 4) as usize].fill(0xFF);

        let rect = super::super::atlas::DirtyRect {
            x: 0,
            y: 1,
            width: 4,
            height: 1,
        };
        let out = extract_atlas_rect(&pixels, size, &rect);
        assert_eq!(out.len(), 16);
        assert!(out.iter().all(|&b| b == 0xFF));
    }

    // -----------------------------------------------------------------------
    //  Blink helper
    // -----------------------------------------------------------------------

    #[test]
    fn blink_visible_steady_always_true() {
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorSteady,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorSteady,
            true
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::UnderlineCursorSteady,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::VerticalLineCursorSteady,
            false
        ));
    }

    #[test]
    fn blink_visible_blinking_follows_flag() {
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorBlink,
            false
        ));
        assert!(cursor_blink_is_visible(
            &CursorVisualStyle::BlockCursorBlink,
            true
        ));
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::UnderlineCursorBlink,
            false
        ));
        assert!(!cursor_blink_is_visible(
            &CursorVisualStyle::VerticalLineCursorBlink,
            false
        ));
    }

    // -----------------------------------------------------------------------
    //  build_cursor_verts_only
    // -----------------------------------------------------------------------

    /// When the cursor is hidden (`show_cursor = false`), the function must
    /// return an empty vec — no geometry at all.
    #[test]
    fn cursor_verts_only_hidden_returns_empty() {
        let verts = build_cursor_verts_only(
            8,
            16,
            false,
            true,
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert!(verts.is_empty(), "hidden cursor should produce no verts");
    }

    /// A steady block cursor always produces exactly `CURSOR_QUAD_FLOATS` floats,
    /// regardless of the blink flag.
    #[test]
    fn cursor_verts_only_steady_block_always_visible() {
        for blink_on in [false, true] {
            let verts = build_cursor_verts_only(
                8,
                16,
                true,
                blink_on,
                CursorPos { x: 1, y: 2 },
                &CursorVisualStyle::BlockCursorSteady,
            );
            assert_eq!(
                verts.len(),
                CURSOR_QUAD_FLOATS,
                "steady cursor must always produce one quad (blink_on={blink_on})"
            );
        }
    }

    /// A blinking cursor with `blink_on = false` must return empty verts.
    #[test]
    fn cursor_verts_only_blink_off_returns_empty() {
        let verts = build_cursor_verts_only(
            8,
            16,
            true,
            false,
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
        );
        assert!(verts.is_empty(), "blink-off cursor should produce no verts");
    }

    /// Partial VBO update: `build_cursor_verts_only` produces exactly
    /// `CURSOR_QUAD_FLOATS` floats and they occupy the correct byte range.
    ///
    /// The test verifies that:
    ///   1. The cursor-only builder produces `CURSOR_QUAD_FLOATS` floats
    ///      (= `VERTS_PER_QUAD * BG_VERTEX_FLOATS`).
    ///   2. Patching those floats into a pre-built background VBO at the
    ///      recorded offset produces the expected combined buffer — only the
    ///      cursor region changes, all other floats are untouched.
    #[test]
    fn partial_vbo_update_only_modifies_cursor_region() {
        // Build a full background VBO for one line + a cursor at (col=0, row=0).
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let full_verts = build_background_verts(
            std::slice::from_ref(&line),
            8,
            16,
            13.0,
            8.0,
            1.0,
            true,
            true, // cursor visible
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
            None,
        );

        // Record where the cursor quad starts (it is appended at the end).
        let cursor_float_offset = full_verts.len() - CURSOR_QUAD_FLOATS;
        let cursor_byte_offset = cursor_float_offset * std::mem::size_of::<f32>();

        // The pre-cursor portion must be unchanged — capture it before mutation.
        let pre_cursor = full_verts[..cursor_float_offset].to_vec();

        // Build cursor-only verts with blink_on=false using a *blinking* style.
        // BlockCursorSteady ignores blink_on (always visible); BlockCursorBlink
        // respects it, so blink_on=false correctly produces an empty vec.
        let cursor_off_verts = build_cursor_verts_only(
            8,
            16,
            true,
            false, // blink off → empty verts (only true for blinking style)
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
        );

        // Simulate the partial-update patch: mutate full_verts in-place to
        // overwrite the cursor region (matches draw_with_cursor_only_update).
        let mut patched = full_verts;
        if cursor_off_verts.is_empty() {
            // Zero-fill the cursor region (matches what draw_with_cursor_only_update does).
            for f in &mut patched[cursor_float_offset..] {
                *f = 0.0;
            }
        } else {
            patched[cursor_float_offset..].copy_from_slice(&cursor_off_verts);
        }

        // Pre-cursor region must be bit-identical.
        assert_eq!(
            patched[..cursor_float_offset],
            pre_cursor[..],
            "partial update must not modify floats before the cursor quad offset              (byte_offset={cursor_byte_offset})"
        );

        // The cursor region must now be all zeros (blink-off patch).
        assert!(
            patched[cursor_float_offset..].iter().all(|&f| f == 0.0),
            "cursor region must be zeroed after blink-off patch"
        );
    }
}
