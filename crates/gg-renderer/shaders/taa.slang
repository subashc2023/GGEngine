// Temporal antialiasing's resolve, a pixel a thread. Each frame draws the view at another
// sub-pixel offset and blends it with the history, so each pixel averages what's drawn across it.
//
// - History: reprojected by depth and motion, without the frame before's offset, at the nearest of
//   the 3 by 3 pixels, so a moving edge takes its own motion (Karis, "High Quality Temporal
//   Supersampling", 2014); sampled by Catmull-Rom in five bilinear taps (Jimenez, "Filmic SMAA",
//   2016).
// - Clipped toward the middle of a YCoCg box of the 3 by 3 pixels, their mean give or take their
//   deviation (Salvi, "An Excursion in Temporal Supersampling", 2016), within their least and
//   most: no ghosts, and no dark fringe from the filter's undershoot.
// - Colors are weighed by 1 / (1 + luminance) as they're clipped and blended, so a lone bright
//   pixel doesn't flicker.
// - This frame's color: the 3 by 3 pixels, weighed by distance from the pixel's center at this
//   offset.
// - A still pixel takes less of this frame (Karis), or it follows each offset and never settles.
// - A short history gives each frame an equal share, so the first frames average fast.
// - The history is scaled by the exposure's change.
// - No history, or the point off the frame before or behind its camera: this frame's color alone.
//
// Still views: the clip throws out what's thinner than a pixel, or masked near its cutoff, at each
// offset that misses it, so it flickers. So each pixel keeps the luminance each offset last drew,
// and whether it matched the cycle before. A still pixel all 8 offsets matched isn't clipped: its
// luminance settles on their mean, by a still pixel's share a frame (at once, hundreds of pixels
// jumped together). Any other change breaks the match for two cycles, so it leaves no trail. FSR 2
// skips the clip where luminance returns (`ffx_fsr2_accumulate.h`, `RectifyHistory`), Unreal's TSR
// where it flickers within a bound; this asks for the same luminance at the same offset.
//
// Moving views: what's thinner than a pixel shows as dashes running along it, each clipped the
// frame after. So each pixel also tries a luminance-only resolve, clamped to the 3 by 3 pixels,
// then `TRIED` of the way to this frame's. Where that clamp keeps taking back what the frames put
// in, the pixel flickers. Pushes that aren't flicker:
// - A change pushes the way the frames pull, or where they pulled nothing, and ends the evidence.
// - An edge isn't pushed: the pixels round it hold both its sides.
// - A still pixel an offset draws otherwise than a cycle before changed, whatever the clamp does.
//
// A flickering pixel keeps its history's luminance past the 3 by 3 pixels, on the side the clamp
// pushed from and as far as the frames drew, and takes less of each frame, so it shows their mean
// (as Unreal's TSR, `r.TSR.ShadingRejection.Flickering`). Chroma is clipped as usual. Costs:
// behind what crosses such pixels, they show untested for tens of frames; what's drawn and gone
// faster than every 20 frames, while it or the view moves, shows its mean. Nothing a pixel wide or
// more is taken to flicker.
//
// `Resolve` and `Push` match `Resolve` and `ResolvePush` in src/taa.rs, field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// Pixels across and down a workgroup: `GROUP` in src/taa.rs.
static const uint GROUP = 8;

// This frame's share once the history is that long. A still pixel: about the last 25 frames',
// three cycles of the offsets. One moved a pixel or more, whose history blurs between pixels: the
// last 10's.
static const float CURRENT_STILL = 0.04;
static const float CURRENT_MOVED = 0.1;
// This frame's share of the color of a pixel all 8 offsets repeat; its luminance settles by
// `CURRENT_STILL`. A quarter of the still share's ripple, settling over a few hundred frames.
static const float CURRENT_REPEATS = 0.01;

// The falloff of a pixel's weight in this frame's color with its squared distance in pixels: a
// Gaussian of deviation 0.35, about a pixel box's.
static const float FALLOFF = 4.0;

// The neighborhood's deviations from its mean the history may stray, each way.
static const float SPREAD = 1.25;

// Offsets the view cycles through: `OFFSETS` in src/taa.rs.
static const uint OFFSETS = 8;

// Added to a kept luminance: the least normal half float, so black isn't 0, which marks none kept.
static const float KEPT_BLACK = 1.0 / 16384.0;

// How near, relative, an offset's luminance must come to a cycle before's to match: two half-float
// steps, as a still view's offsets draw exactly as they did.
static const float SAME = 2.0 / 1024.0;

