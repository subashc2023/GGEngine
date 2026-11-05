// Models' meshes, shaded as glTF's metallic-roughness materials are, by every light of the frame
// and its sky; or, in a frame without either and for a material that's unlit, drawn unlit, as
// KHR_materials_unlit defines it: each fragment its material's base color times its texture's,
// sampled at the vertices' texture coordinates, times the vertices' color. Each instance is a mesh
// with its own transform, ID and material, writing its ID where it shows. `ModelVertex` in
// src/model.rs, which `Vertex` here matches field for field, and `Params`, which `Params` here
// matches byte for byte; the material itself is in the table the frame's data points at.
//
// Shading follows glTF 2.0's Appendix B, as Khronos's sample viewer implements it: a Lambertian
// diffuse lobe and a GGX specular lobe, with Smith's height-correlated visibility, Schlick's
// Fresnel, and 0.04 reflected head-on by every dielectric. A material of
// KHR_materials_pbrSpecularGlossiness's is shaded by the same lobes, as the extension's reference
// has it: it reflects its specular color head-on, diffuses its diffuse color less that color's
// strongest channel, and is as rough as 1 less its glossiness. Lights fall off as
// KHR_lights_punctual's reference does. The sky lights it from all around by its prefiltered
// light, through the split sum, with Fdez-Agüera's roughness-dependent Fresnel and his scattering
// many times over ("A Multiple-Scattering Microfacet Model for Real-Time Image-based Lighting",
// 2019), as the sample viewer's ibl.glsl and pbr.frag have it (glTF-Sample-Renderer at df3e51cb),
// which the occlusion map darkens, as it darkens nothing else. The light that casts the frame's
// shadows lights only what its shadow maps see of it. Their light, times the frame's exposure,
// then the material's emission, as the view shows it, make the color.
//
// As it is, it's for materials that blend by their alpha, and discards what's fully transparent,
// which blending would leave as it was, so that a pick there finds what's behind it; it writes no
// depth, so it keeps early depth testing. model_opaque.slang and model_masked.slang include it
// with OPAQUE or MASKED defined, for materials whose alpha glTF ignores, or uses only to cut out
// what's less opaque than their cutoff: both are drawn fully opaque, so they return an alpha of 1.
// Of those, only the masked one discards, since a shader that can discard while it writes depth
// loses early depth testing on some GPUs; its shadows are cut out as it is, by `shadow_main`,
// where those of an opaque material, drawn without a fragment shader, are whole.

#include "mesh.slang"

struct Vertex {
    float3 position;
    float2 uv;
    // Linear RGBA.
    float4 color;
    float3 normal;
    // Its W the sign of the bitangent, cross(normal, tangent) * W.
    float4 tangent;
};

struct Params {
    // The material's index in the frame's table.
    uint material;
};

[[vk::push_constant]] ConstantBuffer<Push<Vertex, Params>> push;

static const float PI = 3.14159265358979;
// The least roughness shading takes, as Filament's: below it the specular lobe is too narrow to
// filter, and at 0 its peak divides by 0.
static const float LEAST_ROUGHNESS = 0.045;
// The largest a half float holds, short of infinity, which the view's colors are.
static const float BRIGHTEST = 65504.0;
static const uint UNLIT = 1;
static const uint NORMAL_MAPPED = 2;
static const uint SPECULAR_GLOSSINESS = 4;
// `Sampler::LinearClamp`'s index in the bindless set.
static const uint LINEAR_CLAMP = 0;
// The texels inside its map's edges that each cascade keeps the points it shadows to, as `MARGIN`
// in src/shadow.rs says.
static const float MARGIN = 8.0;
// How far a point moves along its surface's normal, in texels of its cascade, times the sine of
// the light's angle to the normal, before its shadow map is sampled, so that a surface doesn't
// shadow itself: the filter's reach. Where a plane slopes from the light at an angle, its depth
// changes by the angle's tangent a texel, so the texels the filter reads, up to 3 away, are as
// much as 3 tangents of a texel nearer the light than the point; moved off it by `d`, the point
// is `d` over the cosine nearer, which is enough where `d` is 3 sines of a texel (Holbert's normal
// offset, "Saying Goodbye to Shadow Acne", GDC 2011).
static const float NORMAL_OFFSET = 3.0;
// How far a point moves toward the light, in texels of its cascade, before its shadow map is
// sampled, besides: where a surface faces the light, the normal offset is 0, and its depths and
// the point's, found by the rasterizer and the shader, are the same but for their rounding, which
// would leave some of it in its own shadow.
static const float LIGHT_OFFSET = 1.0;

