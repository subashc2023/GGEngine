// The sky's prefiltering, in compute passes, as Khronos's glTF Sample Renderer prefilters an
// environment (source/shaders/ibl_filtering.frag, at commit df3e51cb):
// - an image sky's sun, found by contrast with the sky round it, which lights as a light;
// - the sky's equirectangular image into a cube, each level averaging the one before;
// - from that cube, Lambert's diffuse light, cosine-weighted, and GGX's specular at each level, a
//   step of roughness each. Both importance sample the cube's levels by the solid angle a sample
//   covers (Křivánek and Colbert, "Real-time Shading with Filtered Importance Sampling", 2008).
// - the split sum's table of GGX's reflectance as F0's scale and bias, with Smith's
//   height-correlated visibility: view cosine across, roughness down (Karis, "Real Shading in
//   Unreal Engine 4", 2013).
//
// `Push` matches `Push` in src/sky.rs field for field.

#include "equirect.slang"

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(0, 0)]] TextureCube cubes[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

// An image sky's sun, as `sun_main` finds it: `SUN_BYTES` in src/sky.rs.
struct Sun {
    // The way to its brightest texel, round which its texels are left out; 1 if a sun was found.
    float3 peak;
    uint found;
    // Its way: its texels' ways, weighted by what each adds over the sky round it.
    float3 way;
    // The median luminance of the sky round it, in the sky's units: what its texels are kept to.
    float median;
    // What its texels add over that, times their solid angles: its illuminance facing it, in the
    // sky's units.
    float3 color;
    float unused;
};

struct Push {
    // The image sky's sun, which `sun_main` writes and `cube_main` reads.
    Sun* sun;
    // The size read, in texels: the sky's width and height, or a cube's first level's side, twice.
    uint2 source_size;
    // What's read: the sky's texture, the cube's, or a level's storage slot.
    uint source;
    // The written level's storage slot.
    uint target;
    // The written level's side, in texels.
    uint size;
    // The written specular level's roughness.
    float roughness;
    // Samples per texel.
    uint samples;
    uint unused;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// A sun is a texel at least `CONTRAST` times the median luminance of the sky between `RING_NEAR`
// and `RING_FAR` round it: a clear noon sun is tens of thousands of times its sky, a cloud's lit
// edge under ten. It's what's past that median within `SUN_REACH` of it, which holds Bistro's sky's
// sun but for 1.5% of its light, spread round it. A sky too coarse to have texels in that ring has
// no sun told from it.
static const float CONTRAST = 100.0;
static const float SUN_REACH = 4.0 * PI / 180.0;
static const float RING_NEAR = 6.0 * PI / 180.0;
static const float RING_FAR = 12.0 * PI / 180.0;

// Threads in the workgroup that finds the sun, and bins of its ring's histogram of log2
// luminance, in the sky's units: `RING_PER_STOP` a stop from 2^`RING_LOWEST`, the first holding
// anything darker, to past a half float's largest.
static const uint SUN_GROUP = 256;
static const float RING_LOWEST = -16.0;
static const float RING_PER_STOP = 8.0;

// The way texel `at` of an equirectangular image `size` texels across and down stands for, at its
// center, and the solid angle it covers, as `equirect` maps ways.
float4 texel_way(uint2 at, uint2 size) {
    float2 uv = (float2(at) + 0.5) / float2(size);
    float across = 2.0 * PI * (uv.x - 0.5);
    float down = PI * uv.y;
    float3 way = float3(sin(down) * cos(across), cos(down), sin(down) * sin(across));
    return float4(way, 2.0 * PI * PI / float(size.x * size.y) * sin(down));
}

// The rows of an equirectangular image `height` texels down within `reach` radians of `way`: the
// first, and one past the last.
uint2 rows_near(float3 way, float reach, uint height) {
    float down = acos(clamp(way.y, -1.0, 1.0));
    float first = floor((down - reach) / PI * float(height));
    float last = ceil((down + reach) / PI * float(height));
    return uint2(clamp(first, 0.0, float(height)), clamp(last, 0.0, float(height)));
}

// The sky's texel `at`, as light.
float3 sky_texel(uint2 at) {
    return finite(textures[push.source].Load(int3(int2(at), 0)).rgb);
}

groupshared float brightest[SUN_GROUP];
groupshared uint brightest_at[SUN_GROUP];
groupshared uint ring[SUN_GROUP];
groupshared float ring_median;
groupshared float3 added[SUN_GROUP];
groupshared float3 weighted[SUN_GROUP];

// The image sky's sun, by one workgroup, so its sums come out alike from run to run: its brightest
// texel, the median of the sky round it, and, if that texel is a sun's, what the texels near it add
// over that median. A scan of every texel, then of the rows near it: 0.3 ms for a 1K sky on an RTX
// 4090. Past 4K, the scan would take a workgroup's partial result each, then a reduce.
[shader("compute")]
[numthreads(SUN_GROUP, 1, 1)]
void sun_main(uint3 id: SV_DispatchThreadID) {
    uint thread = id.x;
    uint2 size = push.source_size;
    uint texels = size.x * size.y;
    // The brightest texel, the first among equals.
    float most = -1.0;
    uint most_at = 0;
    for (uint i = thread; i < texels; i += SUN_GROUP) {
        float bright = luminance(sky_texel(uint2(i % size.x, i / size.x)));
        if (bright > most) {
            most = bright;
            most_at = i;
        }
    }
    brightest[thread] = most;
    brightest_at[thread] = most_at;
    ring[thread] = 0;
    if (thread == 0) {
        ring_median = 0.0;
    }
    GroupMemoryBarrierWithGroupSync();
    for (uint apart = SUN_GROUP / 2; apart > 0; apart >>= 1) {
        if (thread < apart) {
            float other = brightest[thread + apart];
            uint other_at = brightest_at[thread + apart];
            if (other > brightest[thread]
                || (other == brightest[thread] && other_at < brightest_at[thread])) {
                brightest[thread] = other;
                brightest_at[thread] = other_at;
            }
        }
        GroupMemoryBarrierWithGroupSync();
    }
    most = brightest[0];
    most_at = brightest_at[0];
    float3 peak = texel_way(uint2(most_at % size.x, most_at / size.x), size).xyz;
    // The sky round it, into the histogram.
    uint2 rows = rows_near(peak, RING_FAR, size.y);
    uint near = (rows.y - rows.x) * size.x;
    for (uint i = thread; i < near; i += SUN_GROUP) {
        uint2 at = uint2(i % size.x, rows.x + i / size.x);
        float cosine = dot(texel_way(at, size).xyz, peak);
        if (cosine <= cos(RING_NEAR) && cosine >= cos(RING_FAR)) {
            // log2 of 0 is minus infinity, into the first bin.
            float stops = log2(luminance(sky_texel(at))) - RING_LOWEST;
            uint bin = uint(clamp(stops * RING_PER_STOP, 0.0, float(SUN_GROUP - 1)));
            InterlockedAdd(ring[bin], 1);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    // A Hillis-Steele inclusive scan of the counts, as exposure.slang's: the rank past each bin's
    // last texel. The bin holding the middle rank gives the median, at its middle; the first, 0.
    uint count = ring[thread];
    for (uint offset = 1; offset < SUN_GROUP; offset <<= 1) {
        uint before = thread >= offset ? ring[thread - offset] : 0;
        GroupMemoryBarrierWithGroupSync();
        ring[thread] += before;
        GroupMemoryBarrierWithGroupSync();
    }
    uint ringed = ring[SUN_GROUP - 1];
    uint middle = ringed / 2;
    uint end = ring[thread];
    if (end - count <= middle && middle < end) {
        float stops = RING_LOWEST + (float(thread) + 0.5) / RING_PER_STOP;
        ring_median = thread == 0 ? 0.0 : exp2(stops);
    }
    GroupMemoryBarrierWithGroupSync();
    float median = ring_median;
    // What the texels near it add over the sky round it, if it's a sun: alike for every thread.
    bool found = ringed > 0 && most > 0.0 && most >= CONTRAST * median;
    float3 adds = float3(0.0);
    float3 ways = float3(0.0);
    if (found) {
        rows = rows_near(peak, SUN_REACH, size.y);
        near = (rows.y - rows.x) * size.x;
        for (uint i = thread; i < near; i += SUN_GROUP) {
            uint2 at = uint2(i % size.x, rows.x + i / size.x);
            float4 texel = texel_way(at, size);
            float3 color = sky_texel(at);
            float bright = luminance(color);
            if (dot(texel.xyz, peak) >= cos(SUN_REACH) && bright > median) {
                float3 excess = color * (1.0 - median / bright) * texel.w;
                adds += excess;
                ways += texel.xyz * luminance(excess);
            }
        }
    }
    added[thread] = adds;
    weighted[thread] = ways;
    GroupMemoryBarrierWithGroupSync();
    for (uint apart = SUN_GROUP / 2; apart > 0; apart >>= 1) {
        if (thread < apart) {
            added[thread] += added[thread + apart];
            weighted[thread] += weighted[thread + apart];
        }
        GroupMemoryBarrierWithGroupSync();
    }
    if (thread == 0) {
        float3 way = weighted[0];
        Sun sun;
        sun.peak = peak;
        sun.found = found ? 1 : 0;
        sun.way = dot(way, way) > 0.0 ? normalize(way) : peak;
        sun.median = median;
        sun.color = added[0];
        sun.unused = 0.0;
        push.sun[0] = sun;
    }
}

// The sky into a cube's first level: each texel the mean of four points of the image across it.
// It leaves out a found sun, which lights as a light: each point within `SUN_REACH` of it is kept
// to the sky's median round it, keeping its hue, as `sun_main` summed what's past that.
[shader("compute")]
[numthreads(8, 8, 1)]
void cube_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    Texture2D sky = textures[push.source];
    SamplerState repeat = samplers[LINEAR_REPEAT];
    Sun sun = push.sun[0];
    bool leaves = sun.found != 0;
    float3 sum = float3(0.0);
    for (uint i = 0; i < 4; i++) {
        float2 at = float2(id.xy) + float2(0.25 + 0.5 * (i & 1), 0.25 + 0.5 * (i >> 1));
        float3 direction = cube_direction(id.z, at, float(push.size));
        float3 color = equirect(sky, repeat, float2(push.source_size), direction);
        float bright = luminance(color);
        if (leaves && dot(direction, sun.peak) >= cos(SUN_REACH) && bright > sun.median) {
            color *= sun.median / bright;
        }
        sum += color;
    }
    storage[push.target][id] = float4(sum / 4.0, 1.0);
}

// A cube's next level, each texel the mean of the four it covers.
[shader("compute")]
[numthreads(8, 8, 1)]
void downsample_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    RWTexture2DArray<float4> above = storage[push.source];
    uint3 at = uint3(id.xy * 2, id.z);
    float4 sum = above[at] + above[at + uint3(1, 0, 0)] + above[at + uint3(0, 1, 0)]
        + above[at + uint3(1, 1, 0)];
    storage[push.target][id] = float4(sum.rgb / 4.0, 1.0);
}

// The `i`th of `count` Hammersley points in the unit square.
float2 hammersley(uint i, uint count) {
    return float2(float(i) / float(count), float(reversebits(i)) * 2.3283064365386963e-10);
}

// An orthonormal frame whose third vector is `normal`, as columns.
float3x3 frame_of(float3 normal) {
    float3 up = abs(normal.y) < 0.9999999 ? float3(0.0, 1.0, 0.0)
                                          : float3(0.0, 0.0, normal.y > 0.0 ? 1.0 : -1.0);
    float3 tangent = normalize(cross(up, normal));
    float3 bitangent = cross(normal, tangent);
    return float3x3(tangent, bitangent, normal);
}

// `local`, a way in `frame`'s space, in the world's.
float3 in_world(float3x3 frame, float3 local) {
    return frame[0] * local.x + frame[1] * local.y + frame[2] * local.z;
}

// The cube level for one of `count` samples of density `pdf` over solid angle: the level whose
// texels cover as much as the sample, by Křivánek and Colbert's formula for cubes, as the Sample
// Renderer has it.
float level_of(float pdf, uint count) {
    float side = float(push.source_size.x);
    return max(0.5 * log2(6.0 * side * side / (float(count) * pdf)), 0.0);
}

// The cosine-weighted mean of the cube's light round each texel's way: irradiance over π, which
// Lambert's diffuse multiplies by the base color.
[shader("compute")]
[numthreads(8, 8, 1)]
void irradiance_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    TextureCube cube = cubes[push.source];
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 normal = cube_direction(id.z, float2(id.xy) + 0.5, float(push.size));
    float3x3 frame = frame_of(normal);
    float3 sum = float3(0.0);
    for (uint i = 0; i < push.samples; i++) {
        float2 xi = hammersley(i, push.samples);
        float cosine = sqrt(1.0 - xi.y);
        float sine = sqrt(xi.y);
        float phi = 2.0 * PI * xi.x;
        float3 way = in_world(frame, float3(sine * cos(phi), sine * sin(phi), cosine));
        float pdf = cosine / PI;
        sum += cube.SampleLevel(clamp_, way, level_of(pdf, push.samples)).rgb;
    }
    storage[push.target][id] = float4(sum / float(push.samples), 1.0);
}

// GGX's distribution; `alpha` is the roughness squared.
float ggx(float n_dot_h, float alpha) {
    float a = n_dot_h * alpha;
    float k = alpha / (1.0 - n_dot_h * n_dot_h + a * a);
    return k * k / PI;
}

// The `i`th of `count` halfway vectors sampling GGX's distribution, in its frame's space, and its
// density over the reflected way's solid angle, seen straight on, the view being the normal.
float4 ggx_sample(uint i, uint count, float alpha) {
    float2 xi = hammersley(i, count);
    float cosine = saturate(sqrt((1.0 - xi.y) / (1.0 + (alpha * alpha - 1.0) * xi.y)));
    float sine = sqrt(1.0 - cosine * cosine);
    float phi = 2.0 * PI * xi.x;
    return float4(sine * cos(phi), sine * sin(phi), cosine, ggx(cosine, alpha) / 4.0);
}

// The cube's light round each texel's way, weighted by GGX at the level's roughness, seen straight
// on, as the split sum takes it. At roughness 0, the cube's level of the texel's size.
[shader("compute")]
[numthreads(8, 8, 1)]
void specular_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    TextureCube cube = cubes[push.source];
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 normal = cube_direction(id.z, float2(id.xy) + 0.5, float(push.size));
    float3 color;
    if (push.roughness == 0.0) {
        float level = log2(float(push.source_size.x) / float(push.size));
        color = cube.SampleLevel(clamp_, normal, level).rgb;
    } else {
        float3x3 frame = frame_of(normal);
        float alpha = push.roughness * push.roughness;
        float3 sum = float3(0.0);
        float weight = 0.0;
        for (uint i = 0; i < push.samples; i++) {
            float4 halfway = ggx_sample(i, push.samples, alpha);
            float3 h = in_world(frame, halfway.xyz);
            float3 way = reflect(-normal, h);
            float n_dot_l = dot(normal, way);
            if (n_dot_l > 0.0) {
                float level = level_of(halfway.w, push.samples);
                sum += cube.SampleLevel(clamp_, way, level).rgb * n_dot_l;
                weight += n_dot_l;
            }
        }
        color = sum / weight;
    }
    storage[push.target][id] = float4(color, 1.0);
}