// The most a still pixel may move between frames, in pixels: a still view's reprojection rounding.
static const float STILL = 0.01;

// This frame's share of the flicker test's resolve: more than the real one's, so what a frame puts
// in is plain to measure as the next frame's clamp takes it back.
static const float TRIED = 0.25;

// The least push that tells anything, relative to the luminance pushed: smaller counts as neither
// flicker nor change.
static const float TOLD = 0.02;

// The evidence that a pixel flickers:
// - `EVIDENCE_GAIN`: what a push that takes back all the frames put in adds.
// - `EVIDENCE_FADES`: what's left a frame later, halving in 23 frames.
// - `EVIDENCE_MOST`: the most it comes to.
// - `FLICKERS_FROM`: where a pixel starts to flicker, past one push, so what passes once changes
//   nothing.
// - `FLICKERS_BY`: where it wholly is, three pushes close together.
//
// What's drawn and gone every 20 frames or more stays under that. Fading by 0.95 does every 10,
// but slats 2.5 pixels apart then flicker half as much as without the test at some pans, not a
// quarter. By 0.985, what blinks every 20 frames is taken to flicker.
static const float EVIDENCE_GAIN = 0.3;
static const float EVIDENCE_FADES = 0.97;
static const float EVIDENCE_MOST = 2.0;
static const float FLICKERS_FROM = 0.35;
static const float FLICKERS_BY = 0.75;

// The push the frames' colors don't explain, in swings, that ends the evidence; a smaller one cuts
// it in proportion.
static const float CHANGE = 1.5;

// What's left a frame later of the swing, the largest push that took back what the frames put in,
// halving in 23 frames; and of what they put in that no push took back yet, halving in 34. Where
// the pixels round a line mostly hold it, the push comes many frames after.
static const float SWING_FADES = 0.97;
static const float UNSETTLED_FADES = 0.98;

// This frame's share of a flickering pixel once the evidence reaches `SETTLED_BY`: about the last
// 33 frames', a third of a moving pixel's ripple. Before, its usual share brings the history to
// what the frames draw in a few frames. `EVIDENCE_MOST` is well past `SETTLED_BY`, so a lasting
// flicker keeps one share; one following the evidence, which rises on misses, would weigh hits
// more and darken it.
static const float CURRENT_FLICKERS = 0.03;
static const float SETTLED_BY = 1.2;

struct Resolve {
    // From this frame's clip space, offset, to the world, homogeneous.
    float4x4 world_from_clip;
    // From the world to the frame before's clip space, without its offset.
    float4x4 previous_clip_from_world;
    // This frame's offset, in the view's normalized coordinates.
    float2 jitter;
    // In pixels.
    uint2 size;
    // Bindless textures: this frame's colors, depth and motion, and what the frame before resolved.
    uint colors;
    uint depth;
    uint motion;
    uint history;
    // Bindless storage images: what this frame resolves into, for the next, and the view's resolved
    // colors, if written apart.
    uint target;
    uint resolved;
    // Frames in a row the history holds, 0 for none; whether the resolved colors are written apart.
    uint held;
    uint writes_resolved;
    // What the history is multiplied by: this frame's exposure over the frame before's.
    float history_scale;
    // Bindless storage images, read and written in place: what each offset last drew of each pixel,
    // offsets 0 to 3 in the first, 4 to 7 in the second. Each is this frame's luminance as
    // `weighed` has it, plus `KEPT_BLACK`; negative where it differed from a cycle before; 0 where
    // none was kept.
    uint seen_first;
    uint seen_second;
    // Which offset this frame draws at, from 0.
    uint phase;
    // What the flicker test keeps of each pixel: the frame before's, a bindless texture sampled
    // bilinearly where the history is, and this frame's, a storage image.
    // - Red: its tried resolve's luminance, as `weighed` has it.
    // - Green: how far the frames' own colors moved it, which no clamp has taken back yet.
    // - Blue: the swing, the largest recent push that took such a move back, signed as the push.
    // - Alpha: the evidence.
    uint flicker;
    uint flickers;
    uint2 unused;
};

struct Push {
    Resolve* resolve;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// `color` weighed by 1 / (1 + luminance), in YCoCg, where the clip and blend work; `unweighed`
// undoes it.
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
    return c / max(1.0 - luminance(c), 1.0 / BRIGHTEST);
}

