// Screen-space reflections: what each opaque pixel's mirror shows, traced through the view's depth
// before the view shades. Shading takes a found reflection in place of the sky's specular light,
// blurred by roughness. Rays find the colors the frame before showed, antialiased, before its
// bloom, grade and canvas.
//
// Passes, a pixel a thread:
// - `depth_main` and `reduce_main`: a pyramid of the nearest depth, each level's sides half the
//   last's, rounded up, so texel `c` of level `n` covers pixels `c * 2^n` to `(c + 1) * 2^n`.
// - `trace_main`: each pixel's mirror ray through the pyramid.
// - `blur_main`: the reflections' coarser levels, which shading samples to blur them.
// - `keep_main`: the drawn, resolved view's colors, for the next frame's rays.
//
// The trace is AMD's FidelityFX SSSR's, under the MIT license; functions follow its
// `FFX_SSSR_HierarchicalRaymarch` and `FFX_SSSR_ValidateHit`:
// https://github.com/GPUOpen-Effects/FidelityFX-SSSR/blob/master/ffx-sssr/ffx_sssr.h
// Positions are in pixels across and down, and the view's depth. View space is ao.slang's: X right,
// Y up and Z ahead, so depth is positive.
//
// Normals are the shading's, bumps included, where a pass found them: visibility.slang's
// `normal_main` for the visibility buffer, and model.slang where the opaque and masked model
// shaders drew depth alone. Elsewhere, as under a game's shader, they come from the depth: a
// curved mesh's flat facets, whose rays step at their edges.
//
// The pyramid is a pointer the push constants hold, indexed, and each entry point reads through
// functions of its own, as pyramid.slang says why. `Push` and `Trace` match `Push` and `Trace` in
// src/reflections.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D<float> depths[];
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];
[[vk::binding(2, 0)]] [format("r32f")] RWTexture2DArray<float> scalars[];

// The most levels the pyramid has: `MOST_LEVELS` in src/reflections.rs.
static const uint MOST_LEVELS = 12;

// What the trace reads.
struct Trace {
    // To the frame before's clip space, without its offset, as the kept colors were drawn.
    float4x4 previous_clip_from_view;
    // Turns the shading's normals.
    float4x4 view_from_world;
    // As ao.slang's `Push` has them: from a point's texture coordinates, from the view's top left,
    // to its view-space X and Y over its depth, `(view_mul * uv + view_add) * depth`; and from the
    // view's depth `z` to view-space depth, `depth_mul / (depth_add + z)`.
    float2 view_mul;
    float2 view_add;
    float depth_mul;
    float depth_add;
    // In pixels.
    uint2 size;
    // The colors the frame before kept, a bindless texture; the storage slots of the reflections'
    // first level and of their spread.
    uint colors;
    uint target;
    uint spread;
    // The pyramid's levels.
    uint levels;
    // A ray's reach, in world units, and how far behind what the depth shows it may pass and still
    // find it.
    float distance;
    float thickness;
    // Where each level starts in the pyramid, in texels.
    uint starts[MOST_LEVELS];
    // Takes the kept colors to this frame's light: this frame's exposure over theirs.
    float scale;
    // Bindless textures, or 0 for none: world-space normals of what drew each of `Push`'s two
    // depths, alpha 1 where there's one.
    uint normals[2];
    uint unused[3];
};

struct Push {
    // Every level of the pyramid, end to end, each row by row.
    float* pyramid;
    // What `trace_main` reads.
    Trace* trace;
    // Bindless textures, the view's two depths; the nearer is what's opaque.
    uint2 depths;
    // What's written, in texels.
    uint2 size;
    // What's read, in texels.
    uint2 source_size;
    // What's read: the pyramid level's start; the reflections' level's storage slot; or the view's
    // colors, a bindless texture.
    uint source;
    // What's written: the pyramid level's start, or a storage slot.
    uint target;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Pixels across and down a workgroup: `GROUP` in src/reflections.rs.
static const uint GROUP = 8;
// The most steps a ray takes, texel to texel at any level.
static const uint MOST_STEPS = 64;
// The largest float short of infinity, FidelityFX's FFX_SSSR_FLOAT_MAX.
static const float FLOAT_MAX = 3.402823466e38;
// How far past a texel's edge a ray steps into the next, in pixels, as FidelityFX's `uv_offset`.
static const float NUDGE = 0.005;
// The share of the view's height over which hits fade toward its edges, as FidelityFX's; and of a
// ray's reach over which they fade toward its end.
static const float EDGE_FADE = 0.05;
static const float DISTANCE_FADE = 0.25;

// The nearer of the view's two depths at the first level's texel `at`.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void depth_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (at.x >= push.size.x || at.y >= push.size.y) {
        return;
    }
    float a = depths[push.depths.x].Load(int3(int2(at), 0));
    float b = depths[push.depths.y].Load(int3(int2(at), 0));
    // Reversed: the nearer is the greater, and 0, where nothing opaque drew, infinitely far.
    push.pyramid[push.target + at.y * push.size.x + at.x] = max(a, b);
}

