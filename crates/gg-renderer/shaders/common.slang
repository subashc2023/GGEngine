// Helpers shaders share. Included where they're used, never compiled alone.

#pragma once

static const float PI = 3.14159265358979;

// The largest finite half float: the view's colors are half floats.
static const float BRIGHTEST = 65504.0;

// The view-space depth of what's infinitely far, as the sky: past any sample's reach, and far
// enough short of a float's largest that its points and their differences stay finite.
static const float FARTHEST = 1.0e30;

// `Sampler::LinearClamp`'s index in the bindless set.
static const uint LINEAR_CLAMP = 0;

// Luminance weights: Rec. 709's, the sRGB primaries' own, as ASC CDL's saturation weighs them.
static const float3 LUMA = float3(0.2126, 0.7152, 0.0722);

float luminance(float3 color) {
    return dot(color, LUMA);
}

// `color` kept to what the view's colors hold as light: 0 where NaN or below 0, `BRIGHTEST` above
// it, which a shader's float can exceed and round-to-nearest writes as infinity. NaN is told by its
// bits: a driver may fold `isnan` away, as Vulkan lets it assume no float is NaN or infinite.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}

// `value`, each channel 0 to 1, as the view's colors hold it for its sRGB image, which the tonemap
// pass writes clamped, to store it as its byte over 255: decoded from sRGB, which the image
// encodes again.
float3 stored_as(float3 value) {
    float3 v = saturate(value);
    return select(v <= 0.04045, v / 12.92, pow((v + 0.055) / 1.055, 2.4));
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
