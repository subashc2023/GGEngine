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
// pixel. This frame's color is the 3 by 3 pixels' too, each weighed by how far from the pixel's
// center the frame's offset drew it, rather than the pixel's own alone, which is drawn up to half a
// pixel off; and less of it is blended in where the pixel stays where it was (Karis, as above): a
// still view's pixels otherwise follow each frame's offset, by the newest's share of them, and
// never settle. While the history holds fewer frames than that share stands for, this frame's takes
// a share equal to each of theirs, so that a view's first frames average as fast as they come. The
// history is scaled by how the exposure changed since the frame before, as an adapting one does,
// so that what it holds is of this frame's light. Where the frame before resolved nothing, or the
// point showed outside it or behind its camera, the pixel takes this frame's alone.
//
// The clip throws the history out wherever this frame's pixels round it don't hold it, and so in
// a still view it throws out what's thinner than a pixel, or cut out by a mask near its cutoff, at
// each offset that misses it, and the pixel shows each offset's own color in turn: it flickers,
// as it wouldn't without antialiasing. So each pixel keeps the luminance each offset drew of it the
// last time the offset came round, and whether that matched the time before. A pixel that stood
// still while each of the 8 offsets drew it as it had a cycle before is changing only as the
// offsets move what's drawn: its history isn't clipped, and its luminance settles on the mean of
// the 8, which is what blending them converges to, without the ripple that each offset's share of
// a blend leaves. It settles by a still pixel's share of the way each frame rather than at once,
// which jumped from wherever the clip had left it, hundreds of pixels together, as the store came
// to hold two cycles after the view stopped. Anything else that changes it, as something passing
// over it, a shadow too, or light that changes, breaks the match for two cycles, which the clip
// resolves as it otherwise does, so that it leaves no trail. FSR 2 keeps the history unclipped
// where a pixel's luminance returns to what it was (`ffx_fsr2_accumulate.h`, `RectifyHistory`),
// and Unreal's TSR where it flickers within a bound: this test asks more, the same luminance at the
// same offset, so that no change but the offsets' is kept.
//
// A view that moves has no cycle to match: each frame draws each point at another place in its
// pixel, and what's thinner than a pixel shows as dashes, where a frame's pixels land on it, that
// run along it many times as fast as the view moves, each thrown out by the clip the frame after.
// So beside the real resolve each pixel tries one of its luminance alone, kept with the history
// and taken where it is: clamped to this frame's pixels round it, then moved a quarter of the way
// to this frame's own. Where that clamp keeps taking back what the frames' own colors put in, the
// pixel flickers: it's drawn, and then nothing round it holds what was drawn. A change is told
// from that by the way the clamp pushes: what arrives, leaves or is lit otherwise pushes the way
// the frames' colors pull already, or where they pulled nothing, which ends the evidence, so that
// it leaves no more of a trail than without the test; an edge isn't pushed at all, as the pixels
// round it hold both its sides; and a pixel that stands still and is drawn by an offset otherwise
// than a cycle before, which the offsets alone don't do, changes, whatever the clamp does: what
// blinks before a still view shows as each frame draws it. A pixel taken to flicker keeps its
// history's luminance past the pixels round it, on the side the clamp pushed it from and as far
// as the frames drew, and takes less of each frame: it shows the mean of what the frames draw, as
// it does at rest, rather than each frame's in turn, as Unreal's TSR does where luminance
// flickers (`r.TSR.ShadingRejection.Flickering`). Its chroma is clipped as any pixel's. The test
// costs what such tests do: behind what crosses such pixels the evidence starts again, and they
// show as without the test for some tens of frames, until it's back; and what's drawn and gone
// again faster than every 20 frames, while it or the view moves, is taken to flicker and shows
// its mean. What's a pixel wide or more, which the pixels round it always hold, isn't taken to
// flicker, and ripples by a moving pixel's share as it did.
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