// The level read's texel at `at`, clamped.
float below_at(uint2 at) {
    at = min(at, push.source_size - 1);
    return push.pyramid[push.source + at.y * push.source_size.x + at.x];
}

// A level after the first, each texel the nearest of the 2 by 2 under it.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void reduce_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (at.x >= push.size.x || at.y >= push.size.y) {
        return;
    }
    uint2 under = at * 2;
    float nearest = max(max(below_at(under), below_at(under + uint2(1, 0))),
                        max(below_at(under + uint2(0, 1)), below_at(under + uint2(1, 1))));
    push.pyramid[push.target + at.y * push.size.x + at.x] = nearest;
}

// Level `level`'s size: the view's halved that many times, rounded up.
uint2 level_size(Trace trace, uint level) {
    return (trace.size + (1u << level) - 1) >> level;
}

// The nearest depth over level `level`'s texel `texel`, clamped.
float nearest_at(Trace trace, uint level, int2 texel) {
    uint2 size = level_size(trace, level);
    uint2 at = uint2(clamp(texel, int2(0), int2(size) - 1));
    return push.pyramid[trace.starts[level] + at.y * size.x + at.x];
}

float view_depth(Trace trace, float z) {
    return min(trace.depth_mul / (trace.depth_add + z), FARTHEST);
}

float3 position(Trace trace, float2 uv, float depth) {
    return float3((trace.view_mul * uv + trace.view_add) * depth, depth);
}

// Where `p`, in view space ahead of the view, shows: its pixel across and down, and its depth.
float3 screen(Trace trace, float3 p) {
    float2 uv = (p.xy / p.z - trace.view_add) / trace.view_mul;
    return float3(uv * float2(trace.size), trace.depth_mul / p.z - trace.depth_add);
}

// The nearest point the view shows at `pixel`, clamped.
float3 point_at(Trace trace, int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(trace.size) - 1);
    float z = nearest_at(trace, 0, at);
    return position(trace, (float2(at) + 0.5) / float2(trace.size), view_depth(trace, z));
}

// The way along the surface from `center`: toward the pixel after or from the one before,
// whichever the depth runs to more smoothly, of those in the view.
float3 along(float3 center, float3 before, float3 after, bool has_before, bool has_after) {
    float3 forward = after - center;
    float3 backward = center - before;
    if (!has_before) {
        return forward;
    }
    if (!has_after) {
        return backward;
    }
    return abs(forward.z) < abs(backward.z) ? forward : backward;
}

// The normal at `pixel`, whose point is `center`, from the points round it: toward the view, as
// the pixel below is down view space.
float3 normal_at(Trace trace, int2 pixel, float3 center) {
    int2 last = int2(trace.size) - 1;
    float3 across = along(center, point_at(trace, pixel + int2(-1, 0)),
                          point_at(trace, pixel + int2(1, 0)), pixel.x > 0, pixel.x < last.x);
    float3 down = along(center, point_at(trace, pixel + int2(0, -1)),
                        point_at(trace, pixel + int2(0, 1)), pixel.y > 0, pixel.y < last.y);
    float3 normal = cross(across, down);
    float length2 = dot(normal, normal);
    return length2 > 1.0e-30 ? normal * rsqrt(length2) : -normalize(center);
}

// The normal at `pixel`, in the view, whose point is `center` and nearer depth `z`: the shading's,
// where what drew that depth has normals and drew one there; else `normal_at`'s.
float3 surface_normal(Trace trace, int2 pixel, float3 center, float z) {
    int3 at = int3(pixel, 0);
    [unroll]
    for (uint i = 0; i < 2; i++) {
        if (trace.normals[i] != 0 && depths[push.depths[i]].Load(at) == z) {
            float4 shaded = textures[trace.normals[i]].Load(at);
            if (shaded.a > 0.0) {
                return normalize(mul(trace.view_from_world, float4(shaded.xyz, 0.0)).xyz);
            }
        }
    }
    return normal_at(trace, pixel, center);
}

