// Screen-space reflections: what each opaque pixel's mirror shows, traced through the view's depth
// before the view shades. Shading takes a found reflection in place of the sky's specular light,
// blurred by roughness. Rays find the colors the frame before showed, antialiased, before its
// bloom, grade and canvas.
//
// Passes, a pixel a thread:
// - `depth_main` and `reduce_main`: a pyramid of the nearest front depth and the farthest back,
//   each level's sides half the last's, rounded up, so texel `c` of level `n` covers pixels
//   `c * 2^n` to `(c + 1) * 2^n`. A pixel's back is its back faces' depth where that bounds what
//   shows there, a closed mesh's far side; else its front moved `thickness` farther.
// - `trace_main`: each pixel's mirror ray through the pyramid, into the first level of the
//   reflections and of their surfaces, which shading tells apart by. Their images are the view's
//   padded to whole texels of their last level, so each level's texel `c` covers pixels `c * 2^n`
//   to `(c + 1) * 2^n`, where shading samples it.
// - `blur_main`: the coarser levels of both, which shading samples to blur a reflection.
// - `keep_main`: the drawn, resolved view's colors, for the next frame's rays.
//
// The trace is AMD's FidelityFX SSSR's, under the MIT license; functions follow its
// `FFX_SSSR_HierarchicalRaymarch` and `FFX_SSSR_ValidateHit`:
// https://github.com/GPUOpen-Effects/FidelityFX-SSSR/blob/master/ffx-sssr/ffx_sssr.h
// It finds a pixel's interval from front to back, not all that's behind the front, so a ray goes
// on past what it passes behind, as Voxagon's "A better depth buffer for raymarching" says:
// https://blog.voxagon.se/2017/11/22/a-better-depth-buffer-for-raymarching.html
// Within a closed mesh's interval it takes the front's colors, which stand in for its hidden side.
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
    // The colors the frame before kept, a bindless texture; the storage slots of the first level of
    // the reflections and of their surfaces.
    uint colors;
    uint target;
    uint surfaces;
    // The pyramid's levels.
    uint levels;
    // A ray's reach, in world units, and how far behind what the depth shows it may pass and still
    // find it where no back face bounds it.
    float distance;
    float thickness;
    // Where each level starts in the pyramid, in texels.
    uint starts[MOST_LEVELS];
    // Takes the kept colors to this frame's light: this frame's exposure over theirs.
    float scale;
    // Bindless textures, or 0 for none: world-space normals of what drew each of `Push`'s two
    // depths, alpha 1 where there's one.
    uint normals[2];
    // Not 0: the first level holds each ray's outcome, `trace_main` says how, for
    // `Channel::ReflectionRays`.
    uint rays;
    uint unused[2];
};

struct Push {
    // Every level of the pyramid, end to end, each row by row: the nearest front, then the farthest
    // back.
    float2* pyramid;
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
    // The blurs': the storage slots of the surfaces' level read and level written.
    uint2 surfaces;
    // The depth of the nearest back faces, a bindless texture.
    uint back;
    // The first level's: `Trace`'s, to move a front `thickness` farther.
    float depth_mul;
    float depth_add;
    float thickness;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Pixels across and down a workgroup: `GROUP` in src/reflections.rs.
static const uint GROUP = 8;
// The most steps a ray takes, texel to texel at any level. Between one surface's back and another's
// front, a ray steps at the finest levels: 64 left rays passing behind the showcase's glowing orb
// short of the wall past it.
static const uint MOST_STEPS = 128;
// The largest float short of infinity, FidelityFX's FFX_SSSR_FLOAT_MAX.
static const float FLOAT_MAX = 3.402823466e38;
// How far past a texel's edge a ray steps into the next, in pixels, as FidelityFX's `uv_offset`.
static const float NUDGE = 0.005;
// The share of the view's height over which hits fade toward its edges, as FidelityFX's; and of a
// ray's reach over which they fade toward its end.
static const float EDGE_FADE = 0.05;
static const float DISTANCE_FADE = 0.25;
// How far behind the front, as a share of its depth, a back face must be to bound it: nearer, it's
// the front drawn again, as a double-sided mesh draws both its sides.
static const float SAME_SURFACE = 1.0e-4;

// The back depth at `pixel` that bounds what shows there, of front depth `front`: the nearest back
// faces', where they're behind it, a closed mesh's far side; else 0, none. A back face nearer than
// the front is an open mesh's, or one the camera is inside.
float closed_back(int2 pixel, float front) {
    float back = depths[push.back].Load(int3(pixel, 0));
    return back < front * (1.0 - SAME_SURFACE) ? back : 0.0;
}

// The first level's texel `at`: the nearer of the view's two depths, and its back. Where nothing
// opaque is, the back is above every depth, so it bounds no coarser texel.
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
    float front = max(a, b);
    float back = closed_back(int2(at), front);
    if (front == 0.0) {
        back = FLOAT_MAX;
    } else if (back == 0.0) {
        float farther = push.depth_mul / (push.depth_add + front) + push.thickness;
        back = push.depth_mul / farther - push.depth_add;
    }
    push.pyramid[push.target + at.y * push.size.x + at.x] = float2(front, back);
}

