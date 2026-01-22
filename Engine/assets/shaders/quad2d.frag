#version 450
#extension GL_EXT_nonuniform_qualifier : require

// Bindless texture array (unbounded) in set 1
layout(set = 1, binding = 0) uniform sampler2D uTextures[];

layout(location = 0) in vec2 vTexCoord;
layout(location = 1) in vec4 vColor;
layout(location = 2) in float vTilingFactor;
layout(location = 3) flat in uint vTexIndex;

layout(location = 0) out vec4 outColor;

void main() {
    // Use nonuniformEXT for dynamically non-uniform texture indexing
    vec4 texColor = texture(uTextures[nonuniformEXT(vTexIndex)], vTexCoord * vTilingFactor);
    outColor = texColor * vColor;
}
