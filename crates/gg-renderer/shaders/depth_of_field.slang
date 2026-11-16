// Depth of field, as through a thin lens: each point spread over its circle of confusion, gathered
// at half size into a near field and a far one, composited by their circles. Call of Duty:
// Advanced Warfare's (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced Warfare",
// 2014). A circle's radius in pixels is `add + mul * depth` of reversed depth, signed: more than 0
// past the focus, less before it, as src/depth_of_field.rs says, kept within the largest.
//
// Five passes:
// - `downsample_main`: 2 by 2 pixels to a half-size texel, their mean and the nearest's circle, so
//   what's thin and near isn't lost.
// - `tiles_main`: each tile's largest circle before the focus.
// - `dilate_main`: each tile's largest of those reaching it, as what's near spreads over what's in
//   focus.
// - `gather_main`: each texel's fields from a disk of rings as wide as the largest circle that can
//   reach it. Near: what's near scattered toward it, weighed by 1 over its circle's area and by how
//   much of the texel it covers. Far: what's at or past the focus, each circle within the texel's
//   own, so what's behind doesn't spread over what's nearer and sharper.
// - `composite_main`: each pixel lerped toward the far field by its circle, from 1 pixel to 2, the
//   near field laid over it by its coverage.
//
// `Push` matches `DepthOfFieldPush` in src/depth_of_field.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's `Sampler::Linear` with `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// Texels across and down a workgroup: `GROUP` in src/depth_of_field.rs.
static const uint GROUP = 8;
// Half-size texels a tile spans each way, a workgroup of the tiles' pass: `TILE` in
// src/depth_of_field.rs.
static const uint TILE = 8;

// The most texels apart a gather's rings are, with as many rings as that takes, up to the most.
static const float SPACING = 1.25;
static const uint MOST_RINGS = 5;

static const float TAU = 6.28318530717959;

struct Push {
    // The view's size, half that rounded up, and its tiles each way.
    uint2 size;
    uint2 halved_size;
    uint2 tiles;
    // The view's colors, which the downsample reads and the composite writes, and its depth.
    uint colors;
    uint depth;
    // Bindless textures: the half-size view, its tiles' circles, and those dilated.
    uint halved;
    uint tiled;
    // Written by the gather, sampled by the composite.
    uint far;
    uint near;
    // What the downsample, tiles and dilation write: a bindless storage image.
    uint target;
    // A circle's radius in pixels is `add + mul * depth`, within `largest`.
    float add;
    float mul;
    float largest;
    // Tiles a dilation reaches each way.
    uint reach;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The signed circle of confusion's radius at reversed `depth`, in pixels.
float circle(float depth) {
    return clamp(push.add + push.mul * depth, -push.largest, push.largest);
}

// Past the edge where the view is odd; circles in half-size texels.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void downsample_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.halved_size)) {
        return;
    }
    int2 last = int2(push.size) - 1;
    float3 sum = float3(0.0);
    float nearest = push.largest;
    for (int y = 0; y < 2; y++) {
        for (int x = 0; x < 2; x++) {
            int3 at = int3(min(int2(thread.xy) * 2 + int2(x, y), last), 0);
            sum += finite(textures[push.colors].Load(at).rgb);
            nearest = min(nearest, circle(textures[push.depth].Load(at).r));
        }
    }
    storage[push.target][uint3(thread.xy, 0)] = float4(sum * 0.25, nearest * 0.5);
}

// A tile's largest circle before the focus, as a float's bits, which order as unsigned integers do
// for floats of no sign.
groupshared uint nearest_bits;

// A half-size texel a thread.
[shader("compute")]
[numthreads(TILE, TILE, 1)]
void tiles_main(uint3 thread: SV_DispatchThreadID, uint3 group: SV_GroupID, uint index: SV_GroupIndex) {
    if (index == 0) {
        nearest_bits = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (all(thread.xy < push.halved_size)) {
        float c = textures[push.halved].Load(int3(thread.xy, 0)).a;
        // So -0 counts as 0, whose bits are the least.
        InterlockedMax(nearest_bits, asuint(c < 0.0 ? -c : 0.0));
    }
    GroupMemoryBarrierWithGroupSync();
    if (index == 0) {
        storage[push.target][uint3(group.xy, 0)] = float4(asfloat(nearest_bits), 0.0, 0.0, 1.0);
    }
}

// Reaching it: within `reach` tiles, and past the tiles between.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void dilate_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.tiles)) {
        return;
    }
    int reach = int(push.reach);
    int2 last = int2(push.tiles) - 1;
    float most = 0.0;
    for (int y = -reach; y <= reach; y++) {
        for (int x = -reach; x <= reach; x++) {
            int2 at = int2(thread.xy) + int2(x, y);
            if (any(at < 0) || any(at > last)) {
                continue;
            }
            float c = textures[push.tiled].Load(int3(at, 0)).r;
            float between = float(max(max(abs(x), abs(y)) - 1, 0) * int(TILE));
            if (c > between) {
                most = max(most, c);
            }
        }
    }
    storage[push.target][uint3(thread.xy, 0)] = float4(most, 0.0, 0.0, 1.0);
}