// The level read's texel at `at`, clamped.
float2 below_at(uint2 at) {
    at = min(at, push.source_size - 1);
    return push.pyramid[push.source + at.y * push.source_size.x + at.x];
}

// A level after the first, each texel the nearest front and farthest back of the 2 by 2 under it.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void reduce_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (at.x >= push.size.x || at.y >= push.size.y) {
        return;
    }
    uint2 under = at * 2;
    float2 a = below_at(under);
    float2 b = below_at(under + uint2(1, 0));
    float2 c = below_at(under + uint2(0, 1));
    float2 d = below_at(under + uint2(1, 1));
    float2 bounds = float2(max(max(a.x, b.x), max(c.x, d.x)), min(min(a.y, b.y), min(c.y, d.y)));
    push.pyramid[push.target + at.y * push.size.x + at.x] = bounds;
}

// Level `level`'s size: the view's halved that many times, rounded up.
uint2 level_size(Trace trace, uint level) {
    return (trace.size + (1u << level) - 1) >> level;
}

// The nearest front and farthest back over level `level`'s texel `texel`, clamped.
float2 bounds_at(Trace trace, uint level, int2 texel) {
    uint2 size = level_size(trace, level);
    uint2 at = uint2(clamp(texel, int2(0), int2(size) - 1));
    return push.pyramid[trace.starts[level] + at.y * size.x + at.x];
}

// The nearest depth over level `level`'s texel `texel`, clamped.
float nearest_at(Trace trace, uint level, int2 texel) {
    return bounds_at(trace, level, texel).x;
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
// along `way`, in pixels and depth, comes between a pixel's front and back before it leaves the
// view, passes its end or takes its last step. `t`, where it stops, runs from 0 at the origin to 1
// at the end. Texels it's in front of or behind, it skips.
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
        float2 bounds = bounds_at(trace, uint(level), int2(cell));
        bool above = bounds.x < at.z;
        bool beneath = at.z < bounds.y;
        // Where it leaves the texel's sides, and reaches its nearest front or farthest back, which,
        // the depth reversed, only a ray going away from the view can from above, and one coming
        // toward it from beneath.
        float3 planes = float3((cell + side) * texel + nudge, above ? bounds.x : bounds.y);
        float3 ts = (planes - origin) * inverse;
        ts.z = (above ? way.z < 0.0 : way.z > 0.0) ? ts.z : FLOAT_MAX;
        float least = min(min(ts.x, ts.y), ts.z);
        bool outside = above || beneath;
        // Compared by their bits, as FidelityFX does: whether the least is the depth's.
        bool skipped = asuint(least) != asuint(ts.z) && outside;
        t = outside ? least : t;
        level = skipped ? min(level + 1, top) : level - 1;
    }
    return level < 0 && t <= 1.0;
}

// Whether every component of `v` is a finite number, told by its bits, as `finite` tells NaN.
bool all_finite(float3 v) {
    return all((asuint(v) & 0x7F800000) != 0x7F800000);
}

