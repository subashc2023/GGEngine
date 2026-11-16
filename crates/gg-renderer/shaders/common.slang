// What shaders share alike: a half float's largest value, which the view's colors are kept within,
// and `finite`, which keeps a color to it. Included where they're used, never compiled alone; once,
// however many of a shader's includes include it.

#pragma once

// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;

// `color`, as the view's colors hold it, as light: none where it's no number or below 0, and the
// largest a half float holds where it's more, which there is infinite, as a shader's floats can be,
// and as one past that largest is written where the GPU rounds to the nearest. Told by its bits,
// which a driver may not fold away, as it may `isnan` where it assumes, as Vulkan lets it, that no
// float is NaN or infinite.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}
