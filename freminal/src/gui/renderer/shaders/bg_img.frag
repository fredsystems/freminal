#version 330 core
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
