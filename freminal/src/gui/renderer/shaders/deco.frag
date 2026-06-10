#version 330 core
in vec4 v_color;
out vec4 frag_color;

void main() {
    // Premultiplied alpha output.
    frag_color = vec4(v_color.rgb * v_color.a, v_color.a);
}
