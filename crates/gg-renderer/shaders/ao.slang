// The view's ambient occlusion from its depth, which darkens the sky's light: ground-truth ambient
// occlusion (Jimenez, Wu, Pesce and Jarabo, "Practical Real-Time Strategies for Accurate Indirect
// Occlusion", 2016) as Intel's XeGTAO computes it, under the MIT license. Functions follow
// XeGTAO's, with its defaults and high quality, 3 slices of 3 steps each way:
// https://github.com/GameTechDev/XeGTAO/blob/master/Source/Rendering/Shaders/XeGTAO.hlsli
//
// Passes, a pixel a thread: `prefilter_main`, view-space depth's levels; `occlusion_main`, horizons
// along slices of the hemisphere round each normal, turned by noise; then one or more denoises,
// the last `denoise_last_main`. View space is XeGTAO's: X right, Y up and Z ahead, so depth is
// positive.
//
// At half the view's size, each of the occlusion's pixels takes the depth at the center of the
// first of the 2 by 2 view pixels it stands for. `upsample_main` then takes the view's pixels
// from the four nearest, by how near their depths are.
//
// The blur leaves blotches of the screen's noise a few pixels across, which TAA, clipping to this
// frame, doesn't take away. So the last denoise also averages each pixel over frames by a history
// of its own (reverse reprojection caching: Nehab, Sander, Lawrence, Tatarchuk and Isidoro,
// "Accelerating Real-Time Shading with Reverse Reprojection Caching", 2007). It's rejected where
// the depth is another point's, as what's uncovered or moved, and bounded near this frame's terms,
// so moving occlusion leaves little trail. What's left of the noise then moves with the world.
//
// `Push` and `Accumulation` match `Push` and `Accumulation` in src/ao.rs field for field.

#include "common.slang"

