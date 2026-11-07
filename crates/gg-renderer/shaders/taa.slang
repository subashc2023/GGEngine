// Temporal antialiasing's resolve: each pixel of the game's view, drawn a fraction of a pixel off
// this frame, as the frames before it were each by another, blended with what they resolved
// there, a pixel a thread, so that over the frames each pixel averages what's drawn across it. Its
// history, what the frame before resolved, is taken from where the pixel's point showed in it: its
// position in the world, from the view's depth, less how far it moved in the world since, which the
// view's motion holds, seen as the frame before saw it, without its offset; from the nearest of the
// 3 by 3 pixels round it, so that an edge that moves over what's behind it takes the edge's motion
// (Karis, "High Quality Temporal Supersampling", 2014). The history is sampled by a Catmull-Rom
// filter, in five bilinear samples, which keeps it sharp as it's taken between pixels (Jimenez,
// "Filmic SMAA", 2016), and clipped toward the middle of a box of this frame's 3 by 3 pixels round
// it, in YCoCg: their mean, give or take their deviation (Salvi, "An Excursion in Temporal
// Supersampling", 2016), within the least and the most of them, so that what's no longer there,
// uncovered or lit otherwise, doesn't linger as a ghost, nor the filter's undershoot at an edge as
// a dark fringe. Colors are weighed by 1 over 1 plus their luminance as they're clipped and
// blended, so that a lone bright pixel doesn't flicker as the offsets move it in and out of a
// pixel. Where the frame before resolved nothing, or the point showed outside it or behind its
// camera, the pixel takes this frame's alone.
//
// `Resolve` and `Push` match `Resolve` and `ResolvePush` in src/taa.rs, field for field.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// The bindless set's sampler that filters linearly, clamped to the edges: `Sampler::Linear` with
// `Wrap::Clamp`.
static const uint LINEAR_CLAMP = 0;

// How many pixels across and down a workgroup takes: `GROUP` in src/taa.rs.
static const uint GROUP = 8;

// How much of each pixel this frame's color makes, the rest the history's: as much as the last 10
// frames' or so together.
static const float CURRENT = 0.1;

// How many of the neighborhood's deviations from its mean the history may stray, each way.
static const float SPREAD = 1.25;

struct Resolve {
    // From the view's clip space, as this frame draws it, offset, to the world, homogeneous.
    float4x4 world_from_clip;
    // From the world to the view's clip space as the frame before drew it, without its offset.
    float4x4 previous_clip_from_world;
    // This frame's offset, in the view's normalized coordinates.
    float2 jitter;
    // The view's size in pixels.
    uint2 size;
    // The view's colors, its depth and its motion, as this frame drew them, and what the frame
    // before resolved, textures of the bindless set's.
    uint colors;
    uint depth;
    uint motion;
    uint history;
    // What this frame resolves into, for the next, and the view's resolved colors, if it writes
    // them apart, storage images of the bindless set's.
    uint target;
    uint resolved;
    // Whether there's a history to take, and whether the resolved colors are written apart.
    uint has_history;
    uint writes_resolved;
};

