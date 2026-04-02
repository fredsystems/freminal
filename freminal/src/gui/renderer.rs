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
//! The CPU-side instance/vertex builders (`build_background_instances` / `build_foreground_instances`)
//! are pure functions and are fully testable without a GL context.

use eframe::glow::{self, HasContext};
use freminal_common::buffer_states::cursor::CursorPos;
use freminal_common::buffer_states::fonts::FontDecorations;
use freminal_common::cursor::CursorVisualStyle;
use tracing::error;

use super::atlas::{GlyphAtlas, GlyphKey};
use super::colors::{cursor_f, internal_color_to_gl, selection_bg_f, selection_fg_f};
use super::font_manager::FontManager;
use super::shaping::{ShapedGlyph, ShapedLine};
use freminal_common::buffer_states::fonts::BlinkState;
use freminal_common::themes::ThemePalette;
use freminal_terminal_emulator::{ImagePlacement, InlineImage};

// ---------------------------------------------------------------------------
//  GLSL shaders  (GL 3.3 core profile)
// ---------------------------------------------------------------------------

/// Decoration pass: solid-color quads (underlines, strikethrough, cursor, selection).
///
/// The original non-instanced background shader repurposed for decoration/cursor
/// elements that have sub-cell geometry.  Cell backgrounds are now drawn by the
/// instanced background pass below.
///
/// Vertex layout: `vec2 a_pos, vec4 a_color`  (stride = 6 × f32 = 24 bytes)
const DECO_VERT_SRC: &str = r"#version 330 core
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

const DECO_FRAG_SRC: &str = r"#version 330 core
in vec4 v_color;
out vec4 frag_color;

void main() {
    // Premultiplied alpha output.
    frag_color = vec4(v_color.rgb * v_color.a, v_color.a);
}
";

/// Instanced background pass: one draw call for all cell background quads.
///
/// A static unit quad (`UNIT_QUAD`) is scaled to cell size by the vertex shader.
/// Per-instance attributes supply the grid position and resolved RGBA color.
///
/// Vertex layout (per-vertex, divisor 0): `vec2 a_pos`
/// Instance layout (per-instance, divisor 1): `vec2 a_cell_pos, vec4 a_bg_color`
const BG_INST_VERT_SRC: &str = r"#version 330 core

// Static unit-quad vertex (one of 6 triangle vertices for a quad).
layout(location = 0) in vec2 a_pos;

// Per-instance attributes (divisor = 1):
layout(location = 1) in vec2  a_cell_pos;    // (col, row) -- integer grid position
layout(location = 2) in vec4  a_bg_color;    // resolved RGBA

uniform vec2  u_viewport_size;
uniform float u_cell_width;
uniform float u_cell_height;

out vec4  v_bg_color;

