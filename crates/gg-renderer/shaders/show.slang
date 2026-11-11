// An image the frame makes, shown in the game's view in place of its light, a pixel a thread, into
// colors of the view's own that the tonemap pass then maps, as src/show.rs plans it: the image
// stretched over the view, each pixel the texel at its center, read as its kind says, as `Shown`
// in gg-api says a capture reads it. Colors as they are; depth as `1 + log2(depth) / 24`, white at
// the near plane, where reversed depth is 1, a 24th darker each time the distance doubles, and
// black where nothing drew, at 0; IDs and a visibility buffer as a color for each value, black for
// 0; motion as normals are, each axis from -1 to 1 as 0 to 1; and a float a pixel as its value. An
// image of no kind, which the frame doesn't make, shows black.
//
// `Push` matches `ShowPush` in src/show.rs field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] Texture2D<float> floats[];
[[vk::binding(0, 0)]] Texture2D<uint2> pairs[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// How many pixels across and down a workgroup takes: `GROUP` in src/show.rs.
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
    // The view's size, the image, a texture of the bindless set's, and what's written, a storage
    // image of its.
    uint2 size;
    uint source;
    uint kind;
    uint target;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;

// `value`, each channel from 0 to 1, as the view's colors hold it so that the view's sRGB image
// stores it as its byte over 255, as shading.slang's `stored_as` says.
float3 stored_as(float3 value) {
    float3 v = saturate(value);
    return select(v <= 0.04045, v / 12.92, pow((v + 0.055) / 1.055, 2.4));
}

// A color for `value`, one of each value's, from a hash of it by PCG3D (Jarzynski and Olano, "Hash
// Functions for GPU Rendering", 2020), bright enough to tell apart: black for 0.
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

// The texel of an image `texels` across and down at the center of `pixel` of the view.
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
            // No number shows as none, as tonemap.slang's `finite` takes it.
            bool3 nan = (asuint(value) & 0x7FFFFFFF) > 0x7F800000;
            shown = clamp(select(nan, float3(0.0), value), 0.0, BRIGHTEST);
        }
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(shown, 1.0);
}