// Each pixel's mirror ray, into the reflections' first level: the colors found times their trust,
// and that trust; 0 where nothing was found or nothing opaque is. And into their surfaces' first
// level: the pixel's normal in the view, and its ray's spread times that trust, 0 where nothing
// opaque is. The spread is in pixels per unit of a lobe's angle's tangent: `t / (d + t)` of the
// pixels a unit spans a unit ahead, for a point `d` from the eye and its hit `t` from the point, as
// a flat mirror shows what it reflects `d + t` away; shading narrows it where the surface curves.
//
// Where `Trace::rays` asks, the first level holds each ray's outcome instead, by `stored_as`, as
// `Channel::ReflectionRays` lists them: red how far behind what's there it passed, green its trust,
// where it found something trusted; flat colors where it found nothing, or nothing it trusts.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void trace_main(uint3 thread: SV_DispatchThreadID) {
    // Loaded whole, as the model shaders load the frame's data.
    Trace trace = *push.trace;
    uint2 pixel = thread.xy;
    uint3 to = uint3(pixel, 0);
    // The padding past the view, which the blurs read: nothing there.
    if (any(pixel >= trace.size)) {
        storage[trace.target][to] = float4(0.0);
        storage[trace.surfaces][to] = float4(0.0);
        return;
    }
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
        storage[trace.surfaces][to] = float4(0.0);
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
    // surface itself; the sky isn't found. Inside a closed mesh, the ray found it wherever it came
    // in. Elsewhere, it isn't found from behind, and is trusted less the farther behind the ray
    // passed.
    int2 hit = int2(stopped.xy);
    float surface = nearest_at(trace, 0, hit);
    bool closed = closed_back(hit, surface) > 0.0;
    bool apart = any(abs(stopped.xy - origin.xy) >= 2.0);
    // Turned back. Or why `march` found nothing, as its checks tell: the end passed, the view
    // left, or else its steps taken. Or what it found: the surface itself, or nothing opaque.
    float3 outcome = float3(0.0, 1.0, 1.0);
    if (away > 0.0) {
        float3 at = origin + t * way;
        bool left = any(at.xy < 0.0) || any(at.xy >= size);
        outcome = found ? (apart ? float3(0.5) : float3(1.0, 1.0, 0.0))
            : t > 1.0 ? float3(0.0, 0.0, 0.5)
            : left ? float3(0.0, 0.0, 1.0)
            : float3(1.0);
    }
    if (found && apart && surface > 0.0) {
        float3 seen = position(trace, stopped.xy / size, view_depth(trace, surface));
        float3 facing = surface_normal(trace, hit, position(trace, (float2(hit) + 0.5) / size,
                                                            view_depth(trace, surface)), surface);
        outcome = float3(1.0, 0.0, 1.0);
        if (closed || dot(facing, ray) <= 0.0) {
            // Trusted less toward the view's edges and toward the ray's reach. Where no back face
            // bounds it, it found the front, the less the farther behind it the ray passed.
            float behind = closed ? 1.0
                : 1.0 - smoothstep(0.0, trace.thickness, length(seen - reached));
            reached = closed ? reached : seen;
            float2 uv = stopped.xy / size;
            float2 margin = EDGE_FADE * float2(size.y / size.x, 1.0);
            float2 border = smoothstep(float2(0.0), margin, uv)
                * (1.0 - smoothstep(1.0 - margin, float2(1.0), uv));
            float traveled = length(reached - center);
            float far = saturate((trace.distance - traveled) / (trace.distance * DISTANCE_FADE));
            trust = behind * behind * border.x * border.y * far * away;
            outcome = float3(1.0 - behind * behind, trust, 0.0);
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
                outcome = float3(1.0, 0.5, 0.0);
            }
        }
    }
    float distance = length(reached - center);
    float per_unit = size.y / abs(trace.view_mul.y);
    float spread = min(distance / (length(center) + distance), 1.0) * per_unit;
    // Kept a number, which shading picks a level by, as `min` keeps no NaN on every device.
    bool kept = all_finite(float3(spread, trust, 0.0));
    storage[trace.target][to] = trace.rays != 0 ? float4(stored_as(outcome), 1.0)
        : kept ? float4(color * trust, trust)
        : float4(0.0);
    storage[trace.surfaces][to] = float4(normal, kept ? spread * trust : 0.0);
}

// Storage slot `slot`'s texel at `at`, of the level read; nothing past its sides.
float4 finer_at(uint slot, int2 at) {
    if (any(at < 0) || any(at >= int2(push.source_size))) {
        return float4(0.0);
    }
    return storage[slot][uint3(uint2(at), 0)];
}

// A later level of the reflections and of their surfaces, each texel the binomial mean of the 4 by
// 4 under it, weighed 1, 3, 3, 1 each way: of the reflections' colors times their trust, and their
// trust; of the surfaces' normals, as long as the share covered where they're alike, and their
// spreads times their trust. Each keeps the middle of what it averages, which a 2 by 2 mean moves
// to its texel's, so a reflection read from it slides with the view rather than jumping a texel at
// a time.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void blur_main(uint3 thread: SV_DispatchThreadID) {
    uint2 at = thread.xy;
    if (any(at >= push.size)) {
        return;
    }
    static const float WEIGHTS[4] = { 1.0, 3.0, 3.0, 1.0 };
    int2 first = int2(at) * 2 - 1;
    float4 colors = float4(0.0);
    float4 surfaces = float4(0.0);
    for (int y = 0; y < 4; y++) {
        for (int x = 0; x < 4; x++) {
            float weight = WEIGHTS[x] * WEIGHTS[y];
            int2 under = first + int2(x, y);
            colors += weight * finer_at(push.source, under);
            surfaces += weight * finer_at(push.surfaces.x, under);
        }
    }
    storage[push.target][uint3(at, 0)] = colors / 64.0;
    storage[push.surfaces.y][uint3(at, 0)] = surfaces / 64.0;
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