[[vk::binding(0, 0)]] Texture2D<float> depths[];
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("r32f")] RWTexture2DArray<float> scalars[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// What the last denoise accumulates by.
struct Accumulation {
    // From view space to the frame before's clip space, offset as it was.
    float4x4 previous_clip_from_view;
    // What the frame before accumulated, a bindless texture, if `taken` isn't 0; what this frame
    // accumulates, a storage image. Per pixel: the occlusion; its point's depth, up to a half
    // float's largest; and how many frames it holds.
    uint history;
    uint accumulated;
    uint taken;
    uint unused;
};

struct Push {
    // The view-space depth's levels, end to end, each row by row.
    float* levels;
    // Each pixel's occlusion and edges, as `packed` packs them: the occlusion's, which a denoise
    // reads, and a denoise's before the last.
    uint* terms;
    uint* denoised;
    // From a point's texture coordinates in the view, from its top left, to its X and Y in view
    // space, over its depth: `(view_mul * uv + view_add) * depth`.
    float2 view_mul;
    float2 view_add;
    // From the view's depth, `z`, to view-space depth: `depth_mul / (depth_add + z)`.
    float depth_mul;
    float depth_add;
    // The occlusion's size and the view's, in pixels.
    uint2 size;
    uint2 view;
    // Bindless textures, two depths whose nearer is what's opaque: the visibility buffer's and
    // what's drawn otherwise, or one twice.
    uint2 depths;
    // The view's ambient occlusion, a bindless storage image. At half size, only the upsample
    // writes it.
    uint target;
    // How far occluders reach, in world units, and the power the visibility is raised to.
    float radius;
    float power;
    // Which of 64 frames' noise to take.
    uint noise;
    // How much a denoise keeps of each pixel's own term, against its neighbors' weights.
    float blur;
    // Where each level after the first starts, in texels.
    uint start1;
    uint start2;
    uint start3;
    uint start4;
    // View pixels across and down per pixel of the occlusion: 1, or 2 at half size.
    uint step;
    // What the last denoise accumulated, a bindless texture, which the upsample reads.
    uint occluded;
    uint unused;
    Accumulation* accumulation;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// Pixels across and down a workgroup of the occlusion or a denoise: `GROUP` in src/ao.rs. The
// prefilter's threads take 2 by 2 pixels each.
static const uint GROUP = 8;
// The view-space depth's levels: `LEVELS` in src/ao.rs.
static const uint LEVELS = 5;

static const float HALF_PI = 1.57079632679490;
// XeGTAO's defaults, by its names: XE_GTAO_DEFAULT_RADIUS_MULTIPLIER,
// XE_GTAO_DEFAULT_FALLOFF_RANGE, XE_GTAO_DEFAULT_SAMPLE_DISTRIBUTION_POWER,
// XE_GTAO_DEFAULT_DEPTH_MIP_SAMPLING_OFFSET and XE_GTAO_OCCLUSION_TERM_SCALE.
static const float RADIUS_MULTIPLIER = 1.457;
static const float FALLOFF_RANGE = 0.615;
static const float SAMPLE_DISTRIBUTION_POWER = 2.0;
static const float MIP_SAMPLING_OFFSET = 3.30;
static const float TERM_SCALE = 1.5;
// Its high quality's slices, and steps each way along each.
static const uint SLICES = 3;
static const uint STEPS = 3;
// How near a sample may be, in pixels, before it's pushed out to that: the pixel itself tells
// nothing.
static const float PIXEL_TOO_CLOSE = 1.3;
// The most frames a history holds: each frame makes at least a sixteenth, so changes settle in
// some 16 frames, and a moving view averages about 31 frames' noise, a sixth of one frame's.
static const float HELD_MOST = 15.0;
// How far, relative, a history's depth may be from its point's: past what the depth changes to the
// next pixel along its surface.
static const float DEPTH_SLACK = 0.02;
// How far a history may stray past the range of the last denoise's terms round the pixel: about 5
// byte levels, past two denoises' noise, so a still view's averaged history isn't clipped back to
// this frame's. With 0, a floor's grain doubled in a 768 by 512 pan without antialiasing; with
// 0.05, a rolling sphere's trail grew 5 to 7 times
// (`ambient_occlusion_leaves_no_trail_behind_what_moves`).
static const float HISTORY_MARGIN = 0.02;
// The largest half float short of infinity: the depth a history keeps of what's farther.
static const float HALF_MOST = 65504.0;
// How far, relative, a neighbor's depth may be from a view pixel's before the upsample weighs it
// less: each neighbor's weight is its bilinear one over the square of this plus that difference.
static const float UPSAMPLE_SLACK = 0.02;

uint2 level_size(uint level) {
    return max(push.size >> level, uint2(1));
}

uint level_start(uint level) {
    uint4 starts = uint4(push.start1, push.start2, push.start3, push.start4);
    // Indexed within the four, as both sides of the choice may be evaluated.
    return level == 0 ? 0 : starts[max(level, 1) - 1];
}

// The view-space depth at the view's pixel `pixel`, clamped to the view: the nearer of the two
// depths.
float depth_in_view(int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(push.view) - 1);
    float a = depths[push.depths.x].Load(int3(at, 0));
    float b = depths[push.depths.y].Load(int3(at, 0));
    // Reversed: the nearer is the greater. Where neither drew, it's 0, infinitely far.
    float z = max(a, b);
    return min(push.depth_mul / (push.depth_add + z), FARTHEST);
}

// The view-space depth the occlusion's `pixel` takes: the view's at the first pixel it stands for.
float view_depth(int2 pixel) {
    return depth_in_view(pixel * int(push.step));
}

// Where the occlusion's `pixel` takes its depth, in texture coordinates from the view's top left.
float2 uv_of(uint2 pixel) {
    return (float2(pixel * push.step) + 0.5) / float2(push.view);
}

void write_level(uint level, uint2 at, float depth) {
    uint2 size = level_size(level);
    if (at.x < size.x && at.y < size.y) {
        push.levels[level_start(level) + at.y * size.x + at.x] = depth;
    }
}

// XeGTAO_DepthMIPFilter: the mean of four depths weighted toward the farthest, so a coarser level
// keeps to what's behind a thin occluder.
float filtered(float d0, float d1, float d2, float d3) {
    float farthest = max(max(d0, d1), max(d2, d3));
    float effect = 0.75 * push.radius * RADIUS_MULTIPLIER;
    float falloff_range = FALLOFF_RANGE * effect;
    float falloff_from = effect * (1.0 - FALLOFF_RANGE);
    float falloff_mul = -1.0 / falloff_range;
    float falloff_add = falloff_from / falloff_range + 1.0;
    float4 d = float4(d0, d1, d2, d3);
    float4 weight = saturate((farthest - d) * falloff_mul + falloff_add);
    return dot(weight, d) / (weight.x + weight.y + weight.z + weight.w);
}

groupshared float scratch[GROUP][GROUP];

// XeGTAO_PrefilterDepths16x16: each thread's 2 by 2 pixels of the first level, their mean in the
// second, and, by threads in turn, the means of 2 by 2 of those in the three after.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void prefilter_main(uint3 thread: SV_DispatchThreadID, uint3 local: SV_GroupThreadID) {
    uint2 base = thread.xy;
    uint2 pixel = base * 2;
    float d0 = view_depth(int2(pixel));
    float d1 = view_depth(int2(pixel) + int2(1, 0));
    float d2 = view_depth(int2(pixel) + int2(0, 1));
    float d3 = view_depth(int2(pixel) + int2(1, 1));
    write_level(0, pixel, d0);
    write_level(0, pixel + uint2(1, 0), d1);
    write_level(0, pixel + uint2(0, 1), d2);
    write_level(0, pixel + uint2(1, 1), d3);
    float second = filtered(d0, d1, d2, d3);
    write_level(1, base, second);
    scratch[local.y][local.x] = second;
    GroupMemoryBarrierWithGroupSync();
    for (uint level = 2; level < LEVELS; level++) {
        uint apart = 1u << (level - 2);
        uint every = apart * 2;
        if (local.x % every == 0 && local.y % every == 0) {
            float coarser = filtered(scratch[local.y][local.x], scratch[local.y][local.x + apart],
                                     scratch[local.y + apart][local.x],
                                     scratch[local.y + apart][local.x + apart]);
            write_level(level, base / every, coarser);
            scratch[local.y][local.x] = coarser;
        }
        GroupMemoryBarrierWithGroupSync();
    }
}

