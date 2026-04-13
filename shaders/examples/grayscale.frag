// grayscale.frag — convert the terminal output to grayscale.
//
// Available uniforms:
//   uniform sampler2D u_terminal  — the terminal framebuffer texture
//   uniform vec2      u_resolution — viewport size in pixels (x=width, y=height)
//   uniform float     u_time       — elapsed time in seconds
//
// UV (0,0) = bottom-left, (1,1) = top-right (OpenGL convention).
#version 330 core

in  vec2 v_uv;
out vec4 frag_color;

uniform sampler2D u_terminal;
uniform vec2      u_resolution;
uniform float     u_time;

void main() {
    vec4 color = texture(u_terminal, v_uv);
    // Luminance coefficients (ITU-R BT.709).
    float luma = dot(color.rgb, vec3(0.2126, 0.7152, 0.0722));
    frag_color = vec4(vec3(luma), color.a);
}
