#version 450

layout(set = 0, binding = 1) uniform sampler2D uTexture;

layout(push_constant) uniform PushConstants {
    layout(offset = 64) vec4 colorMultiplier;
} push;

layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 outColor;

void main() {
    vec4 texColor = texture(uTexture, vTexCoord);
    outColor = texColor * push.colorMultiplier;
}
