#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_image;

void main() {
    // Image pixels are stored as straight RGBA; output premultiplied alpha.
    vec4 c = texture(u_image, v_uv);
    frag_color = vec4(c.rgb * c.a, c.a);
}
