// The sky's prefiltering, in compute passes, as Khronos's glTF Sample Renderer prefilters an
// environment (source/shaders/ibl_filtering.frag, at commit df3e51cb):
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

struct Push {
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
    // The luminance, in the sky's units, the cube clamps the sky to. What's past it is the sun's,
    // which a directional light stands for.
    float cutoff;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

// The sky into a cube's first level: each texel the mean of four points of the image across it,
// each clamped to the cutoff's luminance, keeping its hue.
[shader("compute")]
[numthreads(8, 8, 1)]
void cube_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    Texture2D sky = textures[push.source];
    SamplerState repeat = samplers[LINEAR_REPEAT];
    float3 sum = float3(0.0);
    for (uint i = 0; i < 4; i++) {
        float2 at = float2(id.xy) + float2(0.25 + 0.5 * (i & 1), 0.25 + 0.5 * (i >> 1));
        float3 direction = cube_direction(id.z, at, float(push.size));
        float3 color = equirect(sky, repeat, float2(push.source_size), direction);
        float luminance = dot(color, float3(0.2126, 0.7152, 0.0722));
        sum += luminance > push.cutoff ? color * (push.cutoff / luminance) : color;
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