// Each field's weighed light and weight, and likewise what isn't near, which shows through the
// near field where the gathering texel is near.
struct Gathered {
    float3 far;
    float far_weight;
    float3 near;
    float near_weight;
    float3 behind;
    float behind_weight;
};

// How much of the near field a circle `c` in half-size texels is: from 1 texel before the focus to
// 2, so what's a hair before stays sharp at full size.
float nearness(float c) {
    return saturate(-c - 1.0);
}

// Adds `texel`, `r` half-size texels away; `own` is the gathering texel's circle past the focus.
void gather(inout Gathered gathered, float4 texel, float r, float own) {
    float c = texel.a;
    float near = nearness(c);
    // Past the focus, no larger than the texel's own, which hides what's behind it.
    float far_c = min(max(c, 0.0), own);
    float far = (1.0 - near) * saturate(far_c - r + 1.0) / max(far_c * far_c, 1.0);
    float near_c = max(-c, 0.0);
    float nearer = near * saturate(near_c - r + 1.0) / max(near_c * near_c, 1.0);
    gathered.far += texel.rgb * far;
    gathered.far_weight += far;
    gathered.near += texel.rgb * nearer;
    gathered.near_weight += nearer;
    gathered.behind += texel.rgb * (1.0 - near);
    gathered.behind_weight += 1.0 - near;
}

// Each ring is a band of the disk as wide, 8 texels per ring out, so each texel stands for an equal
// share, `π radius² / count`; odd rings turn half a texel. The near field is premultiplied by its
// coverage: its texels' shares of the disk over their circles' areas, summed. Where this texel is
// near, its far field is the mean of what's round it that isn't: what its blurred edge shows.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void gather_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.halved_size)) {
        return;
    }
    int2 at = int2(thread.xy);
    int2 last = int2(push.halved_size) - 1;
    Texture2D halved = textures[push.halved];
    float4 center = halved.Load(int3(at, 0));
    float own = max(center.a, 0.0);
    float reaching = textures[push.tiled].Load(int3(at / int(TILE), 0)).r;
    uint3 to = uint3(thread.xy, 0);
    if (max(own, reaching) < 0.5) {
        storage[push.far][to] = float4(center.rgb, 1.0);
        storage[push.near][to] = float4(0.0);
        return;
    }
    // Its own circle plus the soft texel past it, so it blurs alike however far what's near
    // reaches; and at least that reach.
    float radius = max(own + 1.0, reaching);
    uint rings = clamp(uint(ceil(radius / SPACING)), 1u, MOST_RINGS);
    float step = radius / (float(rings) + 0.5);
    Gathered gathered = { float3(0.0), 0.0, float3(0.0), 0.0, float3(0.0), 0.0 };
    gather(gathered, center, 0.0, own);
    for (uint ring = 1; ring <= rings; ring++) {
        uint count = 8 * ring;
        float turned = (ring & 1) != 0 ? 0.5 : 0.0;
        for (uint i = 0; i < count; i++) {
            float angle = (float(i) + turned) * TAU / float(count);
            float2 offset = float2(cos(angle), sin(angle)) * (step * float(ring));
            int2 p = clamp(at + int2(round(offset)), int2(0, 0), last);
            gather(gathered, halved.Load(int3(p, 0)), length(offset), own);
        }
    }
    float3 far = gathered.far_weight > 0.0 ? gathered.far / gathered.far_weight : center.rgb;
    if (gathered.behind_weight > 0.0) {
        far = lerp(far, gathered.behind / gathered.behind_weight, nearness(center.a));
    }
    float count = float(1 + 4 * rings * (rings + 1));
    float coverage = saturate(gathered.near_weight * radius * radius / count);
    float3 near = gathered.near_weight > 0.0 ? gathered.near / gathered.near_weight : float3(0.0);
    storage[push.far][to] = float4(far, 1.0);
    storage[push.near][to] = float4(near * coverage, coverage);
}

// In place, alpha kept; before the focus, toward the far field by its nearness.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void composite_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    uint3 to = uint3(thread.xy, 0);
    float4 sharp = storage[push.colors][to];
    float c = circle(textures[push.depth].Load(int3(thread.xy, 0)).r);
    // The pixel's center at half size, each texel 2 pixels from the top left, past the edge where
    // the view is odd.
    float2 uv = (float2(thread.xy) + 0.5) * 0.5 / float2(push.halved_size);
    SamplerState filter = samplers[LINEAR_CLAMP];
    float3 far = textures[push.far].SampleLevel(filter, uv, 0.0).rgb;
    float4 near = textures[push.near].SampleLevel(filter, uv, 0.0);
    float3 light = lerp(finite(sharp.rgb), far, max(saturate(c - 1.0), nearness(c * 0.5)));
    light = light * (1.0 - near.a) + near.rgb;
    storage[push.colors][to] = float4(light, sharp.a);
}