// A gray's luminance as `weighed` has it, of light `scale` times as bright.
float relit(float weighed, float scale) {
    float light = max(weighed, 0.0) / max(1.0 - weighed, 1.0 / BRIGHTEST) * scale;
    return light / (1.0 + light);
}

// `history`, if outside the box from `least` to `most`, clipped toward its middle onto it.
float3 clipped(float3 history, float3 least, float3 most) {
    float3 middle = (least + most) * 0.5;
    float3 offset = history - middle;
    float3 units = abs(offset / max((most - least) * 0.5, 1.0e-6));
    float farthest = max(units.x, max(units.y, units.z));
    return farthest > 1.0 ? middle + offset / farthest : history;
}

// `history`, of `size` texels, at `uv`: a Catmull-Rom filter of the 4 by 4 texels round it, less
// the corners, in five bilinear samples.
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
    // The 3 by 3 pixels: their weighed mean, deviation, least and most; this frame's color, each
    // weighed by how far from the pixel's center the offset drew it; and the nearest, of the most
    // depth, as depth runs from 1 at the near plane.
    float2 drawn_off = -resolve.jitter * float2(resolve.size) * 0.5;
    float3 current = float3(0.0);
    float weights = 0.0;
    float3 sum = float3(0.0);
    float3 squares = float3(0.0);
    float3 least = float3(1.0e9);
    float3 most = float3(-1.0e9);
    float nearest = -1.0;
    int2 near = at;
    for (int y = -1; y <= 1; y++) {
        for (int x = -1; x <= 1; x++) {
            int2 p = clamp(at + int2(x, y), int2(0, 0), last);
            float3 c = weighed(finite(colors.Load(int3(p, 0)).rgb));
            sum += c;
            squares += c * c;
            least = min(least, c);
            most = max(most, c);
            float2 off = float2(x, y) + drawn_off;
            float weight = exp(-FALLOFF * dot(off, off));
            current += c * weight;
            weights += weight;
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
    current /= weights;
    // Without a history, the pixel's own, as drawn.
    float3 resolved = weighed(finite(drawn.rgb));
    // The history, where taken, this frame's share then, and whether the pixel stood still.
    bool taken = false;
    float3 history = float3(0.0);
    float share = 1.0;
    bool still = false;
    float4 flicker = float4(current.x, 0.0, 0.0, 0.0);
    float2 from = float2(0.0);
    if (resolve.held != 0) {
        // The nearest pixel's point, homogeneous, at infinity for depth 0 under perspective, and
        // where it was before it moved.
        float2 size = float2(resolve.size);
        float2 seen = (float2(near) + 0.5) / size * 2.0 - 1.0;
        float4 world = mul(resolve.world_from_clip, float4(seen, nearest, 1.0));
        float3 moved = textures[resolve.motion].Load(int3(near, 0)).xyz;
        float4 before = mul(resolve.previous_clip_from_world,
                            float4(world.xyz - moved * world.w, world.w));
        // Its motion on screen, from the frame before to this one without its offset: back to
        // where its history is.
        float2 motion = (seen - resolve.jitter) - before.xy / before.w;
        float2 uv = (float2(at) + 0.5) / size - motion * 0.5;
        bool inside = all(uv >= 0.0) && all(uv <= 1.0);
        if (before.w > 0.0 && inside) {
            float3 sampled = catmull_rom(textures[resolve.history], uv, size);
            history = weighed(sampled * resolve.history_scale);
            if (!any(isnan(history))) {
                float pixels = length(motion * size * 0.5);
                taken = true;
                still = pixels <= STILL;
                share = max(lerp(CURRENT_STILL, CURRENT_MOVED, saturate(pixels)),
                            1.0 / (float(resolve.held) + 1.0));
                from = uv;
                float4 left = textures[resolve.flicker].SampleLevel(samplers[LINEAR_CLAMP], uv, 0.0);
                if (!any(isnan(left))) {
                    flicker = left;
                    if (resolve.history_scale != 1.0) {
                        // In this frame's light, as the history: what it tried, and what's
                        // measured from that.
                        float lit = relit(left.x, resolve.history_scale);
                        flicker.x = lit;
                        flicker.y = relit(left.x + left.y, resolve.history_scale) - lit;
                        flicker.z = relit(left.x + left.z, resolve.history_scale) - lit;
                    }
                }
            }
        }
    }
    // What each offset drew: this one's, against the cycle before, where the pixel stood still;
    // where it moved, none of the others', which drew another point. A still pixel this offset
    // draws otherwise changed.
    uint3 to = uint3(pixel.xy, 0);
    float4 first = storage[resolve.seen_first][to];
    float4 second = storage[resolve.seen_second][to];
    float seen[OFFSETS] = { first.x, first.y, first.z, first.w,
                            second.x, second.y, second.z, second.w };
    float drew = max(current.x, 0.0) + KEPT_BLACK;
    bool changed = false;
    if (still) {
        float before = abs(seen[resolve.phase]);
        bool same = abs(before - drew) <= SAME * drew;
        changed = before != 0.0 && !same;
        seen[resolve.phase] = same ? drew : -drew;
    } else {
        for (uint i = 0; i < OFFSETS; i++) {
            seen[i] = 0.0;
        }
        seen[resolve.phase] = -drew;
    }
    storage[resolve.seen_first][to] = float4(seen[0], seen[1], seen[2], seen[3]);
    storage[resolve.seen_second][to] = float4(seen[4], seen[5], seen[6], seen[7]);
    // Whether every offset drew it as a cycle before, and their mean.
    bool repeats = still;
    float mean_drawn = 0.0;
    for (uint i = 0; i < OFFSETS; i++) {
        repeats = repeats && seen[i] > 0.0;
        mean_drawn += seen[i] - KEPT_BLACK;
    }
    mean_drawn /= float(OFFSETS);
    // The flicker test's luminance resolve: what it held, clamped to the 3 by 3 pixels, then
    // `TRIED` of the way to this frame's.
    float tried = clamp(flicker.x, least.x, most.x);
    float push = tried - flicker.x;
    float pull = TRIED * (current.x - tried);
    // How much of the push takes back what the frames' colors put in, which is flicker, and how
    // much doesn't, which is change; neither under `TOLD`. A still pixel that changed doesn't
    // flicker, whatever the clamp does.
    float told = TOLD * (tried + KEPT_BLACK);
    float undone = push * flicker.y < 0.0 ? min(abs(push), abs(flicker.y)) : 0.0;
    float unexplained = abs(push) - undone;
    float evidence = flicker.w * EVIDENCE_FADES;
    if (unexplained > told) {
        evidence *= saturate(1.0 - unexplained / (CHANGE * abs(flicker.z) + told));
    }
    if (undone > told) {
        evidence = min(evidence + EVIDENCE_GAIN * undone / abs(flicker.y), EVIDENCE_MOST);
    }
    if (changed) {
        evidence = 0.0;
    }
    float swing = flicker.z * SWING_FADES;
    if (undone > abs(swing)) {
        swing = sign(push) * undone;
    }
    float unsettled = (flicker.y + sign(push) * undone) * UNSETTLED_FADES + pull;
    storage[resolve.flickers][to] = float4(tried + pull, unsettled, swing, evidence);
    // How far the pixel is taken to flicker, from 0 to 1.
    float flickers = saturate((evidence - FLICKERS_FROM) / (FLICKERS_BY - FLICKERS_FROM));
    if (taken) {
        if (repeats) {
            resolved = float3(lerp(history.x, mean_drawn, CURRENT_STILL),
                              lerp(history.yz, current.yz, CURRENT_REPEATS));
        } else {
            float3 kept = clipped(history, least, most);
            if (flickers > 0.0) {
                // Its luminance, past the 3 by 3 pixels on the side the clamp pushed from, no
                // further than the frames drew. Half Catmull-Rom, half bilinear: unclipped, the
                // sharp filter alone darkens a lone line frame by frame, and bilinear fades it.
                float3 plain = textures[resolve.history]
                    .SampleLevel(samplers[LINEAR_CLAMP], from, 0.0).rgb;
                float smooth = weighed(plain * resolve.history_scale).x;
                float past = abs(swing) / TRIED;
                float wide = clamp(0.5 * (history.x + smooth), least.x - (swing > 0.0 ? past : 0.0),
                                   most.x + (swing < 0.0 ? past : 0.0));
                kept.x = lerp(kept.x, wide, flickers);
            }
            float settled = saturate((evidence - FLICKERS_BY) / (SETTLED_BY - FLICKERS_BY));
            resolved = lerp(kept, current, lerp(share, min(share, CURRENT_FLICKERS), settled));
        }
    }
    // Within a half float: unweighing a saturated color near the weighing's limit can pass it.
    float4 color = float4(finite(unweighed(resolved)), drawn.a);
    storage[resolve.target][to] = color;
    if (resolve.writes_resolved != 0) {
        storage[resolve.resolved][to] = color;
    }
}
