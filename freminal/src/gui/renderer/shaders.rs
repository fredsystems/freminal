// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GLSL shader source strings for all terminal rendering passes.
//!
//! Six render passes are defined:
//! - Decoration pass (`DECO_*`): solid-color quads for underlines, strikethrough,
//!   cursor, and selection highlights.
//! - Instanced background pass (`BG_INST_*`): one draw call for all cell backgrounds.
//! - Foreground pass (`FG_*`): instanced textured glyph quads from the glyph atlas.
//! - Image pass (`IMG_*`): textured quads for inline images.
//! - Background image pass (`BG_IMG_*`): full-viewport textured quad for wallpaper.
//! - Post-processing pass (`POST_*`): fullscreen quad applying a user GLSL fragment shader
//!   to the terminal FBO texture.

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
pub(super) const DECO_VERT_SRC: &str = r"#version 330 core
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

pub(super) const DECO_FRAG_SRC: &str = r"#version 330 core
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
pub(super) const BG_INST_VERT_SRC: &str = r"#version 330 core

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

pub(super) const BG_INST_FRAG_SRC: &str = r"#version 330 core

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
pub(super) const FG_VERT_SRC: &str = r"#version 330 core
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

pub(super) const FG_FRAG_SRC: &str = r"#version 330 core
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
pub(super) const IMG_VERT_SRC: &str = r"#version 330 core
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

pub(super) const IMG_FRAG_SRC: &str = r"#version 330 core
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
//  Background image pass
// ---------------------------------------------------------------------------
//
// A full-viewport textured quad drawn *before* cell backgrounds so that the
// terminal grid composites on top.  The fit mode (fill / fit / cover / tile)
// is resolved on the CPU into UV coordinates that are passed per-vertex.
//
// Vertex layout: `vec2 a_pos, vec2 a_uv`  (stride = 4 × f32 = 16 bytes)
// Same layout as the inline image pass so the same VAO setup function is reused.

/// Background image vertex shader — identical to the inline image pass.
pub(super) const BG_IMG_VERT_SRC: &str = r"#version 330 core
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

/// Background image fragment shader.
///
/// Applies `u_opacity` on top of the image's own alpha so that the host
/// terminal background shows through at the configured opacity.
pub(super) const BG_IMG_FRAG_SRC: &str = r"#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_bg_image;
uniform float     u_opacity;   // background_image_opacity (0.0–1.0)

void main() {
    vec4 c = texture(u_bg_image, v_uv);
    float alpha = c.a * u_opacity;
    // Premultiplied alpha output.
    frag_color = vec4(c.rgb * alpha, alpha);
}
";

// ---------------------------------------------------------------------------
//  Post-processing pass (user custom shader)
// ---------------------------------------------------------------------------
//
// When the user supplies a GLSL fragment shader via `[shader] path = …`, the
// terminal is first rendered to an offscreen FBO.  The FBO colour texture is
// then drawn through this fullscreen quad pass, which applies the user shader
// as a post-processing step.
//
// The vertex shader emits a clip-space quad covering the entire viewport.
// It outputs `v_uv` in [0,1]² for the fragment stage.

/// Post-processing vertex shader — outputs a full-screen clip-space quad.
///
/// Consumes clip-space positions and texture coordinates from vertex attributes
/// (`a_pos` and `a_uv`) for a fullscreen quad submitted by the renderer.
pub(super) const POST_VERT_SRC: &str = r"#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_uv;

out vec2 v_uv;

void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_uv;
}
";

/// Post-processing passthrough fragment shader.
///
/// Used as a fallback when no user shader is configured (or compilation fails).
/// Simply samples the terminal texture and outputs it directly.
pub(super) const POST_PASSTHROUGH_FRAG_SRC: &str = r"#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_terminal;
uniform vec2      u_resolution;
uniform float     u_time;

void main() {
    frag_color = texture(u_terminal, v_uv);
}
";