// How much of each pixel this frame's color makes, the rest the history's, once the history holds
// as many frames: of a pixel that stays where it was, as much as the last 25 frames' or so
// together, three cycles of the offsets, of which the newest is then a small share; and of one
// that moved a pixel or more, whose history blurs as it's sampled between pixels, as much as the
// last 10's.
static const float CURRENT_STILL = 0.04;
static const float CURRENT_MOVED = 0.1;
// Of the color of a pixel whose offsets each drew it as they did a cycle before, whose luminance
// settles on the mean of what they drew by `CURRENT_STILL`: a hundredth, whose ripple as the
// offsets come round is a quarter of the still share's, as it settles over a few hundred frames
// rather than a hundred.
static const float CURRENT_REPEATS = 0.01;

// How sharply a pixel's weight in this frame's color falls with the square of how far from the
// center, in pixels, it was drawn: a Gaussian of a deviation of 0.35 pixels, about a pixel's box's.
static const float FALLOFF = 4.0;

// How many of the neighborhood's deviations from its mean the history may stray, each way.
static const float SPREAD = 1.25;

// How many offsets the view cycles through: `OFFSETS` in src/taa.rs.
static const uint OFFSETS = 8;

// What's added to the luminance an offset drew, as `weighed` has it, as it's kept: the least
// normal half float, so that what's kept of black isn't 0, which marks none kept.
static const float KEPT_BLACK = 1.0 / 16384.0;

// How near what an offset draws must come to what it drew a cycle before to count as the same, of
// what it draws: two steps of the half floats that keep it, as a still view's offsets draw exactly
// as they did.
static const float SAME = 2.0 / 1024.0;

// How far, in pixels, a pixel may have moved since the frame before and still stand still: what
// reprojecting a still view's points rounds to.
static const float STILL = 0.01;

// How much of each pixel this frame's luminance makes of the resolve the flicker test tries: a
// quarter, more than the real resolve takes, so that what one frame puts in is plain to measure
// as the next frame's clamp takes it back.
static const float TRIED = 0.25;

// The least of a clamp's push that tells anything, of the luminance pushed: 2%, under which a
// push neither counts as flicker nor as change.
static const float TOLD = 0.02;

// The evidence that a pixel flickers: what a push that takes back all that the frames put in
// adds, 0.3; what's left of it a frame later, so that it halves in 23 frames without another;
// and the most it comes to. A pixel is taken to flicker from 0.35, more than one such push
// leaves, so that what passes over it once, and is clamped away once, changes nothing; and
// wholly by 0.75, three close together. What's drawn and gone again every 20 frames or more
// stays under that: at 0.95 a frame it does every 10, but slats 2.5 pixels apart, whose pixels
// mostly hold a slat's dashes, then flicker half as much as without the test at some pans,
// rather than a quarter; and at 0.985 what blinks every 20 frames is taken to flicker.
static const float EVIDENCE_GAIN = 0.3;
static const float EVIDENCE_FADES = 0.97;
static const float EVIDENCE_MOST = 2.0;
static const float FLICKERS_FROM = 0.35;
static const float FLICKERS_BY = 0.75;

// How many times the swing a push that the frames' colors don't explain comes to as it ends the
// evidence; a smaller one cuts it in proportion.
static const float CHANGE = 1.5;

// What's left a frame later of the swing, the largest push that took back what the frames put
// in, which halves in 23 frames too, and of what they put in that no push has taken back yet,
// which halves in 34: where the pixels round a line mostly hold it, the push comes many frames
// after.
static const float SWING_FADES = 0.97;
static const float UNSETTLED_FADES = 0.98;