void main() {
    vec2 cell_origin = a_cell_pos * vec2(u_cell_width, u_cell_height);
    vec2 pixel_pos = cell_origin + a_pos * vec2(u_cell_width, u_cell_height);
    vec2 ndc = (pixel_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_bg_color = a_bg_color;
}
";

const BG_INST_FRAG_SRC: &str = r"#version 330 core

in vec4  v_bg_color;

out vec4 frag_color;

uniform float u_bg_opacity;

void main() {
    float alpha = v_bg_color.a * u_bg_opacity;
    frag_color = vec4(v_bg_color.rgb * alpha, alpha);
}
";

/// Foreground pass: instanced textured glyph quads sampled from the atlas.
///
/// Per-vertex: `vec2 a_pos` (unit quad in [0,1]², divisor 0).
/// Per-instance (divisor 1):
///   location 1: `vec2  a_glyph_origin` — pixel position of the glyph quad
///   location 2: `vec2  a_glyph_size`   — pixel size of the glyph quad
///   location 3: `vec4  a_uv_rect`      — (u0, v0, u1, v1) atlas UV
///   location 4: `vec4  a_fg_color`     — RGBA foreground color
///   location 5: `float a_is_color`     — 1.0 for color emoji, 0.0 for mono
const FG_VERT_SRC: &str = r"#version 330 core
layout(location = 0) in vec2  a_pos;
layout(location = 1) in vec2  a_glyph_origin;
layout(location = 2) in vec2  a_glyph_size;
layout(location = 3) in vec4  a_uv_rect;
layout(location = 4) in vec4  a_fg_color;
layout(location = 5) in float a_is_color;

out vec2  v_uv;
out vec4  v_color;
out float v_is_color;

uniform vec2 u_viewport_size;

void main() {
    vec2 pixel_pos = a_glyph_origin + a_pos * a_glyph_size;
    vec2 ndc = (pixel_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv       = mix(a_uv_rect.xy, a_uv_rect.zw, a_pos);
    v_color    = a_fg_color;
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

/// Image pass: textured quads for inline images.
///
/// Vertex layout: `vec2 a_pos, vec2 a_uv`  (stride = 4 × f32 = 16 bytes)
const IMG_VERT_SRC: &str = r"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;

out vec2 v_uv;

uniform vec2 u_viewport_size;

void main() {
    vec2 ndc = (a_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_uv = a_uv;
}
";

const IMG_FRAG_SRC: &str = r"#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_image;

void main() {
    // Image pixels are stored as straight RGBA; output premultiplied alpha.
    vec4 c = texture(u_image, v_uv);
    frag_color = vec4(c.rgb * c.a, c.a);
}
";

// ---------------------------------------------------------------------------
//  Vertex strides (in f32 components)
// ---------------------------------------------------------------------------

/// Decoration vertex: `x, y, r, g, b, a` — 6 floats per vertex.
///
/// Used for underlines, strikethrough, cursor, and selection highlight quads.
/// These are the same layout as the old background pass.
const DECO_VERTEX_FLOATS: usize = 6;
/// Foreground instance: `glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1,
/// r, g, b, a, is_color` — 13 floats per glyph instance.
pub(crate) const FG_INSTANCE_FLOATS: usize = 13;
/// Image vertex: `x, y, u, v` — 4 floats per vertex.
const IMG_VERTEX_FLOATS: usize = 4;
/// Vertices per quad (2 triangles, 6 vertices).
pub(crate) const VERTS_PER_QUAD: usize = 6;
/// Floats for one cursor quad in the decoration VBO.
pub(crate) const CURSOR_QUAD_FLOATS: usize = VERTS_PER_QUAD * DECO_VERTEX_FLOATS;

/// Per-instance data: `col, row, r, g, b, a` — 6 floats per cell instance.
pub(crate) const BG_INSTANCE_FLOATS: usize = 6;

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
/// [`TerminalRenderer::draw`] every frame.
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
    /// Returns a human-readable error string if shader compilation/linking fails
    /// or if any GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), String> {
        self.init_bg_inst_pass(gl)?;
        self.init_deco_pass(gl)?;
        self.init_fg_pass(gl)?;
        self.init_atlas_texture(gl)?;
        self.init_image_pass(gl)?;

        self.initialized = true;
        Ok(())
    }

    /// Initialise the instanced background pass (shader, VAO, unit-quad VBO,
    /// double-buffered instance VBOs).
    fn init_bg_inst_pass(&mut self, gl: &glow::Context) -> Result<(), String> {
        let program = compile_program(gl, BG_INST_VERT_SRC, BG_INST_FRAG_SRC, "bg_instanced")?;

        self.bg_inst_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        self.bg_inst_u_cell_width = unsafe { gl.get_uniform_location(program, "u_cell_width") };
        self.bg_inst_u_cell_height = unsafe { gl.get_uniform_location(program, "u_cell_height") };
        self.bg_inst_u_bg_opacity = unsafe { gl.get_uniform_location(program, "u_bg_opacity") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create bg_inst VAO: {e}"))?
        };
        let unit_quad_vbo = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create bg unit-quad VBO: {e}"))?
        };
        let inst_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create bg instance VBO 0: {e}"))?
        };
        let inst_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create bg instance VBO 1: {e}"))?
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
    fn init_deco_pass(&mut self, gl: &glow::Context) -> Result<(), String> {
        let program = compile_program(gl, DECO_VERT_SRC, DECO_FRAG_SRC, "decoration")?;

        self.deco_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create deco VAO: {e}"))?
        };
        let vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create deco VBO 0: {e}"))?
        };
        let vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create deco VBO 1: {e}"))?
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
    fn init_fg_pass(&mut self, gl: &glow::Context) -> Result<(), String> {
        let program = compile_program(gl, FG_VERT_SRC, FG_FRAG_SRC, "foreground")?;

        self.fg_u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };
        self.fg_u_atlas = unsafe { gl.get_uniform_location(program, "u_atlas") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create foreground VAO: {e}"))?
        };
        let vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create foreground instance VBO 0: {e}"))?
        };
        let vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create foreground instance VBO 1: {e}"))?
        };

        // The unit-quad VBO must already exist (created by init_bg_inst_pass).
        let unit_quad_vbo = self.bg_unit_quad_vbo.ok_or_else(|| {
            "FG init: unit-quad VBO not yet created (call init_bg_inst_pass first)".to_owned()
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
    fn init_atlas_texture(&mut self, gl: &glow::Context) -> Result<(), String> {
        let texture = unsafe {
            gl.create_texture()
                .map_err(|e| format!("create atlas texture: {e}"))?
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
    fn init_image_pass(&mut self, gl: &glow::Context) -> Result<(), String> {
        let img_program = compile_program(gl, IMG_VERT_SRC, IMG_FRAG_SRC, "image")?;

        let img_u_viewport = unsafe { gl.get_uniform_location(img_program, "u_viewport_size") };
        let img_u_image = unsafe { gl.get_uniform_location(img_program, "u_image") };

        let img_vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| format!("create image VAO: {e}"))?
        };
        let img_vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create image VBO 0: {e}"))?
        };
        let img_vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| format!("create image VBO 1: {e}"))?
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

    /// Render a terminal frame from pre-built vertex buffers.
    ///
    /// Used when the vertex buffers were built on the main thread (where
    /// [`FontManager`] is available) before being passed into the
    /// `PaintCallback` closure, which must be `Send + Sync` and therefore
    /// cannot capture a `FontManager`.
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

        // 3. Draw instanced backgrounds, decorations, foreground, images.
        #[allow(clippy::cast_precision_loss)]
        let vp_w = viewport_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let vp_h = viewport_height as f32;

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

        // 3. Draw instanced backgrounds, decorations, foreground, images
        //    with the total float counts from the previously uploaded full
        //    frame.
        #[allow(clippy::cast_precision_loss)]
        let vp_w = viewport_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let vp_h = viewport_height as f32;

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

            #[allow(clippy::cast_possible_wrap)]
            let w = img.width_px as i32;
            #[allow(clippy::cast_possible_wrap)]
            let h = img.height_px as i32;

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
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            if quad_idx >= total_quads as i32 {
                break;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let first_vertex = quad_idx * VERTS_PER_QUAD as i32;
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(*tex));
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                gl.draw_arrays(glow::TRIANGLES, first_vertex, VERTS_PER_QUAD as i32);
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
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, instance_count as i32);
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

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let vertex_count = (vert_floats / DECO_VERTEX_FLOATS) as i32;

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
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            gl.draw_arrays_instanced(glow::TRIANGLES, 0, 6, instance_count as i32);
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let f = size_of::<f32>() as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let inst_stride = (BG_INSTANCE_FLOATS * size_of::<f32>()) as i32;

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
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let stride = (DECO_VERTEX_FLOATS * size_of::<f32>()) as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let offset_c2 = (2 * size_of::<f32>()) as i32;
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let f = size_of::<f32>() as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let inst_stride = (FG_INSTANCE_FLOATS * size_of::<f32>()) as i32;

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
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let stride = (IMG_VERTEX_FLOATS * size_of::<f32>()) as i32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let f = size_of::<f32>() as i32;
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

