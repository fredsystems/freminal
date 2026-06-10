#version 330 core
in vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_terminal;
uniform vec2      u_resolution;
uniform float     u_time;

void main() {
    frag_color = texture(u_terminal, v_uv);
}
