// A camera's lens and film over the view, a pixel a thread, from its colors into an image of its
// own. Before the canvas, so a game's UI shows none of it.
// - Chromatic aberration: red spread out from the middle and blue drawn in, each read from nearer
//   or further out, as a lens bends each color its own amount.
// - Vignette: darkening toward the corners by the cos⁴ law, as a lens's light falls off its axis,
//   as Unreal's natural vignetting does.
// - Film grain: each pixel's light scaled by triangular noise hashed from the pixel and the frame
//   number, alike for that frame each run and in every channel.
//
// `Push` matches `LensPush` in src/lens.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// A workgroup's pixels across and down: `GROUP` in src/lens.rs.
static const uint GROUP = 8;

struct Push {
    // The view's size; its colors, a bindless texture; what's written, a bindless storage image.
    uint2 size;
    uint source;
    uint target;
    // How far red spreads out and blue draws in, as a share of the way; for the vignette, the
    // tangent of the view corners' angle off the lens's axis; the most grain scales light either
    // way, as a share.
    float aberration;
    float vignette;
    float grain;
    // The frame's number, for the grain.
    uint frame;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// PCG3D, hashing three numbers into three (Jarzynski and Olano, "Hash Functions for GPU
// Rendering", 2020).
uint3 pcg3d(uint3 v) {
    v = v * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> 16u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void lens_main(uint3 pixel: SV_DispatchThreadID) {
    if (any(pixel.xy >= push.size)) {
        return;
    }
    Texture2D colors = textures[push.source];
    float4 drawn = colors.Load(int3(pixel.xy, 0));
    float3 light = finite(drawn.rgb);
    float2 size = float2(push.size);
    float2 middle = (float2(pixel.xy) + 0.5) - size * 0.5;
    if (push.aberration > 0.0) {
        SamplerState filter = samplers[LINEAR_CLAMP];
        float2 red = (size * 0.5 + middle * (1.0 - push.aberration)) / size;
        float2 blue = (size * 0.5 + middle * (1.0 + push.aberration)) / size;
        light.r = finite(colors.SampleLevel(filter, red, 0.0).rgb).r;
        light.b = finite(colors.SampleLevel(filter, blue, 0.0).rgb).b;
    }
    // From the middle, as a share of the way to a corner.
    float away = length(middle) / length(size * 0.5);
    float tangent = push.vignette * away;
    float lit = 1.0 / (1.0 + tangent * tangent);
    light *= lit * lit;
    if (push.grain > 0.0) {
        float2 noise = float2(pcg3d(uint3(pixel.xy, push.frame)).xy >> 8u) / 16777216.0;
        // Within a half float's largest: grain could scale light past it, written as infinite, for
        // the canvas to blend into NaN.
        light = min(light * max(1.0 + push.grain * (noise.x + noise.y - 1.0), 0.0), BRIGHTEST);
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(light, drawn.a);
}
