// The game's view's light metered, for its exposure to adapt to, as Unreal's histogram meters it: a
// histogram of the log2 of each pixel's luminance, in candela per square meter, and the mean of that
// log2 over the pixels between two percentiles of it, each bin's at its middle, a bin that a
// percentile cuts counting for the share of it on the inside.
//
// In three passes: `clear_main` empties the histogram; `histogram_main`, a pixel a thread, takes
// each pixel's color back through the exposure the frame shaded at to its luminance and counts it
// into its bin, into the workgroup's own bins first, which then add into the histogram's, so that a
// bin takes an atomic add a workgroup rather than a pixel; and `average_main`, a bin a thread, one
// workgroup, ranks the pixels by bin, darkest first, and writes the mean and how many pixels it
// counted for.
//
// `Push` matches `ExposurePush` in src/exposure.rs field for field.

#include "common.slang"
#include "grade.slang"

[[vk::binding(0, 0)]] Texture2D textures[];

// How many bins the histogram has, and how many a stop of luminance takes, from 2^LOWEST candela
// per square meter, darker than starlight, to 2^20, which only the sun's disk and lamps' filaments
// pass: `BINS`, `PER_STOP` and `LOWEST` in src/exposure.rs.
static const uint BINS = 256;
static const float PER_STOP = 8.0;
static const float LOWEST = -12.0;

// How many pixels across and down a workgroup of the histogram's takes, a bin a thread: `GROUP` in
// src/exposure.rs.
static const uint GROUP = 16;

struct Push {
    // How many of the view's pixels each bin holds.
    uint* bins;
    // What the meter found: the mean log2 of the luminance of the pixels it counted for, and how many
    // that was.
    float* metered;
    // The view's size in pixels, and its colors, a texture of the bindless set's.
    uint2 size;
    uint colors;
    // 1 over what lit shading multiplied light by in the frame: what takes the view's colors back to
    // luminance.
    float unexposed;
    // The percentiles between which the meter counts the pixels, as fractions.
    float low;
    float high;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("compute")]
[numthreads(BINS, 1, 1)]
void clear_main(uint3 thread: SV_DispatchThreadID) {
    push.bins[thread.x] = 0;
}

groupshared uint counted[BINS];

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void histogram_main(uint3 thread: SV_DispatchThreadID, uint index: SV_GroupIndex) {
    counted[index] = 0;
    GroupMemoryBarrierWithGroupSync();
    if (all(thread.xy < push.size)) {
        float3 color = finite(textures[push.colors].Load(int3(int2(thread.xy), 0)).rgb);
        float stops = log2(dot(color, LUMA) * push.unexposed) - LOWEST;
        // What's darker than the lowest bin, black among it, as a sky after dusk, isn't metered, so
        // that a view half of such a sky meters what's lit; no number fails the test too.
        if (stops >= 0.0) {
            uint bin = uint(min(stops * PER_STOP, float(BINS - 1)));
            InterlockedAdd(counted[bin], 1);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    uint count = counted[index];
    if (count > 0) {
        InterlockedAdd(push.bins[index], count);
    }
}

groupshared uint ranks[BINS];
groupshared float2 sums[BINS];

[shader("compute")]
[numthreads(BINS, 1, 1)]
void average_main(uint3 thread: SV_DispatchThreadID) {
    uint bin = thread.x;
    uint count = push.bins[bin];
    // An inclusive scan of the counts, by Hillis and Steele's steps: the rank past each bin's last
    // pixel.
    ranks[bin] = count;
    GroupMemoryBarrierWithGroupSync();
    for (uint offset = 1; offset < BINS; offset <<= 1) {
        uint before = bin >= offset ? ranks[bin - offset] : 0;
        GroupMemoryBarrierWithGroupSync();
        ranks[bin] += before;
        GroupMemoryBarrierWithGroupSync();
    }
    // The ranks counted, from the darkest pixel's 0: those between the percentiles, but at least
    // the pixel at the low one, so that percentiles of one value meter it.
    float total = float(ranks[BINS - 1]);
    float low = min(push.low * total, total - 1.0);
    float high = max(push.high * total, low + 1.0);
    float end = float(ranks[bin]);
    float start = end - float(count);
    float weight = max(min(end, high) - max(start, low), 0.0);
    float middle = LOWEST + (float(bin) + 0.5) / PER_STOP;
    sums[bin] = float2(weight * middle, weight);
    GroupMemoryBarrierWithGroupSync();
    for (uint apart = BINS / 2; apart > 0; apart >>= 1) {
        if (bin < apart) {
            sums[bin] += sums[bin + apart];
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (bin == 0) {
        float2 sum = sums[0];
        push.metered[0] = sum.y > 0.0 ? sum.x / sum.y : 0.0;
        push.metered[1] = sum.y;
    }
}
