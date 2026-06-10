#version 330 core
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
