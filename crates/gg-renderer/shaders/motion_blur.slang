// Motion blur: each of the view's pixels blurred along how what it shows moved across the view while
// the shutter was open, by McGuire, Hennessy, Bukowski and Osman's reconstruction filter ("A
// Reconstruction Filter for Plausible Motion Blur", I3D 2012), which blurs what moves over what's
// still behind it and lets what's behind show through what moves, from the view's colors, depth and
// motion.
//
// In four passes. `velocity_main` finds how far each pixel's point moved across the view, in pixels,
// as taa.slang finds it: where it is, from the view's depth, and where it was, less how far it moved
// in the world since the frame before, as the frame before's camera saw it; half of that times the
// shutter, how far it moves either way of where it shows while the shutter is open, kept within a
// tile's side, and its depth. `tiles_main` finds each tile's largest of those, and `neighbors_main`
// each tile's largest of the 3 by 3 round it, which is as far as any pixel that can blur into the
// tile's moves. Then `gather_main` takes each pixel and a fixed count of others along its tile's
// largest, either way, jittered by noise of the pixel's, each weighed by whether it's in front of the
// pixel or behind it and how far each of the two moves: what's in front by how far its own blur
// reaches the pixel, what's behind by how far the pixel's blur lets it show, and what moves alike at
// a depth alike by both. Where nothing round a tile moves half a pixel, its pixels are as they were.
//
// `Push` and `Seen` match `BlurPush` and `Seen` in src/motion_blur.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// How many pixels across and down a workgroup takes, and how many threads a tile's: `GROUP` in
// src/motion_blur.rs.
static const uint GROUP = 8;

// How many pixels the gather takes, each pixel's own among them: McGuire et al.'s 15.
static const uint SAMPLES = 15;

// How much nearer than another, as a share of its depth, a pixel must be to stand wholly in front of
// it: what's nearer by less is in front and behind at once, in part.
static const float SOFT_DEPTH = 0.01;

struct Seen {
    // From the view's clip space, as this frame draws it, offset, to the world, homogeneous.
    float4x4 world_from_clip;
    // From the world to the view's clip space as the frame before drew it, without its offset.
    float4x4 previous_clip_from_world;
    // This frame's offset, in the view's normalized coordinates.
    float2 jitter;
    // The share of the frame the shutter is open.
    float shutter;
    uint unused;
};

struct Push {
    // How this frame and the one before saw the view, for the velocities.
    Seen* seen;
    // The view's size, the side of a tile, and the tiles' count each way.
    uint2 size;
    uint2 tiles;
    uint tile;
    // The view's colors, depth and motion, its velocities, its tiles' largest and those of their
    // neighbors, textures of the bindless set's.
    uint colors;
    uint depth;
    uint motion;
    uint velocity;
    uint tiled;
    // What the pass writes, a storage image of the set's.
    uint target;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Each pixel's velocity: how far its point moves either way of where it shows while the shutter is
// open, in pixels, within a tile's side, and its reversed depth.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void velocity_main(uint3 pixel: SV_DispatchThreadID) {
    if (any(pixel.xy >= push.size)) {
        return;
    }
    // Loaded whole, as the model shaders load the frame's data.
    Seen seen = *push.seen;
    int3 at = int3(pixel.xy, 0);
    float depth = textures[push.depth].Load(at).r;
    float2 size = float2(push.size);
    float2 clip = (float2(pixel.xy) + 0.5) / size * 2.0 - 1.0;
    // Where it is, homogeneous, at infinity for depth 0 under perspective, and where it was.
    float4 world = mul(seen.world_from_clip, float4(clip, depth, 1.0));
    float3 moved = textures[push.motion].Load(at).xyz;
    float4 before = mul(seen.previous_clip_from_world, float4(world.xyz - moved * world.w, world.w));
    float2 velocity = float2(0.0);
    if (before.w > 0.0) {
        float2 motion = (clip - seen.jitter) - before.xy / before.w;
        velocity = motion * 0.5 * size * (0.5 * seen.shutter);
    }
    float most = float(push.tile);
    float speed = length(velocity);
    // A velocity no number, as a point at infinity under no perspective's makes, moves nothing.
    if (!(speed <= most)) {
        velocity = speed > most ? velocity * (most / speed) : float2(0.0);
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(velocity, depth, 0.0);
}

// The largest velocity among each thread's pixels of a tile, and its length squared, as the threads
// reduce them.
groupshared float3 largest[GROUP * GROUP];

// Each tile's largest velocity, of its pixels, a workgroup a tile, each thread taking every eighth
// pixel each way.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void tiles_main(uint3 thread: SV_GroupThreadID, uint3 group: SV_GroupID, uint index: SV_GroupIndex) {
    uint2 corner = group.xy * push.tile;
    uint2 end = min(corner + push.tile, push.size);
    float3 most = float3(0.0);
    for (uint y = corner.y + thread.y; y < end.y; y += GROUP) {
        for (uint x = corner.x + thread.x; x < end.x; x += GROUP) {
            float2 velocity = textures[push.velocity].Load(int3(x, y, 0)).xy;
            float squared = dot(velocity, velocity);
            if (squared > most.z) {
                most = float3(velocity, squared);
            }
        }
    }
    largest[index] = most;
    GroupMemoryBarrierWithGroupSync();
    for (uint stride = GROUP * GROUP / 2; stride > 0; stride /= 2) {
        if (index < stride && largest[index + stride].z > largest[index].z) {
            largest[index] = largest[index + stride];
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (index == 0) {
        storage[push.target][uint3(group.xy, 0)] = float4(largest[0].xy, 0.0, 0.0);
    }
}

// Each tile's largest velocity of the 3 by 3 tiles round it.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void neighbors_main(uint3 tile: SV_DispatchThreadID) {
    if (any(tile.xy >= push.tiles)) {
        return;
    }
    int2 last = int2(push.tiles) - 1;
    float3 most = float3(0.0);
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            int2 at = clamp(int2(tile.xy) + int2(x, y), int2(0, 0), last);
            float2 velocity = textures[push.tiled].Load(int3(at, 0)).xy;
            float squared = dot(velocity, velocity);
            if (squared > most.z) {
                most = float3(velocity, squared);
            }
        }
    }
    storage[push.target][uint3(tile.xy, 0)] = float4(most.xy, 0.0, 0.0);
}

