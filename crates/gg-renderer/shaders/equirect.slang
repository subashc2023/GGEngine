// What sky.slang and prefilter.slang share:
// - where a world way falls in an equirectangular image, mapped as Khronos's glTF Sample Renderer
//   and three.js do: center column toward +X, a quarter across toward -Z, top row straight up;
// - a cube face's texel as its way, by Vulkan's table of cube map faces.

#include "common.slang"

// `Sampler::LinearRepeat`'s index in the bindless set.
static const uint LINEAR_REPEAT = 2;

// The color `image`, `size` texels, holds the way `direction`, a unit vector, from its first
// level. Filtered across its left and right edges, which meet, not its top and bottom, which don't.
float3 equirect(Texture2D image, SamplerState repeat, float2 size, float3 direction) {
    float u = 0.5 + atan2(direction.z, direction.x) / (2.0 * PI);
    float v = acos(clamp(direction.y, -1.0, 1.0)) / PI;
    float edge = 0.5 / size.y;
    return image.SampleLevel(repeat, float2(u, clamp(v, edge, 1.0 - edge)), 0.0).rgb;
}

// The way point `texel` of cube face `face`, `size` texels a side, stands for, as Vulkan picks a
// face and point by a way. `texel` is from the face's top left, centers half a texel in. Faces: +X,
// -X, +Y, -Y, +Z, -Z.
float3 cube_direction(uint face, float2 texel, float size) {
    float2 st = texel / size * 2.0 - 1.0;
    float3 direction;
    switch (face) {
    case 0: direction = float3(1.0, -st.y, -st.x); break;
    case 1: direction = float3(-1.0, -st.y, st.x); break;
    case 2: direction = float3(st.x, 1.0, st.y); break;
    case 3: direction = float3(st.x, -1.0, -st.y); break;
    case 4: direction = float3(st.x, -st.y, 1.0); break;
    default: direction = float3(-st.x, -st.y, -1.0); break;
    }
    return normalize(direction);
}
