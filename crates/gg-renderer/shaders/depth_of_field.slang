// Depth of field: what's nearer or farther than the focus blurred, as through a thin lens, each point
// spread over its circle of confusion, gathered at half the view's size into a near field and a far
// one and composited by their circles, as Call of Duty: Advanced Warfare's (Jimenez, "Next
// Generation Post Processing in Call of Duty: Advanced Warfare", 2014). A circle's radius, in pixels,
// is `add + mul * depth` of the view's reversed depth, signed: more than 0 past the focus, less before
// it, as src/depth_of_field.rs says, kept within the largest.
//
// In five passes. `downsample_main` takes each 2 by 2 of the view's pixels to a texel at half its size,
// their mean color and the nearest one's circle, so that what's thin and near isn't lost. `tiles_main`
// finds each tile's largest circle before the focus, and `dilate_main` each tile's largest of those
// round it that reach it, since what's near spreads past its own edges, over what's in focus. Then
// `gather_main` gathers each texel's two fields from a disk of rings of texels round it, as wide as the
// largest circle that can reach it: what's near scattered toward it, by each texel's circle that
// covers it, each weighed by 1 over its circle's area, as its light is spread over it, with how much of
// the texel it covers; and what's at or past the focus, each texel's circle kept within the texel's
// own, so that what's behind it doesn't spread over what's nearer and sharper. Last, `composite_main`
// lerps each of the view's pixels toward the far field by its own circle, from 1 pixel to 2, and lays
// the near field over that by its coverage.
//
// `Push` matches `DepthOfFieldPush` in src/depth_of_field.rs field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's sampler that filters linearly, clamped to the edges: `Sampler::Linear` with
// `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// How many texels across and down a workgroup takes: `GROUP` in src/depth_of_field.rs.
static const uint GROUP = 8;
// How many texels at half the view's size a tile spans each way, a workgroup of the tiles' pass:
// `TILE` in src/depth_of_field.rs.
static const uint TILE = 8;

// The most texels apart a gather's rings are, as many rings as keep them so, up to the most.
static const float SPACING = 1.25;
static const uint MOST_RINGS = 5;

static const float TAU = 6.28318530717959;

struct Push {
    // The view's size, half of it rounded up, and its tiles' count each way.
    uint2 size;
    uint2 halved_size;
    uint2 tiles;
    // The view's colors, which the downsample samples and the composite writes, and its depth.
    uint colors;
    uint depth;
    // The view at half its size, its tiles' circles, and those dilated, textures of the bindless
    // set's.
    uint halved;
    uint tiled;
    // The far field and the near one, which the gather writes and the composite samples.
    uint far;
    uint near;
    // What the downsample, the tiles' pass and the dilation write, a storage image of the set's.
    uint target;
    // A circle's radius in the view's pixels, `add + mul * depth`, and the largest.
    float add;
    float mul;
    float largest;
    // How many tiles a dilation reaches each way.
    uint reach;
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

// The circle of confusion's radius at reversed depth `depth`, in the view's pixels, signed.
float circle(float depth) {
    return clamp(push.add + push.mul * depth, -push.largest, push.largest);
}

// Each texel at half the view's size: the mean of the 2 by 2 pixels under it, past the edge where the
// view is odd, and the nearest one's circle, in texels at that size.
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

// The largest circle before the focus among a tile's texels, a float's bits, which order as unsigned
// integers do for floats of no sign.
groupshared uint nearest_bits;

// Each tile's largest circle before the focus, of its texels at half the view's size, a texel a
// thread.
[shader("compute")]
[numthreads(TILE, TILE, 1)]
void tiles_main(uint3 thread: SV_DispatchThreadID, uint3 group: SV_GroupID, uint index: SV_GroupIndex) {
    if (index == 0) {
        nearest_bits = 0;
    }
    GroupMemoryBarrierWithGroupSync();
    if (all(thread.xy < push.halved_size)) {
        float c = textures[push.halved].Load(int3(thread.xy, 0)).a;
        // So that a circle of -0 counts as 0, whose bits are the least.
        InterlockedMax(nearest_bits, asuint(c < 0.0 ? -c : 0.0));
    }
    GroupMemoryBarrierWithGroupSync();
    if (index == 0) {
        storage[push.target][uint3(group.xy, 0)] = float4(asfloat(nearest_bits), 0.0, 0.0, 1.0);
    }
}

// Each tile's largest circle before the focus of those round it that reach it: within `reach` tiles,
// of a circle past the tiles between them.
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

// What a gather sums of its texels: each field's light, weighed, and its weights; and the light of
// what isn't near, alike, what shows through the near field where it's of the near field itself.
struct Gathered {
    float3 far;
    float far_weight;
    float3 near;
    float near_weight;
    float3 behind;
    float behind_weight;
};

// How much of the near field a texel of circle `c`, in texels at half the view's size, is: from a
// circle of 1 texel before the focus to 2, so that what's a hair before it stays sharp at the view's
// size.
float nearness(float c) {
    return saturate(-c - 1.0);
}

// `gathered` with `texel`, a texel at half the view's size `r` texels from the one gathering, whose
// own circle past the focus is `own`.
void gather(inout Gathered gathered, float4 texel, float r, float own) {
    float c = texel.a;
    float near = nearness(c);
    // Past the focus, no larger than the texel's own, as what's behind it is hidden by it.
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

// Each texel's far field and near field, at half the view's size: from the texel and rings of texels
// round it, each ring a band of the disk as wide, of 8 texels for each ring it is out, so that each
// texel stands for as much of the disk, `π radius² / count`, the odd rings turned half a texel. The near
// field is written premultiplied by its coverage: the light its texels spread over the one gathering,
// each of a share of the disk over its circle's area, together. Where the texel gathering is of the
// near field, its far field is the mean of what's round it that isn't, what its blurred edge lets
// show of what's behind it.
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
    // As far as the texel's own circle and the soft texel past its edge, so that it blurs alike
    // however far what's near reaches round it, and as far as that.
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

// Each of the view's pixels, in place, its alpha kept: toward the far field by its own circle, from 1
// pixel to 2 past the focus, or by how much of the near field it is before it, the near field laid
// over that.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void composite_main(uint3 thread: SV_DispatchThreadID) {
    if (any(thread.xy >= push.size)) {
        return;
    }
    uint3 to = uint3(thread.xy, 0);
    float4 sharp = storage[push.colors][to];
    float c = circle(textures[push.depth].Load(int3(thread.xy, 0)).r);
    // The pixel's middle at half the view's size, whose texels each span 2 of its pixels from its top
    // left, past its edge where it's odd.
    float2 uv = (float2(thread.xy) + 0.5) * 0.5 / float2(push.halved_size);
    SamplerState filter = samplers[LINEAR_CLAMP];
    float3 far = textures[push.far].SampleLevel(filter, uv, 0.0).rgb;
    float4 near = textures[push.near].SampleLevel(filter, uv, 0.0);
    float3 light = lerp(finite(sharp.rgb), far, max(saturate(c - 1.0), nearness(c * 0.5)));
    light = light * (1.0 - near.a) + near.rgb;
    storage[push.colors][to] = float4(light, sharp.a);
}
