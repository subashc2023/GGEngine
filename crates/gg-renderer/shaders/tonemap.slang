// The game's view, drawn in colors of high dynamic range, mapped into the range its image shows, 0
// to 1, by a tonemapper: each pixel of the view's colors onto the image's, which the target encodes
// as sRGB; and, tonemapped by any but none, dithered against banding in the image's 8 bits.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

struct Push {
    // The view's colors in the bindless set.
    uint image;
    // `Tonemapper::index` in src/tonemap.rs.
    uint tonemapper;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

static const uint PBR_NEUTRAL = 0;
static const uint ACES = 1;
static const uint AGX = 2;
static const uint NONE = 3;

// A triangle over the whole viewport, from three vertices and no buffer: (-1, -1), (3, -1) and
// (-1, 3) in clip space.
[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    float2 corner = float2((index << 1) & 2, index & 2);
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

// Khronos PBR Neutral, as Khronos's reference gives it, for colors that aren't negative:
// https://github.com/KhronosGroup/ToneMapping/blob/main/PBR_Neutral/pbrNeutral.glsl
float3 pbr_neutral(float3 color) {
    const float START_COMPRESSION = 0.8 - 0.04;
    const float DESATURATION = 0.15;
    float x = min(color.r, min(color.g, color.b));
    float offset = x < 0.08 ? x - 6.25 * x * x : 0.04;
    color -= offset;
    float peak = max(color.r, max(color.g, color.b));
    if (peak < START_COMPRESSION) {
        return color;
    }
    const float D = 1.0 - START_COMPRESSION;
    float new_peak = 1.0 - D * D / (peak + D - START_COMPRESSION);
    color *= new_peak / peak;
    float g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    return lerp(color, float3(new_peak), g);
}

// ACES, as Stephen Hill fitted it, from MJP's and David Neubelt's Baking Lab, under the MIT
// license: https://github.com/TheRealMJP/BakingLab/blob/master/BakingLab/ACES.hlsl
// sRGB to XYZ, D65 to D60, to AP1, and the RRT's saturation.
static const float3x3 ACES_INPUT = float3x3(
    0.59719, 0.35458, 0.04823,
    0.07600, 0.90834, 0.01566,
    0.02840, 0.13383, 0.83777);
// The ODT's saturation, to XYZ, D60 to D65, and to sRGB.
static const float3x3 ACES_OUTPUT = float3x3(
    1.60475, -0.53108, -0.07367,
    -0.10208, 1.10813, -0.00605,
    -0.00327, -0.07276, 1.07602);

float3 aces(float3 color) {
    color = mul(ACES_INPUT, color);
    // The RRT and ODT, fitted.
    float3 a = color * (color + 0.0245786) - 0.000090537;
    float3 b = color * (0.983729 * color + 0.4329510) + 0.238081;
    return saturate(mul(ACES_OUTPUT, a / b));
}

// AgX, as three.js has it, under the MIT license, after Filament's, which follows Blender's, in
// Rec. 2020's primaries:
// https://github.com/mrdoob/three.js/blob/dev/src/renderers/shaders/ShaderChunk/tonemapping_pars_fragment.glsl.js
// GLSL builds its matrices from their columns, which `mul(color, m)` takes these rows as.
static const float3x3 SRGB_TO_REC2020 = float3x3(
    0.6274, 0.0691, 0.0164,
    0.3293, 0.9195, 0.0880,
    0.0433, 0.0113, 0.8956);
static const float3x3 REC2020_TO_SRGB = float3x3(
    1.6605, -0.1246, -0.0182,
    -0.5876, 1.1329, -0.1006,
    -0.0728, -0.0083, 1.1187);
static const float3x3 AGX_INSET = float3x3(
    0.856627153315983, 0.137318972929847, 0.11189821299995,
    0.0951212405381588, 0.761241990602591, 0.0767994186031903,
    0.0482516061458583, 0.101439036467562, 0.811302368396859);
static const float3x3 AGX_OUTSET = float3x3(
    1.1271005818144368, -0.1413297634984383, -0.14132976349843826,
    -0.11060664309660323, 1.157823702216272, -0.11060664309660294,
    -0.016493938717834573, -0.016493938717834257, 1.2519364065950405);
// The log2 of middle gray, 0.18, times 2 to the -10 and to the 6.5.
static const float AGX_MIN_EV = -12.47393;
static const float AGX_MAX_EV = 4.026069;

float3 agx(float3 color) {
    color = mul(color, SRGB_TO_REC2020);
    color = mul(color, AGX_INSET);
    // In log2, from 1e-10, short of 0, which has none.
    color = log2(max(color, 1e-10));
    float3 x = saturate((color - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV));
    // The sigmoid, as Benjamin Wrensch approximated it:
    // https://iolite-engine.com/blog_posts/minimal_agx_implementation
    float3 x2 = x * x;
    float3 x4 = x2 * x2;
    color = 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2
        + 0.1191 * x - 0.00232;
    color = mul(color, AGX_OUTSET);
    color = pow(max(color, 0.0), 2.2);
    return saturate(mul(color, REC2020_TO_SRGB));
}

// sRGB's encoding, of linear values from 0 to 1, as the target writes them, and its decoding.
float3 srgb_encoded(float3 value) {
    return select(value <= 0.0031308, value * 12.92, 1.055 * pow(value, 1.0 / 2.4) - 0.055);
}

float3 srgb_decoded(float3 value) {
    return select(value <= 0.04045, value / 12.92, pow((value + 0.055) / 1.055, 2.4));
}

// PCG2D, a hash of two numbers into two (Jarzynski and Olano, "Hash Functions for GPU Rendering",
// 2020).
uint2 pcg2d(uint2 v) {
    v = v * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> 16u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    return v ^ (v >> 16u);
}

// `color`, from 0 to 1, dithered against banding in the target's 8 bits, where the target encodes
// it: triangular noise, of two uniform noises of up to half a step either way, from a hash of the
// pixel, the same each frame, so that frames of the same colors write the same; added to the color
// encoded, which is then decoded, for the target to encode again. Within a step of 0 or 1, the
// noise shrinks to reach no further than they, so that black and white stay as they are and no
// noise is clamped off, which would shift the mean.
float3 dithered(float3 color, uint2 pixel) {
    float2 halves = float2(pcg2d(pixel) >> 8u) / 16777216.0;
    float noise = (halves.x + halves.y - 1.0) / 255.0;
    float3 shown = srgb_encoded(color);
    float3 reach = saturate(min(shown, 1.0 - shown) * 255.0);
    return srgb_decoded(shown + noise * reach);
}

[shader("fragment")]
float4 fragment_main(float4 position: SV_Position) : SV_Target {
    // A pixel's position is its center, so its whole part is the pixel's.
    int2 pixel = int2(position.xy);
    float4 drawn = textures[push.image].Load(int3(pixel, 0));
    // As what blends can leave below 0, and as the tonemappers would make infinity no number.
    float3 color = finite(drawn.rgb);
    switch (push.tonemapper) {
    case PBR_NEUTRAL:
        color = pbr_neutral(color);
        break;
    case ACES:
        color = aces(color);
        break;
    case AGX:
        color = agx(color);
        break;
    default:
        color = saturate(color);
        break;
    }
    if (push.tonemapper != NONE) {
        color = dithered(color, uint2(pixel));
    }
    return float4(color, saturate(drawn.a));
}
