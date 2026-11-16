// Helpers shaders share. Included where they're used, never compiled alone.

#pragma once

// The largest finite half float: the view's colors are half floats.
static const float BRIGHTEST = 65504.0;

// `color` kept to what the view's colors hold as light: 0 where NaN or below 0, `BRIGHTEST` above
// it, which a shader's float can exceed and round-to-nearest writes as infinity. NaN is told by its
// bits: a driver may fold `isnan` away, as Vulkan lets it assume no float is NaN or infinite.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}
