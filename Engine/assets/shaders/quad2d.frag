#version 450
#extension GL_EXT_nonuniform_qualifier : require

// Separate texture array and sampler (better for bindless on MoltenVK/macOS)
// Binding 0: Single shared sampler (immutable, must be before variable-count binding)
// Binding 1: Array of sampled images (variable descriptor count, must be last)
layout(set = 1, binding = 0) uniform sampler uSampler;
layout(set = 1, binding = 1) uniform texture2D uTextures[];

layout(location = 0) in vec2 vTexCoord;
layout(location = 1) in vec4 vColor;
layout(location = 2) in float vTilingFactor;
layout(location = 3) flat in uint vTexIndex;

layout(location = 0) out vec4 outColor;

void main() {
    // Combine texture and sampler at sample time using nonuniformEXT for dynamic indexing
    vec4 texColor = texture(sampler2D(uTextures[nonuniformEXT(vTexIndex)], uSampler), vTexCoord * vTilingFactor);
    outColor = texColor * vColor;
}
