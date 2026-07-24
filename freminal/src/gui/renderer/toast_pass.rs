// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Self-contained OpenGL pass that draws toast "pill" backgrounds (issue #433).
//!
//! [`ToastRenderer`] owns its own shader program, VAO, and double-buffered
//! VBOs — modeled directly on the image pass in [`super::gpu`] (see
//! `init_image_pass` / `setup_img_attribs` / `upload_img_verts` /
//! `draw_images`). It is driven by the toast overlay's `PaintCallback` in
//! [`crate::gui::toast`] (see `ToastStack::show` / `paint_toasts`), which
//! calls [`ToastRenderer::draw`] on the GL thread after building the pill
//! quads on the main thread.
//!
//! Each toast is drawn as a single quad (2 triangles = 6 vertices), expanded
//! beyond the crisp pill rect by [`GLOW_MARGIN_PX`] on every side so the
//! fragment shader has room to render the outer glow and drop shadow. See
//! `toast.frag` for the SDF-based layering (shadow -> glow -> gradient fill
//! + accent bar).
//!
//! The CPU-side vertex builder ([`build_toast_verts`]) is a pure function,
//! fully testable without a GL context; GL calls are confined to
//! [`ToastRenderer::init`], [`ToastRenderer::draw`], and
//! [`ToastRenderer::destroy`].

use glow::{self, HasContext};
use tracing::error;

use super::errors::{BufferAllocError, GpuInitError};
use super::gpu::{compile_program, gl_f32_i32, gl_i32, upload_verts};
use super::shaders::{TOAST_FRAG_SRC, TOAST_VERT_SRC};
use super::vertex::VERTS_PER_QUAD;

/// Floats per vertex, summing `a_pos` (2), `a_pill_center` (2),
/// `a_pill_halfsize` (2), `a_corner` (1), `a_color_top` (4),
/// `a_color_bottom` (4), `a_glow` (4), `a_accent` (4), and `a_opacity` (1),
/// for a total of 24. See `toast.vert` for the exact attribute-location
/// mapping.
pub(super) const TOAST_VERTEX_FLOATS: usize = 24;

/// Margin, in physical pixels, by which each toast's drawn quad is expanded
/// beyond its crisp pill rect on every side. Gives the fragment shader room
/// to render the outer glow (`GLOW_RADIUS` in `toast.frag`) and the
/// downward-offset drop shadow (`SHADOW_OFFSET` + `SHADOW_BLUR`) without
/// clipping. Must stay >= the largest of those GLSL constants' reach.
const GLOW_MARGIN_PX: f32 = 24.0;

// ---------------------------------------------------------------------------
//  Public CPU-side data
// ---------------------------------------------------------------------------

/// One toast pill to draw this frame, in physical framebuffer pixels.
#[derive(Debug, Clone, Copy)]
pub struct ToastQuad {
    /// Pill rect (the crisp rounded rectangle), physical pixels, top-left origin.
    pub x: f32,
    /// Pill rect top, physical pixels, top-left origin.
    pub y: f32,
    /// Pill rect width, physical pixels.
    pub width: f32,
    /// Pill rect height, physical pixels.
    pub height: f32,
    /// Corner radius in physical pixels.
    pub corner_radius: f32,
    /// Top gradient color, straight (non-premultiplied) RGBA, 0..=1.
    pub color_top: [f32; 4],
    /// Bottom gradient color, straight RGBA, 0..=1.
    pub color_bottom: [f32; 4],
    /// Glow color (straight RGB) + glow intensity in .a (0..=1).
    pub glow: [f32; 4],
    /// Left accent-bar color (straight RGBA). Set alpha 0 to disable.
    pub accent: [f32; 4],
    /// Overall opacity multiplier for fade-in/out animation (0..=1).
    pub opacity: f32,
}

// ---------------------------------------------------------------------------
//  ToastRenderer
// ---------------------------------------------------------------------------