// How far, as a share, a point blurred `velocity` pixels either way covers one `distance` from it.
float cone(float distance, float velocity) {
    return saturate(1.0 - distance / velocity);
}

// Whether a point blurred `velocity` pixels either way covers one `distance` from it, softened at the
// end of its blur.
float cylinder(float distance, float velocity) {
    return 1.0 - smoothstep(0.95 * velocity, 1.05 * velocity, distance);
}

// How much the point of reversed depth `a` is in front of that of `b`: wholly if it's nearer, and
// not at all if it's farther by `SOFT_DEPTH` of the nearer's depth or more.
float in_front(float a, float b) {
    return saturate(1.0 - (b - a) / max(SOFT_DEPTH * max(a, b), 1.0e-7));
}

// PCG2D, a hash of two numbers into two (Jarzynski and Olano, "Hash Functions for GPU Rendering",
// 2020), as tonemap.slang's.
uint2 pcg2d(uint2 v) {
    v = v * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> 16u);
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    return v ^ (v >> 16u);
}

// Each pixel of the view, blurred: the reconstruction filter's, a pixel a thread.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void gather_main(uint3 pixel: SV_DispatchThreadID) {
    if (any(pixel.xy >= push.size)) {
        return;
    }
    int2 at = int2(pixel.xy);
    uint3 to = uint3(pixel.xy, 0);
    Texture2D colors = textures[push.colors];
    float4 color = colors.Load(int3(at, 0));
    float2 largest = textures[push.tiled].Load(int3(at / int(push.tile), 0)).xy;
    if (dot(largest, largest) < 0.25) {
        storage[push.target][to] = float4(finite(color.rgb), color.a);
        return;
    }
    Texture2D velocities = textures[push.velocity];
    float3 own = velocities.Load(int3(at, 0)).xyz;
    // Within half a pixel, as what's still is drawn across its pixel.
    float own_velocity = max(length(own.xy), 0.5);
    float weight = 1.0 / own_velocity;
    float3 sum = finite(color.rgb) * weight;
    // Noise from -0.5 to 0.5, the same each frame, that shifts all the pixel's samples alike.
    float jitter = float(pcg2d(pixel.xy).x >> 8u) / 16777216.0 - 0.5;
    int2 last = int2(push.size) - 1;
    for (uint i = 0; i < SAMPLES; i++) {
        if (i == (SAMPLES - 1) / 2) {
            continue;
        }
        float t = lerp(-1.0, 1.0, (float(i) + jitter + 1.0) / float(SAMPLES + 1));
        int2 other = clamp(int2(round(float2(at) + largest * t)), int2(0, 0), last);
        float3 its = velocities.Load(int3(other, 0)).xyz;
        float its_velocity = max(length(its.xy), 0.5);
        float distance = length(float2(other - at));
        float front = in_front(its.z, own.z);
        float back = in_front(own.z, its.z);
        float alpha = front * cone(distance, its_velocity) + back * cone(distance, own_velocity)
            + cylinder(distance, its_velocity) * cylinder(distance, own_velocity) * 2.0;
        weight += alpha;
        sum += finite(colors.Load(int3(other, 0)).rgb) * alpha;
    }
    storage[push.target][to] = float4(sum / weight, color.a);
}