struct Push {
    Resolve* resolve;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

float luminance(float3 color) {
    return dot(color, float3(0.2126, 0.7152, 0.0722));
}

// `color` weighed by 1 over 1 plus its luminance, in YCoCg: where the neighborhood clips and the
// blend mixes, as `unweighed` takes back.
float3 weighed(float3 color) {
    float3 c = max(color, 0.0) / (1.0 + luminance(max(color, 0.0)));
    float y = dot(c, float3(0.25, 0.5, 0.25));
    float co = dot(c, float3(0.5, 0.0, -0.5));
    float cg = dot(c, float3(-0.25, 0.5, -0.25));
    return float3(y, co, cg);
}

float3 unweighed(float3 ycocg) {
    float t = ycocg.x - ycocg.z;
    float3 c = max(float3(t + ycocg.y, ycocg.x + ycocg.z, t - ycocg.y), 0.0);
    return c / max(1.0 - luminance(c), 1.0 / 65504.0);
}

// `history` clipped toward the middle of the box from `least` to `most`, along the line between
// them, to the box, if it's outside it.
float3 clipped(float3 history, float3 least, float3 most) {
    float3 middle = (least + most) * 0.5;
    float3 offset = history - middle;
    float3 units = abs(offset / max((most - least) * 0.5, 1.0e-6));
    float farthest = max(units.x, max(units.y, units.z));
    return farthest > 1.0 ? middle + offset / farthest : history;
}

// `history`, a texture `size` texels, at `uv`, by a Catmull-Rom filter of its 4 by 4 texels round
// it, the four corners left out, as five bilinear samples.
float3 catmull_rom(Texture2D history, float2 uv, float2 size) {
    SamplerState filter = samplers[LINEAR_CLAMP];
    float2 position = uv * size;
    float2 center = floor(position - 0.5) + 0.5;
    float2 f = position - center;
    float2 f2 = f * f;
    float2 f3 = f2 * f;
    float2 w0 = f2 - 0.5 * (f3 + f);
    float2 w1 = 1.5 * f3 - 2.5 * f2 + 1.0;
    float2 w3 = 0.5 * (f3 - f2);
    float2 w2 = 1.0 - w0 - w1 - w3;
    float2 w12 = w1 + w2;
    float2 at0 = (center - 1.0) / size;
    float2 at12 = (center + w2 / w12) / size;
    float2 at3 = (center + 2.0) / size;
    float4 sum = float4(history.SampleLevel(filter, float2(at12.x, at0.y), 0.0).rgb, 1.0)
            * (w12.x * w0.y)
        + float4(history.SampleLevel(filter, float2(at0.x, at12.y), 0.0).rgb, 1.0) * (w0.x * w12.y)
        + float4(history.SampleLevel(filter, at12, 0.0).rgb, 1.0) * (w12.x * w12.y)
        + float4(history.SampleLevel(filter, float2(at3.x, at12.y), 0.0).rgb, 1.0) * (w3.x * w12.y)
        + float4(history.SampleLevel(filter, float2(at12.x, at3.y), 0.0).rgb, 1.0) * (w12.x * w3.y);
    return max(sum.rgb / sum.w, 0.0);
}

[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void resolve_main(uint3 pixel: SV_DispatchThreadID) {
    // Loaded whole, as the model shaders load the frame's data.
    Resolve resolve = *push.resolve;
    if (pixel.x >= resolve.size.x || pixel.y >= resolve.size.y) {
        return;
    }
    int2 at = int2(pixel.xy);
    int2 last = int2(resolve.size) - 1;
    Texture2D colors = textures[resolve.colors];
    Texture2D depths = textures[resolve.depth];
    float4 drawn = colors.Load(int3(at, 0));
    // The 3 by 3 pixels round it: their mean and deviation, and their least and most, weighed; and
    // the nearest of them, whose depth is the most, as depth runs from 1 at the near plane.
    float3 sum = float3(0.0);
    float3 squares = float3(0.0);
    float3 least = float3(1.0e9);
    float3 most = float3(-1.0e9);
    float nearest = -1.0;
    int2 near = at;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            int2 p = clamp(at + int2(x, y), int2(0, 0), last);
            float3 c = weighed(colors.Load(int3(p, 0)).rgb);
            sum += c;
            squares += c * c;
            least = min(least, c);
            most = max(most, c);
            float depth = depths.Load(int3(p, 0)).r;
            if (depth > nearest) {
                nearest = depth;
                near = p;
            }
        }
    }
    float3 mean = sum / 9.0;
    float3 deviation = sqrt(max(squares / 9.0 - mean * mean, 0.0));
    least = max(least, mean - SPREAD * deviation);
    most = min(most, mean + SPREAD * deviation);
    float3 current = weighed(drawn.rgb);
    float3 resolved = current;
    if (resolve.has_history != 0) {
        // Where the nearest pixel's point is, homogeneous, at infinity for depth 0 under
        // perspective, and where it was, before it moved.
        float2 size = float2(resolve.size);
        float2 seen = (float2(near) + 0.5) / size * 2.0 - 1.0;
        float4 world = mul(resolve.world_from_clip, float4(seen, nearest, 1.0));
        float3 moved = textures[resolve.motion].Load(int3(near, 0)).xyz;
        float4 before = mul(resolve.previous_clip_from_world,
                            float4(world.xyz - moved * world.w, world.w));
        // How far it moved on screen, from where the frame before showed it to where this one
        // does without its offset, which takes the pixel back to where its history is.
        float2 motion = (seen - resolve.jitter) - before.xy / before.w;
        float2 uv = (float2(at) + 0.5) / size - motion * 0.5;
        bool inside = all(uv >= 0.0) && all(uv <= 1.0);
        if (before.w > 0.0 && inside) {
            float3 history = weighed(catmull_rom(textures[resolve.history], uv, size));
            if (!any(isnan(history))) {
                history = clipped(history, least, most);
                resolved = lerp(history, current, CURRENT);
            }
        }
    }
    float4 color = float4(unweighed(resolved), drawn.a);
    uint3 to = uint3(pixel.xy, 0);
    storage[resolve.target][to] = color;
    if (resolve.writes_resolved != 0) {
        storage[resolve.resolved][to] = color;
    }
}
