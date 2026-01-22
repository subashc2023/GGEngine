#version 450

layout(set = 0, binding = 1) uniform sampler2D uTextures[32];  // Array of 32 texture samplers

layout(location = 0) in vec2 vTexCoord;
layout(location = 1) in vec4 vColor;
layout(location = 2) in float vTilingFactor;
layout(location = 3) in float vTexIndex;

layout(location = 0) out vec4 outColor;

void main() {
    int index = int(vTexIndex);
    vec4 texColor = texture(uTextures[index], vTexCoord * vTilingFactor);
    outColor = texColor * vColor;
}