struct Varyings {
    float4 position : SV_Position;
    // Where it is in the world.
    float3 world;
    float2 uv;
    float4 color;
    float3 normal;
    float4 tangent;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Vertex vertex = push.vertices[index];
    float4x4 transform = push.transforms[instance];
    float4 world = mul(transform, float4(vertex.position, 1.0));
    float3x3 linear = (float3x3)transform;
    // Normals go by the inverse of the transform, transposed, which the cofactors' matrix is up
    // to the determinant: its columns are the cross products of the transform's columns. Scaled
    // by the determinant's sign, so that a mirrored mesh's normals still face out; its tangents'
    // W flips with it, as the bitangent mirrors too.
    float3x3 columns = transpose(linear);
    float3 a = columns[0];
    float3 b = columns[1];
    float3 c = columns[2];
    float side = dot(a, cross(b, c)) < 0.0 ? -1.0 : 1.0;
    float3 normal = vertex.normal.x * cross(b, c) + vertex.normal.y * cross(c, a)
        + vertex.normal.z * cross(a, b);
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.world = world.xyz;
    out.uv = vertex.uv;
    out.color = vertex.color;
    out.normal = normal * side;
    out.tangent = float4(mul(linear, vertex.tangent.xyz), vertex.tangent.w * side);
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

// What `material`'s texture `which` holds at `uv`, by its sampler: the instances of one draw can
// carry different materials.
float4 sampled(Material material, uint which, float2 uv) {
    uint2 at = material.textures[which];
    Texture2D texture = textures[NonUniformResourceIndex(at.x)];
    return texture.Sample(samplers[NonUniformResourceIndex(at.y)], uv);
}

// `x` to the fifth power.
float pow5(float x) {
    float x2 = x * x;
    return x2 * x2 * x;
}

// How much of the sky's light GGX reflects, of a material of `f0` and `roughness` seen at a cosine
// of `n_dot_v`, by the split sum's `table`, there: once, with Fresnel's dependence on roughness,
// and many times over.
float3 sky_reflectance(float3 f0, float roughness, float n_dot_v, float2 table) {
    float3 rough = max(float3(1.0 - roughness), f0) - f0;
    float3 once = (f0 + rough * pow5(1.0 - n_dot_v)) * table.x + table.y;
    float missed = 1.0 - (table.x + table.y);
    float3 average = f0 + (1.0 - f0) / 21.0;
    return once + missed * once * average / (1.0 - average * missed);
}

// The sky's light from all around reflected by a surface of `roughness`, facing `normal` and seen
// along `view` at a cosine of `n_dot_v`, as luminance before the exposure: a surface that diffuses
// `diffuse_color` and reflects `f0` head-on, mixed by `metallic` with a metal of `metal` color.
float3 sky_light(Frame frame, float3 diffuse_color, float3 f0, float3 metal, float metallic,
                 float roughness, float3 normal, float3 view, float n_dot_v) {
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 diffuse = cubes[frame.irradiance].SampleLevel(clamp_, normal, 0.0).rgb * diffuse_color;
    float level = roughness * float(frame.specular_levels - 1);
    float3 reflected = reflect(-view, normal);
    float3 specular = cubes[frame.specular].SampleLevel(clamp_, reflected, level).rgb;
    float2 table = textures[frame.brdf].SampleLevel(clamp_, float2(n_dot_v, roughness), 0.0).rg;
    float3 dielectric = lerp(diffuse, specular, sky_reflectance(f0, roughness, n_dot_v, table));
    float3 metal_light = sky_reflectance(metal, roughness, n_dot_v, table) * specular;
    return lerp(dielectric, metal_light, metallic) * frame.sky_luminance;
}

// What the shadow map `map`, `size` texels across, holds of light at `uv`, compared with `depth`:
// each texel near it 1 where `depth` is as near the light as the texel's or nearer, and 0 where
// it's farther, weighted by a kernel that falls from 4 at the point to 0 three texels away, in
// three straight pieces, read in nine samples of four texels each, which the comparison sampler
// filters, at points the point's place in its texel picks so that their weights add up to the
// kernel's. The kernel is Castaño's of 5 ("Shadow Mapping Summary", 2013, as The Witness filters
// its shadows), which is continuous, so that the shadow's edge moves smoothly with the point.
float filtered(uint map, float2 uv, float depth, float size) {
    Texture2D texture = textures[NonUniformResourceIndex(map)];
    SamplerComparisonState compare = comparisons[COMPARISON_SAMPLER];
    float2 texels = uv * size;
    // The corner between texels nearest the point, and how far the point is past the texel center
    // before it, from 0 to 1.
    float2 corner = floor(texels + 0.5);
    float2 s = texels + 0.5 - corner;
    float2 base = corner - 0.5;
    // Each sample's weight and its place, from the texel center before the corner, across then
    // down.
    float3 wu = float3(4.0 - 3.0 * s.x, 7.0, 1.0 + 3.0 * s.x);
    float3 u = float3((3.0 - 2.0 * s.x) / wu.x - 2.0, (3.0 + s.x) / 7.0, s.x / wu.z + 2.0);
    float3 wv = float3(4.0 - 3.0 * s.y, 7.0, 1.0 + 3.0 * s.y);
    float3 v = float3((3.0 - 2.0 * s.y) / wv.x - 2.0, (3.0 + s.y) / 7.0, s.y / wv.z + 2.0);
    float sum = 0.0;
    for (uint j = 0; j < 3; j++) {
        for (uint i = 0; i < 3; i++) {
            float2 at = (base + float2(u[i], v[j])) / size;
            sum += wu[i] * wv[j] * texture.SampleCmpLevelZero(compare, at, depth);
        }
    }
    return sum / 144.0;
}

// How much of the light that casts `shadows` reaches `world`, on a surface whose normal before its
// normal map is `normal`, the light coming from `to_light`: from 0 in its shadow to 1 out of it.
// The nearest cascade whose map holds the point, and the texels the filter reads round it, shadows
// it, from the point moved off its surface by the normal offset and toward the light; past them
// all, or past the shadows' reach, it's lit, and toward their reach, more and more so.
float shadowed(Shadows shadows, float3 world, float3 normal, float3 to_light) {
    float depth = dot(shadows.view_depth.xyz, world) + shadows.view_depth.w;
    if (depth >= shadows.reach) {
        return 1.0;
    }
    float3 light = mul(shadows.light_from_world, float4(world, 1.0)).xyz;
    float3 off = mul((float3x3)shadows.light_from_world, normal);
    float n_dot_l = dot(normal, to_light);
    float sine = sqrt(saturate(1.0 - n_dot_l * n_dot_l));
    float inside = 1.0 - 2.0 * MARGIN / shadows.map_size;
    for (uint i = 0; i < CASCADES; i++) {
        float4 cascade = shadows.cascades[i];
        float3 n = (light - cascade.xyz) * cascade.w;
        if (max(max(abs(n.x), abs(n.y)), abs(n.z)) >= inside) {
            continue;
        }
        float texel = 2.0 / (cascade.w * shadows.map_size);
        // The light's space's +Z is toward the light.
        float3 moved = off * (NORMAL_OFFSET * sine) + float3(0.0, 0.0, LIGHT_OFFSET);
        n = (light + moved * texel - cascade.xyz) * cascade.w;
        float lit = filtered(shadows.maps[i], n.xy * float2(0.5, -0.5) + 0.5, n.z * 0.5 + 0.5,
                             shadows.map_size);
        float fade = saturate((depth - shadows.fade_from) / (shadows.reach - shadows.fade_from));
        return lerp(lit, 1.0, fade);
    }
    return 1.0;
}

[shader("fragment")]
Targets fragment_main(Varyings input, bool front: SV_IsFrontFace) {
    Frame frame = *push.frame;
    Material material = frame.materials[push.params[input.instance].material];
    // Every texture sampled before any branch or discard, which would leave the derivatives that
    // pick their levels undefined. Base colors decode from sRGB as they're sampled, as the
    // emission's and a specular-glossiness map's specular colors do; the other maps, and that
    // map's glossiness in alpha, are read as stored.
    float4 color = sampled(material, 0, input.uv) * material.color * input.color;
    float4 surface = sampled(material, 1, input.uv);
    float2 mapped = sampled(material, 2, input.uv).xy * 2.0 - 1.0;
    float occlusion = 1.0 + material.occlusion_strength * (sampled(material, 3, input.uv).r - 1.0);
    float3 emission = sampled(material, 4, input.uv).rgb * material.emissive;
#if defined(MASKED)
    if (color.a < material.cutoff) {
        discard;
    }
#elif !defined(OPAQUE)
    if (color.a == 0.0) {
        discard;
    }
#endif
#if defined(OPAQUE) || defined(MASKED)
    color.a = 1.0;
#endif
    Targets out;
    out.id = input.id;
    if ((frame.light_count == 0 && frame.specular_levels == 0) || (material.flags & UNLIT) != 0) {
        out.color = color;
        return out;
    }
    // A back face seen of a double-sided material faces the other way, its tangent space too.
    float facing = front ? 1.0 : -1.0;
    float3 normal = normalize(input.normal) * facing;
    // Which way the surface faces where its shadows are sampled: the normal map's bumps cast none.
    float3 surface_normal = normal;
    if ((material.flags & NORMAL_MAPPED) != 0) {
        // Kept square to the normal, which interpolation doesn't keep them.
        float3 tangent = input.tangent.xyz * facing;
        tangent = normalize(tangent - normal * dot(normal, tangent));
        float3 bitangent = cross(normal, tangent) * input.tangent.w;
        // Z from X and Y as the map holds them, which a BC5 map holds alone; then scaled, as
        // glTF scales a normal map's X and Y.
        float z = sqrt(saturate(1.0 - dot(mapped, mapped)));
        float3 local = float3(mapped * material.normal_scale, z);
        normal = normalize(tangent * local.x + bitangent * local.y + normal * local.z);
    }
    // What it diffuses and reflects head-on, which the frame's lights shade; and for the sky's
    // light, a dielectric's, which it mixes with a metal's by how metallic it is.
    float metallic = 0.0;
    float roughness;
    float3 diffuse_color;
    float3 f0;
    float3 sky_diffuse;
    float3 sky_f0;
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        f0 = material.specular * surface.rgb;
        roughness = clamp(1.0 - material.glossiness * surface.a, LEAST_ROUGHNESS, 1.0);
        diffuse_color = color.rgb * (1.0 - max(max(f0.r, f0.g), f0.b));
        sky_diffuse = diffuse_color;
        sky_f0 = f0;
    } else {
        metallic = saturate(material.metallic * surface.b);
        roughness = clamp(material.roughness * surface.g, LEAST_ROUGHNESS, 1.0);
        diffuse_color = color.rgb * (1.0 - metallic);
        f0 = lerp(float3(0.04), color.rgb, metallic);
        sky_diffuse = color.rgb;
        sky_f0 = float3(0.04);
    }
    float alpha = roughness * roughness;
    float alpha2 = alpha * alpha;
    float3 view = normalize(frame.eye.xyz - input.world * frame.eye.w);
    float n_dot_v = max(dot(normal, view), 1e-4);
    float3 light_sum = float3(0.0);
    for (uint i = 0; i < frame.light_count; i++) {
        Light light = frame.lights[i];
        float3 radiance = light.color;
        float3 to_light;
        if (light.kind == 0) {
            to_light = -light.direction;
        } else {
            float3 between = light.position - input.world;
            // Kept off 0 at the light itself.
            float distance2 = max(dot(between, between), 1e-4);
            to_light = between * rsqrt(distance2);
            float reach = sqrt(distance2) * light.inverse_range;
            float reach2 = reach * reach;
            float window = saturate(1.0 - reach2 * reach2);
            radiance *= window * window / distance2;
            float cone = saturate(dot(light.direction, -to_light) * light.angle_scale
                + light.angle_offset);
            radiance *= cone * cone;
        }
        float n_dot_l = dot(normal, to_light);
        if (n_dot_l <= 0.0) {
            continue;
        }
        if (i == frame.shadows.light) {
            radiance *= shadowed(frame.shadows, input.world, surface_normal, to_light);
        }
        float3 halfway = normalize(to_light + view);
        float n_dot_h = saturate(dot(normal, halfway));
        float v_dot_h = saturate(dot(view, halfway));
        float3 fresnel = f0 + (1.0 - f0) * pow5(1.0 - v_dot_h);
        float spread = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
        float distribution = alpha2 / (PI * spread * spread);
        float visibility = 0.5
            / (n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2)
                + n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2));
        float3 specular = fresnel * distribution * visibility;
        float3 diffuse = (1.0 - fresnel) * diffuse_color / PI;
        light_sum += (diffuse + specular) * radiance * n_dot_l;
    }
    if (frame.specular_levels > 0) {
        light_sum += sky_light(frame, sky_diffuse, sky_f0, color.rgb, metallic, roughness, normal,
                               view, n_dot_v)
            * occlusion;
    }
    float3 shaded = light_sum * frame.exposure + emission;
    out.color = float4(min(shaded, BRIGHTEST), color.a);
    return out;
}

#if defined(MASKED)
// A masked material's shadow, drawn into a shadow map: none where it's less opaque than its
// cutoff, as it draws nothing there.
[shader("fragment")]
void shadow_main(Varyings input) {
    // Loaded whole, as fragment_main's is. Read for its material table alone, slangc reaches the
    // table's address through a pointer into the frame, which AMD's driver (25.10.30) miscompiles
    // beside a texture's sample, reading the frame at address 0 and losing the device.
    Frame frame = loadAligned<8>(push.frame);
    Material material = frame.materials[push.params[input.instance].material];
    float alpha = sampled(material, 0, input.uv).a * material.color.a * input.color.a;
    if (alpha < material.cutoff) {
        discard;
    }
}
#endif
