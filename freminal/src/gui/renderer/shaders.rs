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
//!
//! The GLSL source for each pass lives in a sibling `shaders/` directory and is
//! embedded at compile time via `include_str!`. Vertex shaders use the `.vert`
//! extension and fragment shaders use `.frag`.

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
pub(super) const DECO_VERT_SRC: &str = include_str!("./shaders/deco.vert");

pub(super) const DECO_FRAG_SRC: &str = include_str!("./shaders/deco.frag");

/// Instanced background pass: one draw call for all cell background quads.
///
/// A static unit quad (`UNIT_QUAD`) is scaled to cell size by the vertex shader.
/// Per-instance attributes supply the grid position and resolved RGBA color.
///
/// Vertex layout (per-vertex, divisor 0): `vec2 a_pos`
/// Instance layout (per-instance, divisor 1): `vec2 a_cell_pos, vec4 a_bg_color`
pub(super) const BG_INST_VERT_SRC: &str = include_str!("./shaders/bg_inst.vert");

pub(super) const BG_INST_FRAG_SRC: &str = include_str!("./shaders/bg_inst.frag");

/// Foreground pass: instanced textured glyph quads sampled from the atlas.
///
/// Per-vertex: `vec2 a_pos` (unit quad in [0,1]², divisor 0).
/// Per-instance (divisor 1):
///   location 1: `vec2  a_glyph_origin` — pixel position of the glyph quad
///   location 2: `vec2  a_glyph_size`   — pixel size of the glyph quad
///   location 3: `vec4  a_uv_rect`      — (u0, v0, u1, v1) atlas UV
///   location 4: `vec4  a_fg_color`     — RGBA foreground color
///   location 5: `float a_is_color`     — 1.0 for color emoji, 0.0 for mono
pub(super) const FG_VERT_SRC: &str = include_str!("./shaders/fg.vert");

pub(super) const FG_FRAG_SRC: &str = include_str!("./shaders/fg.frag");

/// Image pass: textured quads for inline images.
///
/// Vertex layout: `vec2 a_pos, vec2 a_uv`  (stride = 4 × f32 = 16 bytes)
pub(super) const IMG_VERT_SRC: &str = include_str!("./shaders/img.vert");

pub(super) const IMG_FRAG_SRC: &str = include_str!("./shaders/img.frag");

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
pub(super) const BG_IMG_VERT_SRC: &str = include_str!("./shaders/bg_img.vert");

/// Background image fragment shader.
///
/// Applies `u_opacity` on top of the image's own alpha so that the host
/// terminal background shows through at the configured opacity.
pub(super) const BG_IMG_FRAG_SRC: &str = include_str!("./shaders/bg_img.frag");

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
pub(super) const POST_VERT_SRC: &str = include_str!("./shaders/post.vert");

/// Post-processing passthrough fragment shader.
///
/// Used as a fallback when no user shader is configured (or compilation fails).
/// Simply samples the terminal texture and outputs it directly.
pub(super) const POST_PASSTHROUGH_FRAG_SRC: &str = include_str!("./shaders/post_passthrough.frag");
