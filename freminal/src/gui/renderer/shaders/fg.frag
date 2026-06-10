#version 330 core
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
