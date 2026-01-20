#version 450

layout(push_constant) uniform PushConstants {
    vec4 color;
} pushConstants;

layout(location = 0) in vec3 fragColor;
layout(location = 0) out vec4 outColor;

void main() {
    // Use push constant color directly (ignore vertex colors for 2D)
    outColor = pushConstants.color;
}