// FFX_SSSR_HierarchicalRaymarch, from the pyramid's first level: whether the ray from `origin`
// along `way`, in pixels and depth, finds something before it leaves the view, passes its end or
// takes its last step. `t`, where it stops, runs from 0 at the origin to 1 at the end.
bool march(Trace trace, float3 origin, float3 way, out float t) {
    float3 inverse = float3(way.x != 0.0 ? 1.0 / way.x : FLOAT_MAX,
                            way.y != 0.0 ? 1.0 / way.y : FLOAT_MAX,
                            way.z != 0.0 ? 1.0 / way.z : FLOAT_MAX);
    // The side of each texel it leaves by, and its step past that side, each way.
    float2 side = float2(way.x < 0.0 ? 0.0 : 1.0, way.y < 0.0 ? 0.0 : 1.0);
    float2 nudge = float2(way.x < 0.0 ? -NUDGE : NUDGE, way.y < 0.0 ? -NUDGE : NUDGE);
    // Out of its own pixel first, which it would otherwise find itself in.
    float2 leaving = (floor(origin.xy) + side + nudge - origin.xy) * inverse.xy;
    t = min(leaving.x, leaving.y);
    float2 size = float2(trace.size);
    int top = int(trace.levels) - 1;
    int level = 0;
    for (uint step = 0; step < MOST_STEPS && level >= 0; step++) {
        float3 at = origin + t * way;
        if (t > 1.0 || any(at.xy < 0.0) || any(at.xy >= size)) {
            return false;
        }
        float texel = float(1u << uint(level));
        float2 cell = floor(at.xy / texel);
        float surface = nearest_at(trace, uint(level), int2(cell));
        // Where it leaves the texel's sides, and reaches the depth of its nearest surface, which,
        // the depth reversed, only a ray going away from the view can.
        float3 planes = float3((cell + side) * texel + nudge, surface);
        float3 ts = (planes - origin) * inverse;
        ts.z = way.z < 0.0 ? ts.z : FLOAT_MAX;
        float least = min(min(ts.x, ts.y), ts.z);
        bool above = surface < at.z;
        // Compared by their bits, as FidelityFX does: whether the least is the surface's.
        bool skipped = asuint(least) != asuint(ts.z) && above;
        t = above ? least : t;
        level = skipped ? min(level + 1, top) : level - 1;
    }
    return level < 0 && t <= 1.0;
}

// Whether every component of `v` is a finite number, told by its bits, as `finite` tells NaN.
bool all_finite(float3 v) {
    return all((asuint(v) & 0x7F800000) != 0x7F800000);
}