// The first level's depth at `pixel`, clamped to its edges.
float depth_at(int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(push.size) - 1);
    return push.levels[at.y * push.size.x + at.x];
}

// Level `level`'s nearest texel at `uv`, clamped, as a nearest, clamping sampler would take.
float level_at(uint level, float2 uv) {
    uint2 size = level_size(level);
    uint2 texel = min(uint2(max(uv * float2(size), 0.0)), size - 1);
    return push.levels[level_start(level) + texel.y * size.x + texel.x];
}

// XeGTAO_ComputeViewspacePosition: the point at `uv` in the view, `depth` ahead of it.
float3 position(float2 uv, float depth) {
    return float3((push.view_mul * uv + push.view_add) * depth, depth);
}

// XeGTAO_CalculateEdges: how smoothly the depth runs from the center to each side's pixel, 1 to 0
// where it breaks, against a plane sloping through the center, by the depth's own scale.
float4 edges_of(float center, float left, float right, float top, float bottom) {
    float4 edges = float4(left, right, top, bottom) - center;
    float slope_lr = (edges.y - edges.x) * 0.5;
    float slope_tb = (edges.w - edges.z) * 0.5;
    float4 sloped = edges + float4(slope_lr, -slope_lr, slope_tb, -slope_tb);
    edges = min(abs(edges), abs(sloped));
    return saturate(1.25 - edges / (center * 0.011));
}

// XeGTAO_PackEdges: each edge in 2 bits, 0 to 3, the left's highest, in a byte.
uint packed_edges(float4 edges) {
    uint4 steps = uint4(round(saturate(edges) * 2.9));
    return (steps.x << 6) | (steps.y << 4) | (steps.z << 2) | steps.w;
}

// XeGTAO_UnpackEdges.
float4 unpacked_edges(uint edges) {
    uint4 steps = uint4(edges >> 6, edges >> 4, edges >> 2, edges) & 3;
    return float4(steps) / 3.0;
}

// A term from 0 to `TERM_SCALE`, which raw terms can pass 1 by and denoised ones average back to,
// over that scale, in the low 16 bits; its edges' byte above.
uint packed(float term, uint edges) {
    return uint(saturate(term) * 65535.0 + 0.5) | (edges << 16);
}

