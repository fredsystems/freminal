#version 330 core
// Toast pass vertex shader (issue #433).
//
// Vertex layout — one vertex is 24 floats (see TOAST_VERTEX_FLOATS in
// toast_pass.rs); all pill parameters are constant across a pill's 6
// vertices (duplicated per-vertex, non-instanced):
//   location 0: a_pos            vec2  — expanded-quad corner, physical px
//   location 1: a_pill_center     vec2  — pill rect center, physical px
//   location 2: a_pill_halfsize   vec2  — pill half-extent (w/2, h/2), px
//   location 3: a_corner          float — corner radius, px
//   location 4: a_color_top       vec4  — straight RGBA
//   location 5: a_color_bottom    vec4  — straight RGBA
//   location 6: a_glow            vec4  — rgb + intensity in .a
//   location 7: a_accent          vec4  — straight RGBA, alpha 0 disables
//   location 8: a_opacity         float — overall fade multiplier
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec2 a_pill_center;
layout(location = 2) in vec2 a_pill_halfsize;
layout(location = 3) in float a_corner;
layout(location = 4) in vec4 a_color_top;
layout(location = 5) in vec4 a_color_bottom;
layout(location = 6) in vec4 a_glow;
layout(location = 7) in vec4 a_accent;
layout(location = 8) in float a_opacity;

// v_pos varies per-fragment (it is the SDF evaluation point) and must be
// smoothly interpolated. Every other varying is identical across a pill's
// six vertices, so `flat` is used to skip needless interpolation.
out vec2 v_pos;
flat out vec2 v_center;
flat out vec2 v_halfsize;
flat out float v_corner;
flat out vec4 v_color_top;
flat out vec4 v_color_bottom;
flat out vec4 v_glow;
flat out vec4 v_accent;
flat out float v_opacity;

uniform vec2 u_viewport_size;

void main() {
    // Convert from pixel coordinates (top-left origin) to NDC.
    vec2 ndc = (a_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);

    v_pos = a_pos;
    v_center = a_pill_center;
    v_halfsize = a_pill_halfsize;
    v_corner = a_corner;
    v_color_top = a_color_top;
    v_color_bottom = a_color_bottom;
    v_glow = a_glow;
    v_accent = a_accent;
    v_opacity = a_opacity;
}