/// Build the two-pass background data: instanced cell BGs + decoration quads.
///
/// Returns `(bg_instances, deco_verts)`:
/// - `bg_instances`: flat `Vec<f32>` with `BG_INSTANCE_FLOATS` (6) floats per
///   cell that has a non-default background.  Each instance is
///   `(col, row, r, g, b, a)`.  Uploaded to the instance VBO and drawn with
///   `draw_arrays_instanced`.
/// - `deco_verts`: flat `Vec<f32>` with `DECO_VERTEX_FLOATS` (6) floats per
///   vertex.  Contains underline, strikethrough, selection highlight, and
///   cursor quads.  Uploaded to the decoration VBO and drawn with a plain
///   `draw_arrays` call.
///
/// The cursor quad (if visible) is always appended **last** in `deco_verts`
/// so that cursor-only partial updates can patch just the tail.
#[must_use]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn build_background_instances(
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
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
) -> (Vec<f32>, Vec<f32>) {
    let mut instances: Vec<f32> = Vec::new();
    let mut deco: Vec<f32> = Vec::new();

    for (row_idx, line) in shaped_lines.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let y_top = row_idx as f32 * cell_height as f32;

        // --- Per-cell background instances ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(&FontDecorations::Faint);
            let bg_color_raw = run.colors.get_background_color();

            // Skip default backgrounds (transparent — the terminal base color
            // is rendered as a panel clear, not explicit quads).
            if matches!(
                bg_color_raw,
                freminal_common::colors::TerminalColor::DefaultBackground
            ) {
                continue;
            }

            let [r, g, b, a] = internal_color_to_gl(bg_color_raw, is_faint, theme);

            // Emit one instance per cell in this run.
            let col_count = run_col_count(run);
            for c in 0..col_count {
                let col = run.col_start + c;
                #[allow(clippy::cast_precision_loss)]
                {
                    instances.push(col as f32);
                    instances.push(row_idx as f32);
                }
                instances.push(r);
                instances.push(g);
                instances.push(b);
                instances.push(a);
            }
        }

        // --- Underline and strikethrough decoration quads ---
        for run in &line.runs {
            let is_faint = run.font_decorations.contains(&FontDecorations::Faint);
            let has_underline = run.font_decorations.contains(&FontDecorations::Underline);
            let has_strike = run
                .font_decorations
                .contains(&FontDecorations::Strikethrough);

            if !has_underline && !has_strike {
                continue;
            }

            let fg_color = internal_color_to_gl(run.colors.get_color(), is_faint, theme);
            let col_end = run.col_start + run_col_count(run);

            #[allow(clippy::cast_precision_loss)]
            let x0 = run.col_start as f32 * cell_width as f32;
            #[allow(clippy::cast_precision_loss)]
            let x1 = col_end as f32 * cell_width as f32;

            if has_underline {
                let ul_top = y_top + underline_offset;
                let ul_bot = ul_top + stroke_size.max(1.0);
                push_quad(&mut deco, x0, ul_top, x1, ul_bot, fg_color);
            }

            if has_strike {
                let st_top = y_top + strikeout_offset;
                let st_bot = st_top + stroke_size.max(1.0);
                push_quad(&mut deco, x0, st_top, x1, st_bot, fg_color);
            }
        }
    }

    // --- Selection highlight quads (decoration pass) ---
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
            let col_begin = if row == sel_start_row {
                sel_start_col
            } else {
                0
            };
            let col_end = if row == sel_end_row {
                sel_end_col
            } else {
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

            push_quad(&mut deco, x0, y0, x1, y1, selection_bg_f(theme));
        }
    }

    // --- Cursor quad (always last in deco so cursor-only patches work) ---
    if show_cursor && cursor_blink_is_visible(cursor_visual_style, cursor_blink_on) {
        #[allow(clippy::cast_precision_loss)]
        let cx = cursor_pos.x as f32 * cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let cy = cursor_pos.y as f32 * cell_height as f32;
        #[allow(clippy::cast_precision_loss)]
        let cw = cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let ch = cell_height as f32;

        let color = cursor_f(theme, cursor_color_override);

        match cursor_visual_style {
            CursorVisualStyle::BlockCursorBlink | CursorVisualStyle::BlockCursorSteady => {
                push_quad(&mut deco, cx, cy, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::UnderlineCursorBlink | CursorVisualStyle::UnderlineCursorSteady => {
                let bar_h = (ch * 0.1).max(2.0);
                push_quad(&mut deco, cx, cy + ch - bar_h, cx + cw, cy + ch, color);
            }
            CursorVisualStyle::VerticalLineCursorBlink
            | CursorVisualStyle::VerticalLineCursorSteady => {
                let bar_w = (cw * 0.1).max(1.0);
                push_quad(&mut deco, cx, cy, cx + bar_w, cy + ch, color);
            }
        }
    }

    (instances, deco)
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
#[allow(clippy::too_many_arguments)]
pub fn build_cursor_verts_only(
    cell_width: u32,
    cell_height: u32,
    show_cursor: bool,
    cursor_blink_on: bool,
    cursor_pos: CursorPos,
    cursor_visual_style: &CursorVisualStyle,
    theme: &ThemePalette,
    cursor_color_override: Option<(u8, u8, u8)>,
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

        let color = cursor_f(theme, cursor_color_override);

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

/// Options controlling per-glyph foreground rendering.
///
/// Bundled to keep `build_foreground_instances` within the 7-argument lint limit.
pub struct FgRenderOptions {
    /// Normalised selection region `(start_col, start_row, end_col, end_row)`,
    /// or `None` when no selection is active.
    pub selection: Option<(usize, usize, usize, usize)>,
    /// Whether slow-blink (SGR 5) text is currently in its visible phase.
    pub text_blink_slow_visible: bool,
    /// Whether fast-blink (SGR 6) text is currently in its visible phase.
    pub text_blink_fast_visible: bool,
}

impl FgRenderOptions {
    /// Convenience constructor for the common case where all text is fully visible
    /// (e.g. internal helper calls and tests that do not exercise blink).
    #[must_use]
    pub const fn all_visible(selection: Option<(usize, usize, usize, usize)>) -> Self {
        Self {
            selection,
            text_blink_slow_visible: true,
            text_blink_fast_visible: true,
        }
    }
}

/// Build the foreground instance buffer from shaped lines.
///
/// For each shaped glyph: looks up the atlas entry (rasterising on miss) and
/// emits a single instance at the cell-grid position adjusted by the bearing offsets.
///
/// `opts.selection` is `Some((start_col, start_row, end_col, end_row))` in normalised
/// reading order.  Glyphs that fall within the selection use `SELECTION_FG_F`
/// instead of their normal foreground color.
///
/// Returns a flat `Vec<f32>` with `FG_INSTANCE_FLOATS` floats per glyph instance.
#[must_use]
pub fn build_foreground_instances(
    shaped_lines: &[ShapedLine],
    atlas: &mut GlyphAtlas,
    font_manager: &FontManager,
    cell_height: u32,
    ascent: f32,
    opts: &FgRenderOptions,
    theme: &ThemePalette,
) -> Vec<f32> {
    let mut instances: Vec<f32> = Vec::new();

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
            let normal_fg = internal_color_to_gl(run.colors.get_color(), is_faint, theme);

            // Track the current column as we iterate glyphs within the run.
            let mut col = run.col_start;

            // Determine whether this run's glyphs should be visible based on
            // the blink state.  `BlinkState::None` is always visible.
            let run_visible = match run.blink {
                BlinkState::None => true,
                BlinkState::Slow => opts.text_blink_slow_visible,
                BlinkState::Fast => opts.text_blink_fast_visible,
            };

            for glyph in &run.glyphs {
                let fg_color = if is_cell_selected(row_idx, col, opts.selection) {
                    selection_fg_f(theme)
                } else {
                    normal_fg
                };

                if run_visible {
                    emit_glyph_instance(
                        &mut instances,
                        glyph,
                        atlas,
                        font_manager,
                        baseline_y,
                        fg_color,
                        [cell_top, cell_bottom],
                    );
                }

                col += glyph.cell_width;
            }
        }
    }

    instances
}

/// Build the image vertex buffer from the visible placements.
///
/// Emits one textured quad per unique image ID found in `placements`.
/// Images are sorted by ID before emission so that `draw_images` (which
/// iterates `snap_images.keys()` in the same sorted order) draws the
/// correct texture for each quad.
///
/// Each quad covers the union of all cells that belong to a given image in
/// the current visible window (i.e. the full image bounding box).
///
/// `placements` is parallel to `visible_chars`: one entry per cell in
/// row-major order.  `term_width` is the number of columns per row.
/// `cell_width` and `cell_height` are integer pixel sizes.
///
/// Returns a flat `Vec<f32>` with `IMG_VERTEX_FLOATS` floats per vertex,
/// `VERTS_PER_QUAD` vertices per image quad.
/// Per-image tracking: pixel bounding box and cell-grid extent within the
/// image.  The cell-grid extent (min/max `col_in_image`, `row_in_image`) tells
/// us which portion of the texture is visible, so we can compute UV
/// coordinates that preserve aspect ratio even when the image is partially
/// clipped by the terminal edge.
struct ImageBounds {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    min_col_in_image: usize,
    min_row_in_image: usize,
    max_col_in_image: usize,
    max_row_in_image: usize,
}

#[must_use]
#[allow(clippy::too_many_arguments, clippy::implicit_hasher)]
pub fn build_image_verts(
    placements: &[Option<ImagePlacement>],
    snap_images: &std::collections::HashMap<u64, InlineImage>,
    term_width: usize,
    cell_width: u32,
    cell_height: u32,
) -> Vec<f32> {
    if placements.is_empty() || snap_images.is_empty() {
        return Vec::new();
    }

    let mut bounds: std::collections::HashMap<u64, ImageBounds> = std::collections::HashMap::new();

    for (cell_idx, placement) in placements.iter().enumerate() {
        let Some(p) = placement else { continue };
        let col = if term_width == 0 {
            0
        } else {
            cell_idx % term_width
        };
        let row = if term_width == 0 {
            0
        } else {
            cell_idx / term_width
        };

        #[allow(clippy::cast_precision_loss)]
        let x0 = col as f32 * cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let y0 = row as f32 * cell_height as f32;
        #[allow(clippy::cast_precision_loss)]
        let x1 = x0 + cell_width as f32;
        #[allow(clippy::cast_precision_loss)]
        let y1 = y0 + cell_height as f32;

        let id = p.image_id;
        let entry = bounds.entry(id).or_insert(ImageBounds {
            x0,
            y0,
            x1,
            y1,
            min_col_in_image: p.col_in_image,
            min_row_in_image: p.row_in_image,
            max_col_in_image: p.col_in_image,
            max_row_in_image: p.row_in_image,
        });
        entry.x0 = entry.x0.min(x0);
        entry.y0 = entry.y0.min(y0);
        entry.x1 = entry.x1.max(x1);
        entry.y1 = entry.y1.max(y1);
        entry.min_col_in_image = entry.min_col_in_image.min(p.col_in_image);
        entry.min_row_in_image = entry.min_row_in_image.min(p.row_in_image);
        entry.max_col_in_image = entry.max_col_in_image.max(p.col_in_image);
        entry.max_row_in_image = entry.max_row_in_image.max(p.row_in_image);
    }

    // Emit quads sorted by image ID (must match draw_images iteration order).
    let mut sorted_ids: Vec<u64> = bounds.keys().copied().collect();
    sorted_ids.sort_unstable();

    let mut verts = Vec::with_capacity(sorted_ids.len() * VERTS_PER_QUAD * IMG_VERTEX_FLOATS);

    for id in &sorted_ids {
        let Some(b) = bounds.get(id) else {
            continue;
        };

        let quad = compute_image_quad(b, snap_images.get(id));
        verts.extend_from_slice(&quad);
    }

    verts
}

/// Compute the 6-vertex textured quad for a single image.
///
/// UV coordinates are derived from the image's `display_cols` / `display_rows`
/// — the cell-grid extent the Kitty protocol declared for this image.  The
/// visible portion of the image (determined by `ImageBounds` min/max
/// `col_in_image` / `row_in_image`) maps to the corresponding fraction of
/// the texture.  The full texture (UV 0..1) covers the full `display_cols` ×
/// `display_rows` grid; partial visibility (e.g. image clipped at the terminal
/// edge) produces a UV sub-range.
///
/// The quad pixel size is determined by the bounding box `b`, which is already
/// computed in renderer cell pixels.  This means the image is scaled to fill
/// exactly the declared cell grid — matching the Kitty protocol intent.
#[allow(clippy::cast_precision_loss)]
fn compute_image_quad(b: &ImageBounds, img: Option<&InlineImage>) -> [f32; 4 * VERTS_PER_QUAD] {
    let (u0, v0, u1, v1) = if let Some(img) = img
        && img.display_cols > 0
        && img.display_rows > 0
    {
        let dc = img.display_cols as f32;
        let dr = img.display_rows as f32;

        // Map the visible cell range to the corresponding UV sub-range.
        let u0 = b.min_col_in_image as f32 / dc;
        let v0 = b.min_row_in_image as f32 / dr;
        let u1 = (b.max_col_in_image + 1) as f32 / dc;
        let v1 = (b.max_row_in_image + 1) as f32 / dr;

        (u0, v0, u1, v1)
    } else {
        // Fallback: map full texture if image metadata is unavailable.
        (0.0, 0.0, 1.0, 1.0)
    };

    [
        // Triangle 1
        b.x0, b.y0, u0, v0, b.x1, b.y0, u1, v0, b.x0, b.y1, u0, v1, // Triangle 2
        b.x1, b.y0, u1, v0, b.x1, b.y1, u1, v1, b.x0, b.y1, u0, v1,
    ]
}

/// Emit a single foreground glyph instance (13 floats).
///
/// Looks up (or rasterises) the atlas entry for the glyph, then pushes one
/// instance into `instances`.  Glyphs that extend beyond the cell's vertical
/// extent (`cell_y_range[0]`..`cell_y_range[1]`) are clipped, and their UV
/// and position are adjusted proportionally so the visible portion of the
/// atlas texture is correct.
fn emit_glyph_instance(
    instances: &mut Vec<f32>,
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

    // One instance: glyph_x, glyph_y, glyph_w, glyph_h, u0, v0, u1, v1, r, g, b, a, is_color
    instances.extend_from_slice(&[
        x0,
        y0,
        x1 - x0,
        y1 - y0,
        u0,
        v0_adj,
        u1,
        v1_adj,
        fg_color[0],
        fg_color[1],
        fg_color[2],
        fg_color[3],
        is_color_f,
    ]);
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

// ---------------------------------------------------------------------------
//  Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::config::Config;
    use freminal_common::themes;

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
                blink: BlinkState::None,
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
    //  Background instance + decoration tests
    // -----------------------------------------------------------------------

    /// Shorthand for calling `build_background_instances` with typical test
    /// defaults (no selection, `CATPPUCCIN_MOCHA`, no cursor color override).
    fn bg_instances_test(
        lines: &[ShapedLine],
        cell_width: u32,
        cell_height: u32,
        show_cursor: bool,
        cursor_blink_on: bool,
        cursor_pos: CursorPos,
        cursor_style: &CursorVisualStyle,
    ) -> (Vec<f32>, Vec<f32>) {
        build_background_instances(
            lines,
            cell_width,
            cell_height,
            13.0,
            8.0,
            1.0,
            show_cursor,
            cursor_blink_on,
            cursor_pos,
            cursor_style,
            None,
            &themes::CATPPUCCIN_MOCHA,
            None,
        )
    }

    #[test]
    fn bg_instances_empty_on_default_background() {
        // A line whose cells all have `DefaultBackground` should produce no
        // instances and no decoration verts.
        let line = make_line(5, 8.0, default_colors(), vec![]);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            0,
            "default background should produce no instances"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_per_cell_for_colored_run() {
        // A single run with 3 non-default-background cells should produce 3
        // instances (one per cell).
        let colors = StateColors::default().with_background_color(TerminalColor::Red);
        let line = make_line(3, 8.0, colors, vec![]);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            3 * BG_INSTANCE_FLOATS,
            "expected 3 cell instances"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_adjacent_same_color_per_cell() {
        // Two adjacent runs with the same background color produce one instance
        // per cell (instanced rendering does not merge — the GPU handles it).
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
                    blink: BlinkState::None,
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
                    blink: BlinkState::None,
                },
            ],
        };

        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        // Two cells → two instances.
        assert_eq!(
            bg.len(),
            2 * BG_INSTANCE_FLOATS,
            "adjacent same-color runs should produce one instance per cell"
        );
        assert_eq!(deco.len(), 0, "no decorations expected");
    }

    #[test]
    fn bg_instances_different_colors_per_cell() {
        // Two runs with different non-default background colors produce one
        // instance per cell.
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
                    blink: BlinkState::None,
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
                    blink: BlinkState::None,
                },
            ],
        };

        let (bg, _deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            bg.len(),
            2 * BG_INSTANCE_FLOATS,
            "different-color runs should produce one instance per cell"
        );
    }

    #[test]
    fn bg_instances_cursor_block_adds_deco_quad() {
        // With `show_cursor = true` and a steady block cursor, one cursor quad
        // should appear in the decoration verts.
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            true,
            CursorPos { x: 1, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg should produce no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "block cursor should add one deco quad"
        );
    }

    #[test]
    fn bg_instances_cursor_blink_off_no_quad() {
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            false, // blink_on = false
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorBlink,
        );
        assert_eq!(bg.len(), 0);
        assert_eq!(
            deco.len(),
            0,
            "blinking cursor with blink_on=false should produce no quad"
        );
    }

    #[test]
    fn bg_instances_cursor_steady_ignores_blink_flag() {
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let (_bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            true,
            false, // blink_on = false — irrelevant for steady cursor
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "steady cursor should render even when blink_on=false"
        );
    }

    #[test]
    fn bg_instances_underline_adds_deco_quad() {
        let line = make_line(3, 8.0, default_colors(), vec![FontDecorations::Underline]);
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg — no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "underline run should produce one decoration quad"
        );
    }

    #[test]
    fn bg_instances_strikethrough_adds_deco_quad() {
        let line = make_line(
            3,
            8.0,
            default_colors(),
            vec![FontDecorations::Strikethrough],
        );
        let (bg, deco) = bg_instances_test(
            &[line],
            8,
            16,
            false,
            false,
            CursorPos::default(),
            &CursorVisualStyle::BlockCursorSteady,
        );
        assert_eq!(bg.len(), 0, "default bg — no instances");
        assert_eq!(
            deco.len(),
            VERTS_PER_QUAD * DECO_VERTEX_FLOATS,
            "strikethrough run should produce one decoration quad"
        );
    }

    #[test]
    fn bg_instances_cursor_position_maps_to_pixel_coords() {
        // Block cursor at (col=2, row=1) with cell_width=10, cell_height=20.
        // Expected x0 = 2*10 = 20, y0 = 1*20 = 20.
        let lines = [
            make_line(5, 10.0, default_colors(), vec![]),
            make_line(5, 10.0, default_colors(), vec![]),
        ];
        let (_bg, deco) = bg_instances_test(
            &lines,
            10,
            20,
            true,
            true,
            CursorPos { x: 2, y: 1 },
            &CursorVisualStyle::BlockCursorSteady,
        );
        // The cursor quad is the last 36 floats (6 verts x 6 floats) in deco.
        assert!(deco.len() >= VERTS_PER_QUAD * DECO_VERTEX_FLOATS);
        let cursor_start = deco.len() - VERTS_PER_QUAD * DECO_VERTEX_FLOATS;
        let x0 = deco[cursor_start];
        let y0 = deco[cursor_start + 1];
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
    //  Foreground instance tests
    // -----------------------------------------------------------------------

    #[test]
    fn fg_instances_empty_on_empty_lines() {
        let instances = build_foreground_instances(
            &[],
            &mut GlyphAtlas::default(),
            &FontManager::new(&Config::default(), 1.0),
            16,
            13.0,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
        );
        assert_eq!(instances.len(), 0);
    }

    #[test]
    fn fg_instances_produces_data_for_ascii_glyphs() {
        let mut fm = FontManager::new(&Config::default(), 1.0);
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
        let lines = cache.shape_visible(&chars, &tags, 80, &mut fm, cell_w, false);

        let instances = build_foreground_instances(
            &lines,
            &mut atlas,
            &fm,
            cell_h,
            ascent,
            &FgRenderOptions::all_visible(None),
            &themes::CATPPUCCIN_MOCHA,
        );

        // Three ASCII glyphs each produce one instance = FG_INSTANCE_FLOATS floats.
        // Some glyphs may be spaces (zero-size) — so at minimum some instances must exist.
        assert!(
            instances.len() >= FG_INSTANCE_FLOATS,
            "at least one foreground instance expected, got {} floats",
            instances.len()
        );
        assert_eq!(
            instances.len() % FG_INSTANCE_FLOATS,
            0,
            "foreground instance count must be a multiple of one instance ({FG_INSTANCE_FLOATS} floats)",
        );
    }

    // -----------------------------------------------------------------------
    //  Push quad helper
    // -----------------------------------------------------------------------

    #[test]
    fn push_quad_produces_six_vertices() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 0.0, 0.0, 10.0, 10.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(verts.len(), VERTS_PER_QUAD * DECO_VERTEX_FLOATS);
    }

    #[test]
    fn push_quad_corner_positions() {
        let mut verts = Vec::new();
        push_quad(&mut verts, 5.0, 3.0, 15.0, 13.0, [0.0, 1.0, 0.0, 1.0]);

        // Vertex 0 (top-left): x=5, y=3
        assert!((verts[0] - 5.0).abs() < f32::EPSILON);
        assert!((verts[1] - 3.0).abs() < f32::EPSILON);

        // Vertex 1 (top-right): x=15, y=3
        assert!((verts[DECO_VERTEX_FLOATS] - 15.0).abs() < f32::EPSILON);
        assert!((verts[DECO_VERTEX_FLOATS + 1] - 3.0).abs() < f32::EPSILON);
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
            &themes::CATPPUCCIN_MOCHA,
            None,
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
                &themes::CATPPUCCIN_MOCHA,
                None,
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
            &themes::CATPPUCCIN_MOCHA,
            None,
        );
        assert!(verts.is_empty(), "blink-off cursor should produce no verts");
    }

    /// Partial VBO update: `build_cursor_verts_only` produces exactly
    /// `CURSOR_QUAD_FLOATS` floats and they occupy the correct byte range.
    ///
    /// The test verifies that:
    ///   1. The cursor-only builder produces `CURSOR_QUAD_FLOATS` floats
    ///      (= `VERTS_PER_QUAD * DECO_VERTEX_FLOATS`).
    ///   2. Patching those floats into a pre-built deco VBO at the
    ///      recorded offset produces the expected combined buffer — only the
    ///      cursor region changes, all other floats are untouched.
    #[test]
    fn partial_vbo_update_only_modifies_cursor_region() {
        // Build the full deco VBO for one line + a cursor at (col=0, row=0).
        let line = make_line(3, 8.0, default_colors(), vec![]);
        let (_bg, full_deco) = bg_instances_test(
            std::slice::from_ref(&line),
            8,
            16,
            true,
            true, // cursor visible
            CursorPos { x: 0, y: 0 },
            &CursorVisualStyle::BlockCursorSteady,
        );

        // Record where the cursor quad starts (it is appended at the end).
        let cursor_float_offset = full_deco.len() - CURSOR_QUAD_FLOATS;
        let cursor_byte_offset = cursor_float_offset * std::mem::size_of::<f32>();

        // The pre-cursor portion must be unchanged — capture it before mutation.
        let pre_cursor = full_deco[..cursor_float_offset].to_vec();

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
            &themes::CATPPUCCIN_MOCHA,
            None,
        );

        // Simulate the partial-update patch: mutate full_deco in-place to
        // overwrite the cursor region (matches draw_with_cursor_only_update).
        let mut patched = full_deco;
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
