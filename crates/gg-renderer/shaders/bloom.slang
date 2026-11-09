// Bloom: light spreading round what's bright in the game's view, as it does through a camera's lens,
// as Call of Duty: Advanced Warfare's (Jimenez, "Next Generation Post Processing in Call of Duty:
// Advanced Warfare", 2014). A chain of levels, each half the one before each way, rounded up, the
// first half the view: each texel of level k spans 2^k of the view's pixels each way, from its top
// left, so that a level's texel covers the 2 by 2 of the one before under it, past the edge where
// the one before is odd. Each level is downsampled from the one before by 13 bilinear taps, which
// read its 6 by 6 texels round the 2 by 2 under the texel: four boxes of 2 by 2 taps at its corners,
// each an eighth, and one at its middle, a half, so that what's small and bright doesn't shimmer as
// it moves across texels. The first, from the view's colors, reads them texel by texel, as light,
// which the view's colors can be infinite or no number, and weighs each box by 1 over 1 plus its
// luminance, Karis's average, so that a lone pixel far brighter than the rest, a firefly, doesn't
// bloom into a blot. Then from the smallest up, each level takes the one below by a 3 by 3 tent of
// bilinear taps a texel of that level apart, lerping toward it by its share, so that each level holds
// a weighted mean of itself and every level below, the weights adding up to 1; and the view takes the
// first level by the same tent, lerping toward it by the bloom's intensity, so that bloom spreads
// light without adding any. That last pass grades the view's light too, as grade.slang says, after
// blooming it, and runs to grade it alone where it doesn't bloom.
//
// `Push` matches `BloomPush` in src/bloom.rs field for field.

#include "grade.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's sampler that filters linearly, clamped to the edges: `Sampler::Linear` with
// `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// How many texels across and down a workgroup takes: `GROUP` in src/bloom.rs.
static const uint GROUP = 8;

struct Push {
    // The size of what's written, and of what's read.
    uint2 size;
    uint2 source_size;
    // What's read, a texture of the bindless set's, and what's written, a storage image of its.
    uint source;
    uint target;
    // How far an upsample lerps toward the level below, and the view toward the first level.
    float share;
    // Of the last pass, whether it blooms the view, `BLOOMS`, and grades it, `GRADES`.
    uint does;
    // How the last pass grades the view, if it does.
    Grade grade;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// `BloomPush::BLOOMS` and `BloomPush::GRADES` in src/bloom.rs.
static const uint BLOOMS = 1;
static const uint GRADES = 2;

// `color`, as the view's colors hold it, as light: none where it's no number or below 0, and the
// largest a half float holds where it's more, which there is infinite, as tonemap.slang's `finite`
// says. Told by its bits, as that says why.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}

float luminance(float3 color) {
    return dot(color, LUMA);
}

// The view's color at `at`, as light, but within its edges.
float3 drawn(int2 at) {
    at = clamp(at, int2(0, 0), int2(push.source_size) - 1);
    return finite(textures[push.source].Load(int3(at, 0)).rgb);
}

// The view's 2 by 2 pixels from `at`, averaged: what a bilinear tap between them gives.
float3 tap(int2 at) {
    return 0.25 * (drawn(at) + drawn(at + int2(1, 0)) + drawn(at + int2(0, 1)) + drawn(at + int2(1, 1)));
}

// Four taps, averaged, weighed by Karis's average: the weight in `w`.
float3 box(float3 a, float3 b, float3 c, float3 d, out float w) {
    float3 mean = 0.25 * (a + b + c + d);
    w = 1.0 / (1.0 + luminance(mean));
    return mean * w;
}

// The first level, from the view's colors, texel by texel.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void first_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    // The view's pixel at the top left of the 6 by 6 round the 2 by 2 under the texel.
    int2 at = int2(thread.xy) * 2 - 2;
    float3 a = tap(at), b = tap(at + int2(2, 0)), c = tap(at + int2(4, 0));
    float3 d = tap(at + int2(1, 1)), e = tap(at + int2(3, 1));
    float3 f = tap(at + int2(0, 2)), g = tap(at + int2(2, 2)), h = tap(at + int2(4, 2));
    float3 i = tap(at + int2(1, 3)), j = tap(at + int2(3, 3));
    float3 k = tap(at + int2(0, 4)), l = tap(at + int2(2, 4)), m = tap(at + int2(4, 4));
    float w0, w1, w2, w3, w4;
    float3 sum = 0.5 * box(d, e, i, j, w0) + 0.125 * box(a, b, f, g, w1)
        + 0.125 * box(b, c, g, h, w2) + 0.125 * box(f, g, k, l, w3)
        + 0.125 * box(g, h, l, m, w4);
    float weight = 0.5 * w0 + 0.125 * (w1 + w2 + w3 + w4);
    storage[push.target][uint3(thread.xy, 0)] = float4(sum / weight, 1.0);
}

// The level before's bilinear tap at `at`, in its texels, from its top left.
float3 sampled(float2 at) {
    float2 uv = at / float2(push.source_size);
    return textures[push.source].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0).rgb;
}

// Each level after the first, from the one before, by bilinear taps between its texels.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void down_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    // Between the 2 by 2 under the texel, in the level before's texels.
    float2 at = float2(thread.xy) * 2.0 + 1.0;
    float3 a = sampled(at + float2(-2, -2)), b = sampled(at + float2(0, -2));
    float3 c = sampled(at + float2(2, -2)), d = sampled(at + float2(-1, -1));
    float3 e = sampled(at + float2(1, -1)), f = sampled(at + float2(-2, 0));
    float3 g = sampled(at), h = sampled(at + float2(2, 0));
    float3 i = sampled(at + float2(-1, 1)), j = sampled(at + float2(1, 1));
    float3 k = sampled(at + float2(-2, 2)), l = sampled(at + float2(0, 2));
    float3 m = sampled(at + float2(2, 2));
    float3 sum = 0.125 * (d + e + i + j) + 0.03125 * (a + c + k + m)
        + 0.0625 * (b + f + h + l) + 0.125 * g;
    storage[push.target][uint3(thread.xy, 0)] = float4(sum, 1.0);
}

// The level below at the middle of the texel at `texel` of the level above it, by a 3 by 3 tent of
// bilinear taps a texel of the level below apart.
float3 tent(uint2 texel) {
    float2 at = (float2(texel) + 0.5) * 0.5;
    float3 sum = 4.0 * sampled(at);
    sum += 2.0 * (sampled(at + float2(-1, 0)) + sampled(at + float2(1, 0))
                  + sampled(at + float2(0, -1)) + sampled(at + float2(0, 1)));
    sum += sampled(at + float2(-1, -1)) + sampled(at + float2(1, -1))
        + sampled(at + float2(-1, 1)) + sampled(at + float2(1, 1));
    return sum / 16.0;
}

// Each level but the smallest, from the smallest up, toward the level below it, in place.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void up_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    uint3 to = uint3(thread.xy, 0);
    float3 level = storage[push.target][to].rgb;
    storage[push.target][to] = float4(lerp(level, tent(thread.xy), push.share), 1.0);
}

// The view's colors, toward the first level by the bloom's intensity, if it blooms, then graded, if
// it grades, in place, their alpha kept.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void composite_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    uint3 to = uint3(thread.xy, 0);
    float4 color = storage[push.target][to];
    float3 light = finite(color.rgb);
    if ((push.does & BLOOMS) != 0) {
        light = lerp(light, tent(thread.xy), push.share);
    }
    if ((push.does & GRADES) != 0) {
        light = graded(light, push.grade);
    }
    storage[push.target][to] = float4(light, color.a);
}
