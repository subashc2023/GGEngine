// A camera's lens and film over the game's view, before its canvas, so that a game's UI shows none of
// them, a pixel a thread, from the view's colors into an image of its own: chromatic aberration, red
// spread out from the view's middle and blue drawn in, each taken from nearer the middle or further
// out, as a lens bends each color by its own amount; a vignette, darkening toward the corners by the
// cos⁴ law, as a lens's light falls off away from its axis, as Unreal's natural vignetting does; and
// film grain, each pixel's light scaled by triangular noise from a hash of the pixel and the frame's
// number, the same for a frame of a number each run, alike in every channel.
//
// `Push` matches `LensPush` in src/lens.rs field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's sampler that filters linearly, clamped to the edges: `Sampler::Linear` with
// `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// How many pixels across and down a workgroup takes: `GROUP` in src/lens.rs.
static const uint GROUP = 8;

struct Push {
    // The view's size, its colors, a texture of the bindless set's, and what's written, a storage
    // image of its.
    uint2 size;
    uint source;
    uint target;
    // How far red's spread out from the middle and blue drawn in, as a share of the way; the tangent
    // of the angle off the lens's axis that the view's corners stand at, for the vignette; and the
    // most the grain scales light by either way, as a share.
    float aberration;
    float vignette;
    float grain;
    // The frame's number, for the grain.
    uint frame;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;

// `color`, as the view's colors hold it, as light: none where it's no number or below 0, and the
// largest a half float holds where it's more, as tonemap.slang's `finite` says.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}

// PCG3D, a hash of three numbers into three (Jarzynski and Olano, "Hash Functions for GPU
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
        // Within a half float's largest, which the grain could scale light past, and which would
        // be written as infinite, for the canvas to blend into no number.
        light = min(light * max(1.0 + push.grain * (noise.x + noise.y - 1.0), 0.0), BRIGHTEST);
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(light, drawn.a);
}
