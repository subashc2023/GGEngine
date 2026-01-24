#version 450
#extension GL_EXT_nonuniform_qualifier : require

// Camera UBO (Set 0, Binding 0) - same as batched renderer
layout(set = 0, binding = 0) uniform CameraUBO {
    mat4 view;
    mat4 projection;
    mat4 viewProjection;
} camera;

// =============================================================================
// Binding 0: Static quad vertices (per-vertex, shared across all instances)
// =============================================================================
layout(location = 0) in vec2 aLocalPosition;  // Unit quad corners: (-0.5,-0.5) to (0.5,0.5)
layout(location = 1) in vec2 aBaseUV;         // Base UVs: (0,0) to (1,1)

// =============================================================================
// Binding 1: Instance data (per-instance)
// =============================================================================
layout(location = 2) in vec3 aPosition;       // World position (x, y, z)
layout(location = 3) in float aRotation;      // Rotation in radians (Z-axis)
layout(location = 4) in vec2 aScale;          // Size (width, height)
// location 5 is padding
layout(location = 6) in vec4 aColor;          // RGBA tint
layout(location = 7) in vec4 aTexCoords;      // UV bounds: minU, minV, maxU, maxV
layout(location = 8) in uint aTexIndex;       // Bindless texture index
layout(location = 9) in float aTilingFactor;  // Texture tiling multiplier

// =============================================================================
// Outputs to fragment shader
// =============================================================================
layout(location = 0) out vec2 vTexCoord;
layout(location = 1) out vec4 vColor;
layout(location = 2) out float vTilingFactor;
layout(location = 3) flat out uint vTexIndex;

void main() {
    // GPU-side TRS transform reconstruction
    float cosR = cos(aRotation);
    float sinR = sin(aRotation);

    // Apply scale to local position
    vec2 scaled = aLocalPosition * aScale;

    // Apply 2D rotation (around Z-axis)
    vec2 rotated = vec2(
        scaled.x * cosR - scaled.y * sinR,
        scaled.x * sinR + scaled.y * cosR
    );

    // Translate to world position
    vec3 worldPos = vec3(rotated + aPosition.xy, aPosition.z);

    // Apply camera transform
    gl_Position = camera.viewProjection * vec4(worldPos, 1.0);

    // Interpolate UV coordinates between min and max bounds based on base UV
    vTexCoord = mix(aTexCoords.xy, aTexCoords.zw, aBaseUV);

    // Pass through to fragment shader
    vColor = aColor;
    vTilingFactor = aTilingFactor;
    vTexIndex = aTexIndex;
}
