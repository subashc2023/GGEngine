// Shows an image the frame makes in place of the view's light, a pixel a thread, as src/show.rs
// plans it, into the view's colors for the tonemap pass. Stretched over the view, each pixel the
// texel at its center, read by kind as `Shown` in gg-api says a capture reads it:
// - Colors: as they are.
// - Depth: `1 + log2(depth) / 24`, white at the near plane (reversed depth 1), a 24th darker per
//   doubling of distance, black where nothing drew (0).
// - IDs and visibility: a color per value, black for 0.
// - Motion: as normals, each axis's -1 to 1 as 0 to 1.
// - Scalars: the value.
// - No kind, which the frame doesn't make: black.
//
// `Push` matches `ShowPush` in src/show.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] Texture2D<float> floats[];
[[vk::binding(0, 0)]] Texture2D<uint2> pairs[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// Pixels across and down a workgroup: `GROUP` in src/show.rs.
static const uint GROUP = 8;

// What an image holds, as `kind` in src/show.rs numbers gg-gpu's `ImageKind`.
static const uint NONE = 0;
static const uint COLOR = 1;
static const uint HDR = 2;
static const uint IDS = 3;
static const uint MOTION = 4;
static const uint VISIBILITY = 5;
static const uint DEPTH = 6;
static const uint SCALAR = 7;

struct Push {
    // The view's size, the image (a bindless texture) and the target (a bindless storage image).
    uint2 size;
    uint source;
    uint kind;
    uint target;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// `value`, each channel 0 to 1, encoded so the view's sRGB image stores it as byte / 255, as
// shading.slang's `stored_as` says.
float3 stored_as(float3 value) {
    float3 v = saturate(value);
    return select(v <= 0.04045, v / 12.92, pow((v + 0.055) / 1.055, 2.4));
}

// A color per value from its PCG3D hash (Jarzynski and Olano, "Hash Functions for GPU Rendering",
// 2020), bright enough to tell apart; black for 0.
float3 hashed(uint2 value) {
    if (value.x == 0 && value.y == 0) {
        return float3(0.0);
    }
    uint3 v = uint3(value.x, value.y, value.x ^ 0x9E3779B9u) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> 16u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return 0.15 + 0.85 * float3(v >> 24u) / 255.0;
}

// The texel of a `texels`-sized image at the center of the view's `pixel`.
int3 texel_at(uint2 pixel, uint2 texels) {
    float2 at = (float2(pixel) + 0.5) * float2(texels) / float2(push.size);
    return int3(min(uint2(at), texels - 1), 0);
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void show_main(uint3 pixel: SV_DispatchThreadID) {
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    uint2 texels;
    float3 shown = float3(0.0);
    if (push.kind == IDS || push.kind == VISIBILITY) {
        Texture2D<uint2> image = pairs[push.source];
        image.GetDimensions(texels.x, texels.y);
        shown = hashed(image.Load(texel_at(pixel.xy, texels)));
    } else if (push.kind == DEPTH || push.kind == SCALAR) {
        Texture2D<float> image = floats[push.source];
        image.GetDimensions(texels.x, texels.y);
        float value = image.Load(texel_at(pixel.xy, texels));
        // log2 of 0 is minus infinity, which saturates to 0.
        shown = stored_as(float3(push.kind == DEPTH ? 1.0 + log2(value) / 24.0 : value));
    } else if (push.kind != NONE) {
        Texture2D image = textures[push.source];
        image.GetDimensions(texels.x, texels.y);
        float3 value = image.Load(texel_at(pixel.xy, texels)).rgb;
        if (push.kind == MOTION) {
            shown = stored_as(value * 0.5 + 0.5);
        } else {
            // NaN shows as none, as in common.slang's `finite`.
            bool3 nan = (asuint(value) & 0x7FFFFFFF) > 0x7F800000;
            shown = clamp(select(nan, float3(0.0), value), 0.0, BRIGHTEST);
        }
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(shown, 1.0);
}