// How much of a pixel that flickers this frame's color makes, once the evidence has come to
// `SETTLED_BY`: as much as the last 33 frames' or so, a third of the ripple a moving pixel's
// share leaves as its line is drawn or missed. Until then it takes the share it would, which
// brings its history from what the clip left to what the frames draw in a few frames. The
// evidence's most is well past `SETTLED_BY`, so that a pixel that keeps flickering keeps one
// share: the evidence rises on the frames that miss, and a share that followed it would weigh
// the frames that hit more, and darken the pixel.
static const float CURRENT_FLICKERS = 0.03;
static const float SETTLED_BY = 1.2;

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
    // How many frames in a row the history holds, 0 where there's none to take, and whether the
    // resolved colors are written apart.
    uint held;
    uint writes_resolved;
    // What the history is multiplied by: this frame's exposure over the frame before's, which an
    // adapting exposure changes, so that it's of the same light as this frame's colors.
    float history_scale;
    // What each offset drew of each pixel, the last time it came round, storage images of the
    // bindless set's that the resolve reads and writes in place: the first four offsets' in the
    // first, the last four's in the second, each the luminance of this frame's color as `weighed`
    // has it, plus `KEPT_BLACK`, negative where it differed from what the offset drew a cycle
    // before, and 0 where none was kept.
    uint seen_first;
    uint seen_second;
    // Which of the offsets this frame draws at, from 0.
    uint phase;
    // What the flicker test keeps of each pixel: what the frame before left, a texture of the
    // bindless set's, taken bilinearly where the history is, and what this frame leaves, a storage
    // image. Red, the luminance its tried resolve holds, as `weighed` has it; green, how far the
    // frames' own colors have moved that which no clamp has taken back yet; blue, the swing, the
    // largest push that took such a move back, of late, signed as the push; alpha, the evidence.
    uint flicker;
    uint flickers;
    uint2 unused;
};

struct Push {
    Resolve* resolve;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;

// `color`, as the view's colors hold it, as light: none where it's no number, and the largest a
// half float holds where it's more, which there is infinite, as a shader's floats can be, and as
// one past that largest is written where the GPU rounds to the nearest. Told by its bits, which a
// driver may not fold away, as it may `isnan` where it assumes, as Vulkan lets it, that no float
// is NaN or infinite.
float3 finite(float3 color) {
    bool3 nan = (asuint(color) & 0x7FFFFFFF) > 0x7F800000;
    return clamp(select(nan, float3(0.0), color), 0.0, BRIGHTEST);
}

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

// A gray's luminance as `weighed` has it, of light `scale` times as bright.
float relit(float weighed, float scale) {
    float light = max(weighed, 0.0) / max(1.0 - weighed, 1.0 / 65504.0) * scale;
    return light / (1.0 + light);
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
    // The 3 by 3 pixels round it: their mean and deviation, and their least and most, weighed;
    // this frame's color, each weighed again by how far from the pixel's center it was drawn, the
    // frame's offset back from its own center; and the nearest of them, whose depth is the most, as
    // depth runs from 1 at the near plane.
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
    // The history, where it's taken, and this frame's share of the pixel then; and whether the
    // pixel stood still since the frame before.
    bool taken = false;
    float3 history = float3(0.0);
    float share = 1.0;
    bool still = false;
    float4 flicker = float4(current.x, 0.0, 0.0, 0.0);
    float2 from = float2(0.0);
    if (resolve.held != 0) {
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
                        // Of this frame's light, as the history is: what it tried, and what's
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
    // What each offset drew of the pixel: this frame's offset's, against the cycle before, where
    // the pixel stood still, and none of any other where it didn't, as they drew another point.
    // Where it stood still and the offset draws it otherwise than it did, the pixel itself changed.
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
    // Whether every offset drew the pixel as it did a cycle before, and the mean of what they drew.
    bool repeats = still;
    float mean_drawn = 0.0;
    for (uint i = 0; i < OFFSETS; i++) {
        repeats = repeats && seen[i] > 0.0;
        mean_drawn += seen[i] - KEPT_BLACK;
    }
    mean_drawn /= float(OFFSETS);
    // The flicker test's resolve of the luminance alone, tried beside the real one: what it held,
    // clamped to this frame's pixels round it, and then `TRIED` of the way to this frame's.
    float tried = clamp(flicker.x, least.x, most.x);
    float push = tried - flicker.x;
    float pull = TRIED * (current.x - tried);
    // How much of the clamp's push takes back what the frames' own colors put in, which is what
    // flickering is, and how much doesn't, which is the view changing; neither where the push is
    // too small to tell. A pixel that changed where it stood still, which the offsets alone
    // don't do, doesn't flicker either, whatever the clamp does.
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
                // Its luminance, past the pixels round it on the side the clamp pushed it from,
                // and no further than the frames drew: taken half by the sharp filter and half
                // bilinearly, as the sharp one alone, unclipped, darkens a lone line frame by
                // frame, and a bilinear one alone fades it.
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