/// Holds all GPU resources for the toast-pill pass.
///
/// Call [`ToastRenderer::init`] once (inside a `PaintCallback`) to create the
/// shader program, VAO, and double-buffered VBOs. Then call
/// [`ToastRenderer::draw`] once per frame with the current toast list.
pub struct ToastRenderer {
    /// Whether GPU resources have been created.
    initialized: bool,
    /// Compiled + linked toast shader program.
    program: Option<glow::Program>,
    /// VAO configured with the 9-attribute toast vertex layout.
    vao: Option<glow::VertexArray>,
    /// Double-buffered vertex VBOs (orphan-then-write upload pattern).
    vbo: [Option<glow::Buffer>; 2],
    /// Which of the two `vbo` slots to write into next.
    vbo_index: usize,
    /// `u_viewport_size` uniform location.
    u_viewport: Option<glow::UniformLocation>,
}

impl Default for ToastRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastRenderer {
    /// Create a new, uninitialized renderer.
    ///
    /// GPU resources are created lazily on the first call to [`Self::init`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            initialized: false,
            program: None,
            vao: None,
            vbo: [None, None],
            vbo_index: 0,
            u_viewport: None,
        }
    }

    /// Return whether GPU resources have been created.
    #[must_use]
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Create all GPU resources for the toast pass.
    ///
    /// Must be called exactly once, from within a `glow` context (e.g. inside
    /// a `PaintCallback`).
    ///
    /// # Errors
    ///
    /// Returns [`GpuInitError`] if shader compilation/linking fails or if any
    /// GL object creation fails.
    pub fn init(&mut self, gl: &glow::Context) -> Result<(), GpuInitError> {
        let program = compile_program(gl, TOAST_VERT_SRC, TOAST_FRAG_SRC, "toast")?;

        let u_viewport = unsafe { gl.get_uniform_location(program, "u_viewport_size") };

        let vao = unsafe {
            gl.create_vertex_array()
                .map_err(|e| BufferAllocError::new("toast VAO", e))?
        };
        let vbo0 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("toast VBO 0", e))?
        };
        let vbo1 = unsafe {
            gl.create_buffer()
                .map_err(|e| BufferAllocError::new("toast VBO 1", e))?
        };

        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo0));
            setup_toast_attribs(gl);
            gl.bind_vertex_array(None);
        }

        self.program = Some(program);
        self.vao = Some(vao);
        self.vbo = [Some(vbo0), Some(vbo1)];
        self.u_viewport = u_viewport;
        self.initialized = true;

        Ok(())
    }

    /// Draw every toast in `quads` this frame.
    ///
    /// Builds the vertex buffer on the CPU via [`build_toast_verts`], uploads
    /// it into the next double-buffer slot (orphan-then-write), and issues a
    /// single `glDrawArrays(TRIANGLES, ...)` call covering all quads.
    ///
    /// No-op (with a logged error) if [`Self::init`] has not been called yet.
    /// No-op (silently) if `quads` is empty.
    pub fn draw(
        &mut self,
        gl: &glow::Context,
        quads: &[ToastQuad],
        viewport_w: i32,
        viewport_h: i32,
    ) {
        if !self.initialized {
            error!("ToastRenderer::draw() called before init()");
            return;
        }
        if quads.is_empty() {
            return;
        }

        let (Some(prog), Some(vao)) = (self.program, self.vao) else {
            return;
        };
        let buf_idx = self.vbo_index;
        let Some(vbo) = self.vbo[buf_idx] else {
            return;
        };

        let verts = build_toast_verts(quads);
        upload_verts(gl, vbo, &verts);

        let vp_w = gl_f32_i32(viewport_w);
        let vp_h = gl_f32_i32(viewport_h);
        let vertex_count = gl_i32(quads.len() * VERTS_PER_QUAD);

        unsafe {
            gl.use_program(Some(prog));
            if let Some(loc) = &self.u_viewport {
                gl.uniform_2_f32(Some(loc), vp_w, vp_h);
            }
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            setup_toast_attribs(gl);
            gl.draw_arrays(glow::TRIANGLES, 0, vertex_count);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }

        self.vbo_index = 1 - self.vbo_index;
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
            for slot in &mut self.vbo {
                if let Some(b) = slot.take() {
                    gl.delete_buffer(b);
                }
            }
        }

        self.initialized = false;
    }
}