// XeGTAO_CalculateNormal: the normal at `c` from the points round it, each pair's cross product
// weighted by how smoothly the depth runs to both; toward the view where none does.
float3 normal_of(float4 edges, float3 c, float3 l, float3 r, float3 t, float3 b) {
    float4 accepted = saturate(float4(edges.x * edges.z, edges.z * edges.y, edges.y * edges.w,
                                      edges.w * edges.x) + 0.01);
    l = normalize(l - c);
    r = normalize(r - c);
    t = normalize(t - c);
    b = normalize(b - c);
    float3 normal = accepted.x * cross(l, t) + accepted.y * cross(t, r)
        + accepted.z * cross(r, b) + accepted.w * cross(b, l);
    float length2 = dot(normal, normal);
    return length2 > 1.0e-20 ? normal * rsqrt(length2) : -normalize(c);
}

// XeGTAO_FastSqrt and XeGTAO_FastACos, -1 to 1 onto 0 to π (Drobot, "Low Level
// Optimizations for GCN", 2014; Lagarde, "Inverse trigonometric functions GPU optimization for
// AMD GCN architecture", 2014).
float fast_sqrt(float x) {
    return asfloat(0x1fbd1df5 + (asint(x) >> 1));
}

float fast_acos(float x) {
    float a = abs(x);
    float result = (-0.156583 * a + HALF_PI) * fast_sqrt(1.0 - a);
    return x >= 0.0 ? result : PI - result;
}

// XeGTAO's `HilbertIndex`: pixel `x`, `y`'s place along a Hilbert curve through a 64 by 64 tile.
uint hilbert_index(uint x, uint y) {
    uint index = 0;
    for (uint level = 32; level > 0; level /= 2) {
        uint region_x = (x & level) > 0 ? 1 : 0;
        uint region_y = (y & level) > 0 ? 1 : 0;
        index += level * level * ((3 * region_x) ^ region_y);
        if (region_y == 0) {
            if (region_x == 1) {
                x = 63 - x;
                y = 63 - y;
            }
            uint swapped = x;
            x = y;
            y = swapped;
        }
    }
    return index;
}

// XeGTAO's `SpatioTemporalNoise`: the R2 sequence (Roberts, "The Unreasonable Effectiveness of
// Quasirandom Sequences", 2018) at `pixel`'s place along the Hilbert curve, 288 on for each of the
// 64 frames' noise, `frame`.
float2 noise_at(uint2 pixel, uint frame) {
    uint index = hilbert_index(pixel.x % 64, pixel.y % 64) + 288 * (frame % 64);
    return frac(0.5 + float(index) * float2(0.75487766624669276005, 0.5698402909980532659114));
}

