// crt.frag — classic CRT monitor effect: scanlines, vignette, and slight
//             chromatic aberration.
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

// --- tuneable parameters ---
const float SCANLINE_STRENGTH = 0.25;   // 0.0 = no scanlines, 1.0 = full black bands
const float VIGNETTE_STRENGTH = 0.35;   // 0.0 = no vignette, 1.0 = heavy corners
const float ABERRATION_AMOUNT = 0.002;  // chromatic aberration UV offset

void main() {
    // Chromatic aberration: sample R/B channels with a tiny UV offset.
    float r = texture(u_terminal, v_uv + vec2( ABERRATION_AMOUNT, 0.0)).r;
    float g = texture(u_terminal, v_uv                                 ).g;
    float b = texture(u_terminal, v_uv + vec2(-ABERRATION_AMOUNT, 0.0)).b;
    float a = texture(u_terminal, v_uv).a;
    vec3 color = vec3(r, g, b);

    // Scanline darkening: dim even physical pixel rows.
    float line   = fract(v_uv.y * u_resolution.y * 0.5);
    float scan   = 1.0 - SCANLINE_STRENGTH * step(0.5, line);
    color *= scan;

    // Vignette: darken towards the corners using a smooth radial gradient.
    vec2  centered = v_uv * 2.0 - 1.0;
    float vig      = 1.0 - VIGNETTE_STRENGTH * dot(centered, centered);
    color *= clamp(vig, 0.0, 1.0);

    frag_color = vec4(color, a);
}