/// Configure vertex attributes for the toast shader.
///
/// Layout (see `toast.vert` / `TOAST_VERTEX_FLOATS`):
/// `location 0 = vec2 a_pos, location 1 = vec2 a_pill_center,
/// location 2 = vec2 a_pill_halfsize, location 3 = float a_corner,
/// location 4 = vec4 a_color_top, location 5 = vec4 a_color_bottom,
/// location 6 = vec4 a_glow, location 7 = vec4 a_accent,
/// location 8 = float a_opacity`.
/// Stride = `TOAST_VERTEX_FLOATS * size_of::<f32>()` = 96 bytes.
unsafe fn setup_toast_attribs(gl: &glow::Context) {
    let stride = gl_i32(TOAST_VERTEX_FLOATS * size_of::<f32>());
    let f = gl_i32(size_of::<f32>());

    // Byte offsets of each attribute within one vertex, in units of `f`.
    let off_pos = 0;
    let off_center = 2 * f;
    let off_halfsize = 4 * f;
    let off_corner = 6 * f;
    let off_color_top = 7 * f;
    let off_color_bottom = 11 * f;
    let off_glow = 15 * f;
    let off_accent = 19 * f;
    let off_opacity = 23 * f;

    unsafe {
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, stride, off_pos);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, stride, off_center);
        gl.enable_vertex_attrib_array(2);
        gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, off_halfsize);
        gl.enable_vertex_attrib_array(3);
        gl.vertex_attrib_pointer_f32(3, 1, glow::FLOAT, false, stride, off_corner);
        gl.enable_vertex_attrib_array(4);
        gl.vertex_attrib_pointer_f32(4, 4, glow::FLOAT, false, stride, off_color_top);
        gl.enable_vertex_attrib_array(5);
        gl.vertex_attrib_pointer_f32(5, 4, glow::FLOAT, false, stride, off_color_bottom);
        gl.enable_vertex_attrib_array(6);
        gl.vertex_attrib_pointer_f32(6, 4, glow::FLOAT, false, stride, off_glow);
        gl.enable_vertex_attrib_array(7);
        gl.vertex_attrib_pointer_f32(7, 4, glow::FLOAT, false, stride, off_accent);
        gl.enable_vertex_attrib_array(8);
        gl.vertex_attrib_pointer_f32(8, 1, glow::FLOAT, false, stride, off_opacity);
    }
}

// ---------------------------------------------------------------------------
//  Pure CPU vertex builder
// ---------------------------------------------------------------------------

/// Build the flat `Vec<f32>` vertex buffer for `quads`.
///
/// Pure and GL-free so it is fully unit-testable. Emits
/// `TOAST_VERTEX_FLOATS` floats per vertex, [`VERTS_PER_QUAD`] vertices per
/// quad (2 triangles), one quad per entry in `quads`.
fn build_toast_verts(quads: &[ToastQuad]) -> Vec<f32> {
    let mut out = Vec::with_capacity(quads.len() * VERTS_PER_QUAD * TOAST_VERTEX_FLOATS);
    for q in quads {
        push_toast_quad(&mut out, q);
    }
    out
}

