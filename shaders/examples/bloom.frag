// bloom.frag — simple glow / bloom effect: bright regions bleed light into
//              their neighbours using a 9-tap Gaussian blur on the bright pass.
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
const float BLOOM_THRESHOLD = 0.6;  // luminance above this contributes to bloom
const float BLOOM_STRENGTH  = 0.4;  // how much bloom light is added back
const float BLUR_RADIUS     = 2.0;  // Gaussian kernel half-width in pixels

// 3×3 Gaussian weights (unnormalized; we normalise in the loop).
const float WEIGHTS[9] = float[](
    1.0, 2.0, 1.0,
    2.0, 4.0, 2.0,
    1.0, 2.0, 1.0
);

void main() {
    vec2 texel = 1.0 / u_resolution;
    vec4 base  = texture(u_terminal, v_uv);

    // Bright-pass blur: accumulate only pixels above the luminance threshold.
    vec3  bloom_accum = vec3(0.0);
    float weight_sum  = 0.0;
    int   idx         = 0;
    for (int dy = -1; dy <= 1; dy++) {
        for (int dx = -1; dx <= 1; dx++) {
            vec2 offset = vec2(float(dx), float(dy)) * texel * BLUR_RADIUS;
            vec3 sample_color = texture(u_terminal, v_uv + offset).rgb;
            float luma = dot(sample_color, vec3(0.2126, 0.7152, 0.0722));
            float w    = WEIGHTS[idx] * max(luma - BLOOM_THRESHOLD, 0.0);
            bloom_accum += sample_color * w;
            weight_sum  += w;
            idx++;
        }
    }
    if (weight_sum > 0.0) {
        bloom_accum /= weight_sum;
    }

    vec3 result = base.rgb + bloom_accum * BLOOM_STRENGTH;
    frag_color = vec4(result, base.a);
}
