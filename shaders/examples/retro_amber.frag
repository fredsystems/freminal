// retro_amber.frag — tint the terminal with a warm amber phosphor look.
//
// Available uniforms:
//   uniform sampler2D u_terminal  — the terminal framebuffer texture
//   uniform vec2      u_resolution — viewport size in pixels (x=width, y=height)
//   uniform float     u_time       — elapsed time in seconds
#version 330 core

in  vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_terminal;
uniform vec2      u_resolution;
uniform float     u_time;

// Amber phosphor colour (normalized sRGB).
const vec3 AMBER = vec3(1.0, 0.65, 0.0);

void main() {
    vec4 src = texture(u_terminal, v_uv);

    // Convert to luminance, then tint with amber.
    float luma = dot(src.rgb, vec3(0.2126, 0.7152, 0.0722));
    vec3  tinted = luma * AMBER;

    // Slight scanline darkening: dim every other physical pixel row.
    float scanline = 0.85 + 0.15 * step(0.5, fract(v_uv.y * u_resolution.y * 0.5));

    frag_color = vec4(tinted * scanline, src.a);
}