/// Append one expanded quad (6 vertices) for `q` to `out`.
fn push_toast_quad(out: &mut Vec<f32>, q: &ToastQuad) {
    // Expand the drawn quad beyond the crisp pill rect so the fragment
    // shader has room to render the outer glow and drop shadow.
    let x0 = q.x - GLOW_MARGIN_PX;
    let y0 = q.y - GLOW_MARGIN_PX;
    let x1 = q.x + q.width + GLOW_MARGIN_PX;
    let y1 = q.y + q.height + GLOW_MARGIN_PX;

    let center = [q.x + q.width / 2.0, q.y + q.height / 2.0];
    let halfsize = [q.width / 2.0, q.height / 2.0];

    // Two triangles covering the expanded quad.
    let positions: [[f32; 2]; VERTS_PER_QUAD] =
        [[x0, y0], [x1, y0], [x0, y1], [x1, y0], [x1, y1], [x0, y1]];

    for pos in positions {
        out.extend_from_slice(&pos);
        out.extend_from_slice(&center);
        out.extend_from_slice(&halfsize);
        out.push(q.corner_radius);
        out.extend_from_slice(&q.color_top);
        out.extend_from_slice(&q.color_bottom);
        out.extend_from_slice(&q.glow);
        out.extend_from_slice(&q.accent);
        out.push(q.opacity);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        GLOW_MARGIN_PX, TOAST_VERTEX_FLOATS, ToastQuad, VERTS_PER_QUAD, build_toast_verts,
    };

    fn sample_quad() -> ToastQuad {
        ToastQuad {
            x: 100.0,
            y: 50.0,
            width: 200.0,
            height: 60.0,
            corner_radius: 10.0,
            color_top: [0.1, 0.2, 0.3, 0.9],
            color_bottom: [0.4, 0.5, 0.6, 0.95],
            glow: [0.9, 0.1, 0.1, 0.5],
            accent: [0.0, 1.0, 0.0, 1.0],
            opacity: 0.75,
        }
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert!(build_toast_verts(&[]).is_empty());
    }

    #[test]
    fn n_quads_produce_expected_float_count() {
        let quads = vec![sample_quad(), sample_quad(), sample_quad()];
        let verts = build_toast_verts(&quads);
        assert_eq!(
            verts.len(),
            quads.len() * VERTS_PER_QUAD * TOAST_VERTEX_FLOATS
        );
    }

    #[test]
    fn single_quad_produces_one_quads_worth_of_floats() {
        let verts = build_toast_verts(&[sample_quad()]);
        assert_eq!(verts.len(), VERTS_PER_QUAD * TOAST_VERTEX_FLOATS);
    }

    /// Extract vertex `idx` (`0..VERTS_PER_QUAD`) of the first (and only)
    /// quad in `verts` as a `TOAST_VERTEX_FLOATS`-length slice.
    fn vertex_slice(verts: &[f32], idx: usize) -> &[f32] {
        let start = idx * TOAST_VERTEX_FLOATS;
        &verts[start..start + TOAST_VERTEX_FLOATS]
    }

    #[test]
    fn expanded_quad_extends_beyond_pill_rect_by_glow_margin() {
        let q = sample_quad();
        let verts = build_toast_verts(&[q]);

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for i in 0..VERTS_PER_QUAD {
            let v = vertex_slice(&verts, i);
            let (x, y) = (v[0], v[1]);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }

        assert!((min_x - (q.x - GLOW_MARGIN_PX)).abs() < f32::EPSILON);
        assert!((min_y - (q.y - GLOW_MARGIN_PX)).abs() < f32::EPSILON);
        assert!((max_x - (q.x + q.width + GLOW_MARGIN_PX)).abs() < f32::EPSILON);
        assert!((max_y - (q.y + q.height + GLOW_MARGIN_PX)).abs() < f32::EPSILON);
    }

    #[test]
    fn per_vertex_pill_params_replicated_across_all_six_vertices() {
        let q = sample_quad();
        let verts = build_toast_verts(&[q]);

        let expected_center = [q.x + q.width / 2.0, q.y + q.height / 2.0];
        let expected_halfsize = [q.width / 2.0, q.height / 2.0];

        for i in 0..VERTS_PER_QUAD {
            let v = vertex_slice(&verts, i);
            // center: floats [2..4)
            assert_eq!(&v[2..4], &expected_center);
            // halfsize: floats [4..6)
            assert_eq!(&v[4..6], &expected_halfsize);
            // corner: float [6]
            assert!((v[6] - q.corner_radius).abs() < f32::EPSILON);
            // color_top: floats [7..11)
            assert_eq!(&v[7..11], &q.color_top);
            // color_bottom: floats [11..15)
            assert_eq!(&v[11..15], &q.color_bottom);
            // glow: floats [15..19)
            assert_eq!(&v[15..19], &q.glow);
            // accent: floats [19..23)
            assert_eq!(&v[19..23], &q.accent);
            // opacity: float [23]
            assert!((v[23] - q.opacity).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn two_quads_do_not_share_vertex_data() {
        let mut q2 = sample_quad();
        q2.x = 500.0;
        q2.opacity = 0.2;
        let verts = build_toast_verts(&[sample_quad(), q2]);

        assert_eq!(verts.len(), 2 * VERTS_PER_QUAD * TOAST_VERTEX_FLOATS);

        let second_quad_start = VERTS_PER_QUAD * TOAST_VERTEX_FLOATS;
        let second_quad_verts = &verts[second_quad_start..];
        let v0 = &second_quad_verts[0..TOAST_VERTEX_FLOATS];
        // x position of the second quad's first vertex should reflect q2.x,
        // not the first quad's x.
        assert!((v0[0] - (q2.x - GLOW_MARGIN_PX)).abs() < f32::EPSILON);
        assert!((v0[23] - q2.opacity).abs() < f32::EPSILON);
    }
}