// XeGTAO_MainPass, with its normals generated in place (XE_GTAO_GENERATE_NORMALS_INPLACE) and a
// 32-bit depth (XE_GTAO_FP32_DEPTHS).
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void occlusion_main(uint3 thread: SV_DispatchThreadID) {
    uint2 pixel = thread.xy;
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    uint index = pixel.y * push.size.x + pixel.x;
    int2 at = int2(pixel);
    // The occlusion's pixel in texture coordinates.
    float2 pixel_size = float(push.step) / float2(push.view);
    float2 uv = uv_of(pixel);
    float z = depth_at(at);
    float left = depth_at(at + int2(-1, 0));
    float right = depth_at(at + int2(1, 0));
    float top = depth_at(at + int2(0, -1));
    float bottom = depth_at(at + int2(0, 1));
    float4 edges = edges_of(z, left, right, top, bottom);
    uint edge_bits = packed_edges(edges);
    // Nothing occludes the sky; its edges still part it from what's round it.
    if (z >= FARTHEST) {
        push.terms[index] = packed(1.0 / TERM_SCALE, edge_bits);
        return;
    }
    float3 normal = normal_of(edges, position(uv, z),
                              position(uv + float2(-1.0, 0.0) * pixel_size, left),
                              position(uv + float2(1.0, 0.0) * pixel_size, right),
                              position(uv + float2(0.0, -1.0) * pixel_size, top),
                              position(uv + float2(0.0, 1.0) * pixel_size, bottom));
    // Moved a little toward the view, against the depth's imprecision.
    z *= 0.99999;
    float3 center = position(uv, z);
    float3 view = normalize(-center);
    float effect = push.radius * RADIUS_MULTIPLIER;
    float falloff_range = FALLOFF_RANGE * effect;
    float falloff_from = effect * (1.0 - FALLOFF_RANGE);
    float falloff_mul = -1.0 / falloff_range;
    float falloff_add = falloff_from / falloff_range + 1.0;
    float2 noise = noise_at(pixel, push.noise);
    // A pixel's width in view space at the center's depth, and the radius in pixels.
    float2 pixel_extent = z * push.view_mul * pixel_size;
    float screen_radius = effect / pixel_extent.x;
    // Fading out where the radius spans few pixels.
    float visibility = saturate((10.0 - screen_radius) / 100.0) * 0.5;
    float least = PIXEL_TOO_CLOSE / screen_radius;
    for (uint slice = 0; slice < SLICES; slice++) {
        // Lines 5 to 15 of the paper's Algorithm 1: the slice's way across the view, and the
        // normal projected into its plane, at angle `n` from the way to the view.
        float phi = (float(slice) + noise.x) / float(SLICES) * PI;
        float cos_phi = cos(phi);
        float sin_phi = sin(phi);
        float2 omega = float2(cos_phi, -sin_phi) * screen_radius;
        float3 direction = float3(cos_phi, sin_phi, 0.0);
        float3 ortho_direction = direction - dot(direction, view) * view;
        float3 axis = normalize(cross(ortho_direction, view));
        float3 projected = normal - axis * dot(normal, axis);
        float sign_norm = sign(dot(ortho_direction, projected));
        float projected_length = max(length(projected), 1.0e-20);
        float cos_norm = saturate(dot(projected, view) / projected_length);
        float n = sign_norm * fast_acos(cos_norm);
        // The horizons start below the surface, which samples out of reach fall back to.
        float low0 = cos(n + HALF_PI);
        float low1 = cos(n - HALF_PI);
        float horizon0 = low0;
        float horizon1 = low1;
        for (uint step = 0; step < STEPS; step++) {
            // The R1 sequence along the steps.
            float step_noise = frac(noise.y + float(slice + step * STEPS) * 0.6180339887498948482);
            float s = pow((float(step) + step_noise) / float(STEPS), SAMPLE_DISTRIBUTION_POWER);
            s += least;
            float2 offset = s * omega;
            float level = clamp(log2(length(offset)) - MIP_SAMPLING_OFFSET, 0.0,
                                float(LEVELS - 1));
            uint nearest = uint(level + 0.5);
            // Snapped to a pixel's center.
            offset = round(offset) * pixel_size;
            float2 uv0 = uv + offset;
            float2 uv1 = uv - offset;
            float3 delta0 = position(uv0, level_at(nearest, uv0)) - center;
            float3 delta1 = position(uv1, level_at(nearest, uv1)) - center;
            float distance0 = length(delta0);
            float distance1 = length(delta1);
            // Out of reach, a sample falls back to the low horizon, smoothly over the falloff.
            float weight0 = saturate(distance0 * falloff_mul + falloff_add);
            float weight1 = saturate(distance1 * falloff_mul + falloff_add);
            float cos0 = lerp(low0, dot(delta0 / distance0, view), weight0);
            float cos1 = lerp(low1, dot(delta1 / distance1, view), weight1);
            horizon0 = max(horizon0, cos0);
            horizon1 = max(horizon1, cos1);
        }
        // XeGTAO's fudge against overdarkening on steep slopes.
        projected_length = lerp(projected_length, 1.0, 0.05);
        // The paper's line 27 on: the visible arc between the horizons, cosine-weighted.
        float h0 = -fast_acos(horizon1);
        float h1 = fast_acos(horizon0);
        float arc0 = (cos_norm + 2.0 * h0 * sin(n) - cos(2.0 * h0 - n)) / 4.0;
        float arc1 = (cos_norm + 2.0 * h1 * sin(n) - cos(2.0 * h1 - n)) / 4.0;
        visibility += projected_length * (arc0 + arc1);
    }
    visibility /= float(SLICES);
    visibility = max(pow(max(visibility, 0.0), push.power), 0.03);
    push.terms[index] = packed(visibility / TERM_SCALE, edge_bits);
}

// The packed term a denoise reads at `pixel`, clamped to the view.
uint term_at(int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(push.size) - 1);
    return push.terms[at.y * push.size.x + at.x];
}