// Each pixel's mirror ray, into the reflections' first level: the colors found times their trust,
// and that trust; 0 where nothing was found or nothing opaque is. And its spread, in pixels per
// unit of a lobe's angle's tangent: `t / (d + t)` of the pixels a unit spans a unit ahead, for a
// point `d` from the eye and its hit, or where it stopped, `t` from the point, as a mirror shows
// what it reflects `d + t` away.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void trace_main(uint3 thread: SV_DispatchThreadID) {
    // Loaded whole, as the model shaders load the frame's data.
    Trace trace = *push.trace;
    uint2 pixel = thread.xy;
    if (pixel.x >= trace.size.x || pixel.y >= trace.size.y) {
        return;
    }
    uint3 to = uint3(pixel, 0);
    float z = nearest_at(trace, 0, int2(pixel));
    float2 size = float2(trace.size);
    float3 center = position(trace, (float2(pixel) + 0.5) / size, view_depth(trace, z));
    bool opaque = z > 0.0;
    float3 normal = float3(0.0);
    if (opaque) {
        normal = surface_normal(trace, int2(pixel), center, z);
    }
    // Nothing where nothing opaque is, nor where its normal is no number, as with sky on both
    // sides.
    if (!opaque || !all_finite(normal)) {
        storage[trace.target][to] = float4(0.0);
        scalars[trace.spread][to] = 0.0;
        return;
    }
    float3 view = normalize(center);
    float3 ray = reflect(view, normal);
    // Rays back toward the view find what's behind it, which it doesn't show: faded out from
    // across the view to halfway toward it.
    float away = saturate(1.0 - 2.0 * dot(ray, -view));
    // As far as its reach, but short of the near plane, toward which the depth stops.
    float near = trace.depth_mul / (trace.depth_add + 1.0);
    float reach = trace.distance;
    if (ray.z < 0.0) {
        reach = min(reach, 0.99 * (center.z - near) / -ray.z);
    }
    float3 origin = screen(trace, center);
    float3 way = screen(trace, center + ray * reach) - origin;
    float t = 0.0;
    bool found = away > 0.0 && march(trace, origin, way, t);
    float3 stopped = origin + clamp(t, 0.0, 1.0) * way;
    float3 reached = position(trace, stopped.xy / size, view_depth(trace, stopped.z));
    float trust = 0.0;
    float3 color = float3(0.0);
    // FFX_SSSR_ValidateHit: what's found as far as two pixels from where the ray set out is the
    // surface itself; the sky isn't found, nor what the ray finds from behind.
    int2 hit = int2(stopped.xy);
    float surface = nearest_at(trace, 0, hit);
    bool apart = any(abs(stopped.xy - origin.xy) >= 2.0);
    if (found && apart && surface > 0.0) {
        float3 seen = position(trace, stopped.xy / size, view_depth(trace, surface));
        float3 facing = surface_normal(trace, hit, position(trace, (float2(hit) + 0.5) / size,
                                                            view_depth(trace, surface)), surface);
        if (dot(facing, ray) <= 0.0) {
            // Trusted less the farther behind what's there the ray passed, toward the view's edges,
            // and toward the ray's reach.
            float behind = 1.0 - smoothstep(0.0, trace.thickness, length(seen - reached));
            float2 uv = stopped.xy / size;
            float2 margin = EDGE_FADE * float2(size.y / size.x, 1.0);
            float2 border = smoothstep(float2(0.0), margin, uv)
                * (1.0 - smoothstep(1.0 - margin, float2(1.0), uv));
            float traveled = length(seen - center);
            float far = saturate((trace.distance - traveled) / (trace.distance * DISTANCE_FADE));
            trust = behind * behind * border.x * border.y * far * away;
            reached = seen;
            // What the frame before kept where the hit showed then, at this frame's exposure;
            // nothing where it was out of that view.
            float4 then = mul(trace.previous_clip_from_view, float4(seen, 1.0));
            float2 kept = then.xy / then.w * 0.5 + 0.5;
            if (then.w > 0.0 && all(kept >= 0.0) && all(kept <= 1.0)) {
                SamplerState clamp_ = samplers[LINEAR_CLAMP];
                float3 found = textures[trace.colors].SampleLevel(clamp_, kept, 0.0).rgb;
                color = finite(found * trace.scale);
            } else {
                trust = 0.0;
            }
        }
    }
    float distance = length(reached - center);
    float per_unit = size.y / abs(trace.view_mul.y);
    float spread = min(distance / (length(center) + distance), 1.0) * per_unit;
    // Kept a number, which shading picks a level by, as `min` keeps no NaN on every device.
    bool kept = all_finite(float3(spread, trust, 0.0));
    storage[trace.target][to] = kept ? float4(color * trust, trust) : float4(0.0);
    scalars[trace.spread][to] = kept ? spread : 0.0;
}

// The level read's texel at `at`, clamped.
float4 finer_at(uint2 at) {
    at = min(at, push.source_size - 1);
    return storage[push.source][uint3(at, 0)];
}

// A later level of the reflections, each texel the mean of the 2 by 2 under it: of their colors
// times their trust, and of their trust, which shading divides the first by again.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void blur_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (at.x >= push.size.x || at.y >= push.size.y) {
        return;
    }
    uint2 under = at * 2;
    float4 sum = finer_at(under) + finer_at(under + uint2(1, 0)) + finer_at(under + uint2(0, 1))
        + finer_at(under + uint2(1, 1));
    storage[push.target][uint3(at, 0)] = sum / 4.0;
}

// The view's colors, as the frame drew and resolved them, kept for the next frame's rays.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void keep_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (at.x >= push.size.x || at.y >= push.size.y) {
        return;
    }
    float3 color = textures[push.source].Load(int3(int2(at), 0)).rgb;
    storage[push.target][uint3(at, 0)] = float4(finite(color), 1.0);
}
