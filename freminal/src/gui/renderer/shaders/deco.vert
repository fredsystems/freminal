#version 330 core
layout(location = 0) in vec2 a_pos;
layout(location = 1) in vec4 a_color;

out vec4 v_color;

uniform vec2 u_viewport_size;

void main() {
    // Convert from pixel coordinates (top-left origin) to NDC.
    vec2 ndc = (a_pos / u_viewport_size) * 2.0 - 1.0;
    gl_Position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    v_color = a_color;
}