float term_of(uint packed) {
    return float(packed & 0xFFFF) / 65535.0;
}

float4 edges_at(uint packed) {
    return unpacked_edges(packed >> 16);
}

// XeGTAO_Denoise, a pixel at a time: the term at `pixel` blurred with the 8 round it, each weighted
// by how smoothly the depth runs to it both ways, but leaking back some where three or four edges
// break. `center`: the pixel's own word, whose edges it keeps.
float denoised_at(int2 pixel, out uint center) {
    center = term_at(pixel);
    uint l = term_at(pixel + int2(-1, 0));
    uint r = term_at(pixel + int2(1, 0));
    uint t = term_at(pixel + int2(0, -1));
    uint b = term_at(pixel + int2(0, 1));
    float4 edges_l = edges_at(l);
    float4 edges_r = edges_at(r);
    float4 edges_t = edges_at(t);
    float4 edges_b = edges_at(b);
    // Kept to what both sides of each edge see, as the edges found from each side can differ.
    float4 edges = edges_at(center) * float4(edges_l.y, edges_r.x, edges_t.w, edges_b.z);
    float leak_threshold = 2.5;
    float leak_strength = 0.5;
    float edginess = saturate(4.0 - leak_threshold - dot(edges, float4(1.0)))
        / (4.0 - leak_threshold) * leak_strength;
    edges = saturate(edges + edginess);
    float diagonal = 0.85 * 0.5;
    float weight_tl = diagonal * (edges.x * edges_l.z + edges.z * edges_t.x);
    float weight_tr = diagonal * (edges.z * edges_t.y + edges.y * edges_r.z);
    float weight_bl = diagonal * (edges.w * edges_b.x + edges.x * edges_l.w);
    float weight_br = diagonal * (edges.y * edges_r.w + edges.w * edges_b.y);
    float sum = term_of(center) * push.blur;
    float weights = push.blur;
    sum += term_of(l) * edges.x + term_of(r) * edges.y + term_of(t) * edges.z
        + term_of(b) * edges.w;
    weights += edges.x + edges.y + edges.z + edges.w;
    sum += term_of(term_at(pixel + int2(-1, -1))) * weight_tl
        + term_of(term_at(pixel + int2(1, -1))) * weight_tr
        + term_of(term_at(pixel + int2(-1, 1))) * weight_bl
        + term_of(term_at(pixel + int2(1, 1))) * weight_br;
    weights += weight_tl + weight_tr + weight_bl + weight_br;
    return sum / weights;
}

// A denoise before the last, from `terms` into `denoised`.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void denoise_main(uint3 thread: SV_DispatchThreadID) {
    uint2 pixel = thread.xy;
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    uint center;
    float term = denoised_at(int2(pixel), center);
    push.denoised[pixel.y * push.size.x + pixel.x] = packed(term, center >> 16);
}

// Adds what the frame before accumulated at `texel` of `history`, by `weight`, to `sum`, `held` and
// `weights`, if it's in the view and its depth is within `slack` of `depth`.
void take(uint history, int2 texel, float weight, float depth, float slack,
          inout float sum, inout float held, inout float weights) {
    if (any(texel < 0) || any(texel >= int2(push.size))) {
        return;
    }
    float4 kept = textures[history].Load(int3(texel, 0));
    if (abs(kept.y - depth) <= slack) {
        sum += kept.x * weight;
        held += kept.z * weight;
        weights += weight;
    }
}

