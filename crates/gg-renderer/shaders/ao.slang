// The game's view's ambient occlusion, from its depth: how much of the light from all around reaches
// each pixel's point past what's near it, which the sky's light is darkened by. Ground-truth ambient
// occlusion (Jimenez, Wu, Pesce and Jarabo, "Practical Real-Time Strategies for Accurate Indirect
// Occlusion", 2016), as Intel's XeGTAO computes it, under the MIT license, whose functions these
// follow, with its default settings and its high quality, of 3 slices of 3 steps each way:
// https://github.com/GameTechDev/XeGTAO/blob/master/Source/Rendering/Shaders/XeGTAO.hlsli
//
// In three passes, a pixel a thread: `prefilter_main` takes the view's depth to view-space depth, the
// distance ahead of the view, in levels of half the size each, each texel a mean of the 2 by 2 under
// it weighted toward the farthest; `occlusion_main` searches each pixel's horizons along slices of
// the hemisphere round its normal, found from the depth around it, turned by noise that changes from
// pixel to pixel along a Hilbert curve, and with `Push::noise`, which changes from frame to frame,
// and writes how much of it they leave open and where the depth breaks round it; and the denoise,
// once or more, blurs each pixel with the 8 round it but across those breaks, `denoise_main` from one
// buffer into another and `denoise_last_main` into the view's ambient occlusion. The view's space
// here is XeGTAO's: X right, Y up and Z ahead, so that depth is positive.
//
// The noise is on the screen, where the view moves under it, and blurring leaves blotches of it a few
// pixels across, which neither the blur nor temporal antialiasing, which clips each pixel's history to
// what this frame drew round it, takes away. So `denoise_last_main` also averages each pixel's
// occlusion over the frames before, each with noise of its own, by a history of its own (reverse
// reprojection caching: Nehab, Sander, Lawrence, Tatarchuk and Isidoro, "Accelerating Real-Time
// Shading with Reverse Reprojection Caching", 2007): from where the pixel's point showed in the frame
// before, as the camera moved, unless the depth there is of another point, as what's uncovered or
// moved is; bounded by what the last denoise's own terms round the pixel range over, and a little
// more, so that the occlusion of something that moves leaves little trail; and accumulated for the
// next. What's left of the noise then moves with the world.
//
// `Push` and `Accumulation` match `Push` and `Accumulation` in src/ao.rs field for field.

