#version 330 core

in vec4  v_bg_color;

out vec4 frag_color;

uniform float u_bg_opacity;

void main() {
    float alpha = v_bg_color.a * u_bg_opacity;
    frag_color = vec4(v_bg_color.rgb * alpha, alpha);
}