// The last denoise, into the view's ambient occlusion: the fraction of light from all around that
// reaches each pixel, 0 to 1, averaged with the frame before's where the point showed, and
// accumulated for the next.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void denoise_last_main(uint3 thread: SV_DispatchThreadID) {
    // Loaded whole, as the model shaders load the frame's data.
    Accumulation accumulation = *push.accumulation;
    uint2 pixel = thread.xy;
    if (pixel.x >= push.size.x || pixel.y >= push.size.y) {
        return;
    }
    uint center;
    float occlusion = min(denoised_at(int2(pixel), center) * TERM_SCALE, 1.0);
    int2 at = int2(pixel);
    float z = depth_at(at);
    // Frames the history held where the point showed; none where it's taken from nowhere.
    float held = 0.0;
    if (accumulation.taken != 0 && z < FARTHEST) {
        float3 seen = position(uv_of(pixel), z);
        float4 clip = mul(accumulation.previous_clip_from_view, float4(seen, 1.0));
        if (clip.w > 0.0) {
            // The frame before's depth of the point, and the slack for each of the 4 pixels round
            // where it showed: `DEPTH_SLACK`, plus the depth's change to the next pixel each way
            // along the surface, the smaller side's, as across an edge is another surface.
            float across = min(abs(depth_at(at + int2(1, 0)) - z), abs(z - depth_at(at + int2(-1, 0))));
            float down = min(abs(depth_at(at + int2(0, 1)) - z), abs(z - depth_at(at + int2(0, -1))));
            float slack = DEPTH_SLACK * clip.w + across + down;
            // Each history texel holds the occlusion's pixel that takes its depth there, as `uv_of`.
            float2 texel = ((clip.xy / clip.w * 0.5 + 0.5) * float2(push.view) - 0.5)
                / float(push.step);
            float2 floored = floor(texel);
            float2 f = texel - floored;
            float2 g = 1.0 - f;
            int2 corner = int2(floored);
            float sum = 0.0;
            float held_sum = 0.0;
            float weights = 0.0;
            take(accumulation.history, corner, g.x * g.y, clip.w, slack, sum, held_sum, weights);
            take(accumulation.history, corner + int2(1, 0), f.x * g.y, clip.w, slack, sum, held_sum, weights);
            take(accumulation.history, corner + int2(0, 1), g.x * f.y, clip.w, slack, sum, held_sum, weights);
            take(accumulation.history, corner + int2(1, 1), f.x * f.y, clip.w, slack, sum, held_sum, weights);
            if (weights > 0.0) {
                // Within the range of the terms round the pixel, which this frame's denoised
                // occlusion averages, widened by `HISTORY_MARGIN`.
                float least = 1.0;
                float most = 0.0;
                for (int y = -1; y <= 1; y++) {
                    for (int x = -1; x <= 1; x++) {
                        float near = min(term_of(term_at(at + int2(x, y))) * TERM_SCALE, 1.0);
                        least = min(least, near);
                        most = max(most, near);
                    }
                }
                float past = clamp(sum / weights, least - HISTORY_MARGIN, most + HISTORY_MARGIN);
                held = held_sum / weights;
                occlusion = lerp(past, occlusion, 1.0 / (held + 1.0));
            }
        }
    }
    if (push.step == 1) {
        scalars[push.target][uint3(pixel, 0)] = occlusion;
    }
    storage[accumulation.accumulated][uint3(pixel, 0)] =
        float4(occlusion, min(z, HALF_MOST), min(held + 1.0, HELD_MOST), 0.0);
}

// At half size, the view's ambient occlusion from what the last denoise accumulated: the four
// pixels of it round each of the view's, weighted bilinearly and by how near their depths are to
// its own, so what's behind an edge doesn't darken what's before it, or the reverse.
[shader("compute")]
[numthreads(GROUP, GROUP, 1)]
void upsample_main(uint3 thread: SV_DispatchThreadID) {
    uint2 pixel = thread.xy;
    if (pixel.x >= push.view.x || pixel.y >= push.view.y) {
        return;
    }
    // As the history keeps it.
    float z = min(depth_in_view(int2(pixel)), HALF_MOST);
    // The occlusion's pixel `p` took its depth at the view's `p * step`.
    float2 at = float2(pixel) / float(push.step);
    float2 floored = floor(at);
    float2 f = at - floored;
    int2 corner = int2(floored);
    float sum = 0.0;
    float weights = 0.0;
    for (uint i = 0; i < 4; i++) {
        int2 offset = int2(i & 1, i >> 1);
        float2 along = lerp(1.0 - f, f, float2(offset));
        int2 texel = min(corner + offset, int2(push.size) - 1);
        float2 kept = textures[push.occluded].Load(int3(texel, 0)).xy;
        float apart = UPSAMPLE_SLACK + abs(kept.y - z) / z;
        float weight = along.x * along.y / (apart * apart);
        sum += kept.x * weight;
        weights += weight;
    }
    // The nearest neighbor's bilinear weight is a quarter at least.
    scalars[push.target][uint3(pixel, 0)] = sum / weights;
}
