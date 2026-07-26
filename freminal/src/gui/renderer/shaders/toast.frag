#version 330 core
// Toast pass fragment shader (issue #433).
//
// Draws one rounded-rect "pill" per primitive using a signed-distance-field
// (SDF), layered bottom-to-top as: drop shadow -> gradient fill (+ optional
// left accent bar) -> neutral border ring. All layers are composited with
// the standard (straight-alpha) "over" operator and the final result is
// converted to PREMULTIPLIED alpha on output, matching every other pass in
// this renderer (freminal never touches GL blend state — it inherits
// egui_glow's ONE / ONE_MINUS_SRC_ALPHA blend func).
//
// Issue #433 visual redesign: the outer glow was removed (the maintainer
// found it too strong and visually disconnecting). Separation from the
// terminal behind the pill now comes from a crisp neutral border ring
// (theme window_stroke) plus a softened drop shadow, over a solid, opaque,
// neutral fill.
in vec2 v_pos;
flat in vec2 v_center;
flat in vec2 v_halfsize;
flat in float v_corner;
flat in vec4 v_color_top;
flat in vec4 v_color_bottom;
flat in vec4 v_border_color;
flat in float v_border_width;
flat in vec4 v_accent;
flat in float v_opacity;

out vec4 frag_color;

// Tuned constants for the drop shadow and accent bar. These are
// intentionally hardcoded rather than uniforms: every toast pill in the
// overlay shares the same "chrome" look, only the colors/opacity/border
// width differ. The shadow is softer than the pre-redesign value
// (alpha 0.35 -> 0.22, blur 16 -> 12) so the pill lifts off the terminal
// subtly rather than casting a heavy halo.
const float SHADOW_BLUR = 12.0;
const vec2 SHADOW_OFFSET = vec2(0.0, 3.0);
const float SHADOW_ALPHA = 0.22;
const float ACCENT_WIDTH = 4.0;

// Signed distance to a rounded rect centered at `center` with half-extent
// `halfsize` and corner radius `corner`. Negative inside, 0 on the edge,
// positive outside.
float rounded_rect_sdf(vec2 p, vec2 center, vec2 halfsize, float corner) {
    vec2 q = abs(p - center) - (halfsize - corner);
    return length(max(q, 0.0)) - corner;
}

void main() {
    float sdf = rounded_rect_sdf(v_pos, v_center, v_halfsize, v_corner);

    // Anti-aliased fill mask: 1 well inside the pill, 0 well outside, with a
    // ~1px feather (via fwidth) straddling the SDF's zero crossing.
    float aa = fwidth(sdf);
    float fill_mask = 1.0 - smoothstep(-aa, aa, sdf);

    // Vertical gradient: t=0 at the pill's top edge -> color_top, t=1 at the
    // bottom edge -> color_bottom.
    float top_y = v_center.y - v_halfsize.y;
    float t = clamp(v_pos.y - top_y, 0.0, 2.0 * v_halfsize.y);
    t = v_halfsize.y > 0.0 ? t / (2.0 * v_halfsize.y) : 0.0;
    vec4 gradient = mix(v_color_top, v_color_bottom, t);

    // Left accent bar: a solid-color strip along the pill's left edge,
    // intersected with `fill_mask` so the corner rounding is respected.
    // Disabled entirely when a_accent.a == 0.
    float left_edge = v_center.x - v_halfsize.x;
    float accent_region = 1.0 - smoothstep(ACCENT_WIDTH - aa, ACCENT_WIDTH + aa, v_pos.x - left_edge);
    float accent_mask = clamp(accent_region, 0.0, 1.0) * fill_mask * v_accent.a;
    vec3 fill_rgb = mix(gradient.rgb, v_accent.rgb, accent_mask);
    float fill_a = gradient.a * fill_mask;

    // Neutral border ring: the band of pixels just inside the pill edge,
    // i.e. where the SDF is in [-border_width, 0]. Drawn OVER the fill (and
    // the accent bar) so it frames the whole pill, including where the
    // accent bar meets the top/bottom edges. Anti-aliased on both the outer
    // (sdf ~ 0) and inner (sdf ~ -border_width) edges. Disabled when
    // border_width == 0 or border_color.a == 0.
    float outer = 1.0 - smoothstep(-aa, aa, sdf);
    float inner = 1.0 - smoothstep(-v_border_width - aa, -v_border_width + aa, sdf);
    float border_mask = clamp(outer - inner, 0.0, 1.0) * v_border_color.a;
    float border_a = border_mask;

    // Drop shadow: a softened, downward-offset copy of the same SDF, drawn
    // as a translucent dark layer strictly behind everything else.
    float shadow_sdf = rounded_rect_sdf(v_pos - SHADOW_OFFSET, v_center, v_halfsize, v_corner);
    float shadow_mask = 1.0 - smoothstep(-SHADOW_BLUR, SHADOW_BLUR, shadow_sdf);
    float shadow_a = clamp(shadow_mask, 0.0, 1.0) * SHADOW_ALPHA;
    vec3 shadow_rgb = vec3(0.0);

    // Composite bottom-to-top with the "over" operator, accumulating
    // PREMULTIPLIED color: out_rgb = top_rgb*top_a + out_rgb*(1-top_a);
    // out_a = top_a + out_a*(1-top_a). Each layer's color arrives
    // straight-alpha; the products below premultiply it as it accumulates,
    // which is what the final premultiplied emit at the bottom relies on.
    // Order: shadow (back) -> fill+accent -> border (front).
    vec3 out_rgb = shadow_rgb;
    float out_a = shadow_a;

    out_rgb = fill_rgb * fill_a + out_rgb * (1.0 - fill_a);
    out_a = fill_a + out_a * (1.0 - fill_a);

    out_rgb = v_border_color.rgb * border_a + out_rgb * (1.0 - border_a);
    out_a = border_a + out_a * (1.0 - border_a);

    // Apply the overall fade-in/out multiplier, then emit premultiplied.
    // `out_rgb`/`out_a` are ALREADY premultiplied (the over-compositing above
    // emits premultiplied color), so the fade scales BOTH channels by the
    // same `v_opacity` — scaling `out_rgb` by the faded alpha instead would
    // double-multiply and darken every translucent pixel (shadow, AA edges)
    // and muddy the fade. See issue #433 adversarial-review finding 1.
    frag_color = vec4(out_rgb * v_opacity, out_a * v_opacity);
}
