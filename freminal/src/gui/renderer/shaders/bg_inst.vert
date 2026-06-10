#version 330 core

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
