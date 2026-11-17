// Motion blur along how each pixel's point moved while the shutter was open, from the view's
// colors, depth and motion: McGuire, Hennessy, Bukowski and Osman's reconstruction filter ("A
// Reconstruction Filter for Plausible Motion Blur", I3D 2012). What moves blurs over what's still
// behind it, which shows through.
//
// Four passes:
// - `velocity_main`: each pixel's motion in pixels, found as taa.slang finds it, times half the
//   shutter, within a tile's side; and its depth.
// - `tiles_main`: each tile's largest.
// - `neighbors_main`: each tile's largest of the 3 by 3 round it, as far as any pixel blurring into
//   it moves.
// - `gather_main`: each pixel and a fixed count of others along its tile's largest, either way,
//   jittered per pixel. What's in front weighs by how far its blur reaches the pixel, what's behind
//   by how far the pixel's blur lets it show, and what moves alike at a like depth by both.
//
// Tiles with nothing round them moving half a pixel are left as they were.
//
// `Push` and `Seen` match `BlurPush` and `Seen` in src/motion_blur.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// Pixels across and down a workgroup, and a tile's threads each way: `GROUP` in src/motion_blur.rs.
static const uint GROUP = 8;

// Pixels the gather takes, the pixel's own among them: McGuire et al.'s 15.
static const uint SAMPLES = 15;

// The share of its depth by which a pixel must be nearer to stand wholly in front; nearer by less,
// it's partly behind too.
static const float SOFT_DEPTH = 0.01;

struct Seen {
    // This frame's clip space, offset, to the world, homogeneous.
    float4x4 world_from_clip;
    // The world to the frame before's clip space, without its offset.
    float4x4 previous_clip_from_world;
    // This frame's offset, in the view's normalized coordinates.
    float2 jitter;
    // The share of the frame the shutter is open.
    float shutter;
    uint unused;
};

struct Push {
    // For the velocities.
    Seen* seen;
    // The view's size, a tile's side, and the tiles each way.
    uint2 size;
    uint2 tiles;
    uint tile;
    // Bindless textures: the view's colors, depth and motion, its velocities, and its tiles'
    // largest and their neighbors'.
    uint colors;
    uint depth;
    uint motion;
    uint velocity;
    uint tiled;
    // What the pass writes: a bindless storage image.
    uint target;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Each pixel's velocity, either way while the shutter is open, and its reversed depth.
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
    // A velocity that's no number, as a point at infinity makes without perspective, moves nothing.
    if (!(speed <= most)) {
        velocity = speed > most ? velocity * (most / speed) : float2(0.0);
    }
    storage[push.target][uint3(pixel.xy, 0)] = float4(velocity, depth, 0.0);
}

// Each thread's largest velocity in the tile, and its length squared, as the threads reduce them.
groupshared float3 largest[GROUP * GROUP];

// A workgroup a tile, each thread taking every eighth pixel each way.
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

// How much a point blurred `velocity` pixels either way covers one `distance` from it.
float cone(float distance, float velocity) {
    return saturate(1.0 - distance / velocity);
}

// Whether it covers it, softened at the blur's end.
float cylinder(float distance, float velocity) {
    return 1.0 - smoothstep(0.95 * velocity, 1.05 * velocity, distance);
}

// How much the point at reversed depth `a` is in front of `b`'s: wholly if nearer, not at all if
// farther by `SOFT_DEPTH` of the nearer's depth or more.
float in_front(float a, float b) {
    return saturate(1.0 - (b - a) / max(SOFT_DEPTH * max(a, b), 1.0e-7));
}

// A pixel a thread.
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
    // At least half a pixel: what's still is drawn across its pixel.
    float own_velocity = max(length(own.xy), 0.5);
    float weight = 1.0 / own_velocity;
    float3 sum = finite(color.rgb) * weight;
    // -0.5 to 0.5, the same each frame, shifting all the pixel's samples alike.
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
