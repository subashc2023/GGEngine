#version 450

layout(push_constant) uniform PushConstants {
    vec4 u_Color;
} pc;

layout(location = 0) out vec4 outColor;

void main() {
    outColor = pc.u_Color;
}