// Smith's height-correlated visibility for GGX; `alpha` is the roughness squared.
float visibility(float n_dot_v, float n_dot_l, float alpha) {
    float alpha2 = alpha * alpha;
    float from_view = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2);
    float from_light = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2);
    return 0.5 / (from_view + from_light);
}

// The split sum's table, view cosine across and roughness down, at texel centers: GGX's
// reflectance as F0's scale in red, its bias in green.
[shader("compute")]
[numthreads(8, 8, 1)]
void brdf_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    float n_dot_v = (float(id.x) + 0.5) / float(push.size);
    float roughness = (float(id.y) + 0.5) / float(push.size);
    float alpha = roughness * roughness;
    float3 view = float3(sqrt(1.0 - n_dot_v * n_dot_v), 0.0, n_dot_v);
    float scale = 0.0;
    float bias = 0.0;
    for (uint i = 0; i < push.samples; i++) {
        float3 h = ggx_sample(i, push.samples, alpha).xyz;
        float3 light = reflect(-view, h);
        float n_dot_l = saturate(light.z);
        float n_dot_h = saturate(h.z);
        float v_dot_h = saturate(dot(view, h));
        if (n_dot_l > 0.0) {
            // GGX's distribution cancels against the sample's density.
            float weighted = visibility(n_dot_v, n_dot_l, alpha) * v_dot_h * n_dot_l / n_dot_h;
            float fresnel = pow(1.0 - v_dot_h, 5.0);
            scale += (1.0 - fresnel) * weighted;
            bias += fresnel * weighted;
        }
    }
    float2 table = 4.0 * float2(scale, bias) / float(push.samples);
    storage[push.target][uint3(id.xy, 0)] = float4(table, 0.0, 1.0);
}