[[vk::binding(0, 0)]] Texture2D<float> depths[];
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(2, 0)]] [format("r32f")] RWTexture2DArray<float> scalars[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// What the last denoise accumulates by.
struct Accumulation {
    // From the view's space, as `position` has it, to the clip space of the view as the frame before
    // drew it, offset as it was.
    float4x4 previous_clip_from_view;
    // What the frame before accumulated, a texture of the bindless set's, if `taken` isn't 0; and
    // what this frame accumulates, a storage image. Each pixel's occlusion; the depth of its point,
    // as far as a half float holds; and how many frames' occlusion it holds.
    uint history;
    uint accumulated;
    uint taken;
    uint unused;
};

struct Push {
    // The view-space depth's levels, one after another, each a row after another.
    float* levels;
    // Each pixel's occlusion and its edges, packed as `packed` packs them: what the occlusion
    // writes and a denoise reads, and what a denoise before the last writes.
    uint* terms;
    uint* denoised;
    // What takes a point's texture coordinates in the view, from its top left, to its X and Y in
    // the view's space, over its depth: `(view_mul * uv + view_add) * depth`.
    float2 view_mul;
    float2 view_add;
    // What takes the view's depth, `z`, to view-space depth: `depth_mul / (depth_add + z)`.
    float depth_mul;
    float depth_add;
    // The view's size in pixels.
    uint2 size;
    // The two depths whose nearer is what the view shows of what's opaque at each pixel, textures
    // of the bindless set's: the view's, drawn into its visibility buffer, and what's drawn
    // otherwise; or that alone, twice.
    uint2 depths;
    // The view's ambient occlusion, a storage image of the bindless set's.
    uint target;
    // How far occluders reach, in the world's units, and the power the visibility is raised to.
    float radius;
    float power;
    // Which of 64 frames' noise to take.
    uint noise;
    // How much a denoise keeps of each pixel's own term, against its neighbors' weights.
    float blur;
    // Where each level after the first starts among the levels, in texels.
    uint start1;
    uint start2;
    uint start3;
    uint start4;
    uint unused;
    Accumulation* accumulation;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// How many pixels across and down a workgroup of the occlusion or a denoise takes: `GROUP` in
// src/ao.rs. The prefilter's threads take 2 by 2 pixels each.
static const uint GROUP = 8;
// How many levels the view-space depth has: `LEVELS` in src/ao.rs.
static const uint LEVELS = 5;
// The view-space depth that what's infinitely far, as the sky, stands at: past what any sample
// reaches, and far enough short of a float's largest that a point there and the differences
// between points stay finite.
static const float FARTHEST = 1.0e30;

static const float PI = 3.14159265358979;
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
// How many frames' occlusion a history holds at most, so that each frame's makes at least a
// sixteenth of a pixel's: in a view that moves, about the last 31 frames' noise together, a sixth of
// one frame's; and what changes the occlusion settles in some 16 frames.
static const float HELD_MOST = 15.0;
// How far the depth a history keeps may be from that of the point it's taken for, as a share of the
// point's: past what the depth changes to the next pixel along its surface, a fiftieth.
static const float DEPTH_SLACK = 0.02;
// How far a history may lie outside what the last denoise's terms round the pixel range over: some
// 5 of a byte's levels, past the noise two denoises leave, so that a still view's history, which
// averages the frames' noise, isn't clipped back to this frame's; but a change the terms show, as
// occlusion that moved, takes it along. With none, a floor's grain as a 768 by 512 view panned over
// it, without antialiasing, doubled; with 0.05, a sphere rolling across the floor left 5 to 7 times
// the trail, in the GPU test `ambient_occlusion_leaves_no_trail_behind_what_moves`.
static const float HISTORY_MARGIN = 0.02;
// The largest a half float holds, short of infinity: the depth a history keeps of what's farther.
static const float HALF_MOST = 65504.0;

// Level `level`'s size: the view's halved that many times, rounded down, at least a texel.
uint2 level_size(uint level) {
    return max(push.size >> level, uint2(1));
}

// Where level `level` starts among the levels.
uint level_start(uint level) {
    uint4 starts = uint4(push.start1, push.start2, push.start3, push.start4);
    // Indexed within the four, as both sides of the choice may be evaluated.
    return level == 0 ? 0 : starts[max(level, 1) - 1];
}

// The view-space depth at `pixel`: the nearer of the two depths there, but within the view's edges.
float view_depth(int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(push.size) - 1);
    float a = depths[push.depths.x].Load(int3(at, 0));
    float b = depths[push.depths.y].Load(int3(at, 0));
    // Reversed: the nearer is the greater. Where neither drew, it's 0, infinitely far.
    float z = max(a, b);
    return min(push.depth_mul / (push.depth_add + z), FARTHEST);
}

// Writes `depth` at `at` of level `level`, if the level has a texel there.
void write_level(uint level, uint2 at, float depth) {
    uint2 size = level_size(level);
    if (at.x < size.x && at.y < size.y) {
        push.levels[level_start(level) + at.y * size.x + at.x] = depth;
    }
}

// XeGTAO_DepthMIPFilter: the mean of four depths, each weighted by how near it is to the farthest
// of them, so that a coarser level keeps to what's behind a thin occluder rather than average it in.
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
// second, and the means of 2 by 2 of those, by threads in turn, in the three after.
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

// The first level's depth at `pixel`, but within its edges.
float depth_at(int2 pixel) {
    int2 at = clamp(pixel, int2(0), int2(push.size) - 1);
    return push.levels[at.y * push.size.x + at.x];
}

// Level `level`'s depth at `uv`, its nearest texel, but within its edges, as a sampler that takes
// the nearest of each and clamps samples.
float level_at(uint level, float2 uv) {
    uint2 size = level_size(level);
    uint2 texel = min(uint2(max(uv * float2(size), 0.0)), size - 1);
    return push.levels[level_start(level) + texel.y * size.x + texel.x];
}

// XeGTAO_ComputeViewspacePosition: the point at `uv` in the view, `depth` ahead of it.
float3 position(float2 uv, float depth) {
    return float3((push.view_mul * uv + push.view_add) * depth, depth);
}

// XeGTAO_CalculateEdges: how far the depth runs on smoothly from the center to each of the pixels
// left, right, above and below it, from 1 to 0 where it breaks, as a plane sloping through the
// center would carry it, by the depth's own scale.
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

// A term, from 0 to `TERM_SCALE`, which a raw term can pass 1 by and denoised ones average back
// to, over that scale in its low 16 bits; and its edges' byte above them.
uint packed(float term, uint edges) {
    return uint(saturate(term) * 65535.0 + 0.5) | (edges << 16);
}

// XeGTAO_CalculateNormal: the surface's normal at the center, `c`, from the points round it, each
// pair's cross product weighted by how smoothly the depth runs on to both; toward the view where
// none does.
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

// XeGTAO_FastSqrt and XeGTAO_FastACos, which takes -1 to 1 to 0 to π (Drobot, "Low Level
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

// Which of a 64 by 64 tile's pixels `x` and `y` is along a Hilbert curve through it, as XeGTAO's
// `HilbertIndex` has it.
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
// Quasirandom Sequences", 2018) at `pixel`'s place along the Hilbert curve, and 288 on for each of
// the 64 frames' noise, `frame`.
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
    float2 pixel_size = 1.0 / float2(push.size);
    float2 uv = (float2(pixel) + 0.5) * pixel_size;
    float z = depth_at(at);
    float left = depth_at(at + int2(-1, 0));
    float right = depth_at(at + int2(1, 0));
    float top = depth_at(at + int2(0, -1));
    float bottom = depth_at(at + int2(0, 1));
    float4 edges = edges_of(z, left, right, top, bottom);
    uint edge_bits = packed_edges(edges);
    // Where nothing opaque drew, as the sky, nothing occludes; its edges still part it from what's
    // round it.
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
    // How wide a pixel is in the view's space at the center's depth, and the radius in pixels.
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

// The term and edges at `pixel` that a denoise reads, but within the view's edges.
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

// XeGTAO_Denoise, a pixel at a time: the term at `pixel` blurred with the 8 round it, each
// weighted by how smoothly the depth runs on to it, both ways, but for some of the term leaking
// back where three edges or four break; and the pixel's own packed word, whose edges it keeps.
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

// Adds to `sum` and `held` what the frame before accumulated at `texel`, in `history`, by `weight`,
// and that to `weights`, if it's inside the view and its depth is within `slack` of `depth`.
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

// The last denoise, from `terms` into the view's ambient occlusion: the fraction of the light from
// all around that reaches each pixel, from 0 to 1, averaged with what the frame before accumulated
// where the pixel's point showed, and accumulated for the next.
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
    // How many frames' occlusion the history held where the point showed: none where it's taken
    // from nowhere.
    float held = 0.0;
    if (accumulation.taken != 0 && z < FARTHEST) {
        float2 size = float2(push.size);
        float2 uv = (float2(pixel) + 0.5) / size;
        float4 clip = mul(accumulation.previous_clip_from_view, float4(position(uv, z), 1.0));
        if (clip.w > 0.0) {
            // How far the frame before's view saw the point, and how far each of the 4 pixels round
            // where it showed may then have seen theirs: as far, give or take a fiftieth, and what
            // the depth changes to the next pixel each way along the surface, the smaller of the
            // two sides', as across an edge is another surface.
            float across = min(abs(depth_at(at + int2(1, 0)) - z), abs(z - depth_at(at + int2(-1, 0))));
            float down = min(abs(depth_at(at + int2(0, 1)) - z), abs(z - depth_at(at + int2(0, -1))));
            float slack = DEPTH_SLACK * clip.w + across + down;
            float2 texel = (clip.xy / clip.w * 0.5 + 0.5) * size - 0.5;
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
                // Within what the terms round the pixel range over, as this frame's denoised
                // occlusion, their weighted mean, is, widened by `HISTORY_MARGIN`.
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
    scalars[push.target][uint3(pixel, 0)] = occlusion;
    storage[accumulation.accumulated][uint3(pixel, 0)] =
        float4(occlusion, min(z, HALF_MOST), min(held + 1.0, HELD_MOST), 0.0);
}
