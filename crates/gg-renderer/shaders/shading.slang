// How models' meshes are shaded, shared by model.slang's fragments and visibility.slang's pixels.
// Unlit (KHR_materials_unlit) where the frame has no lights or sky, or the material is unlit: base
// color times its texture's times the vertices' color.
//
// Lit as glTF 2.0's Appendix B, by Khronos's sample viewer:
// - Lambertian diffuse, GGX specular, Smith's height-correlated visibility, Schlick's Fresnel, and
//   0.04 reflected head-on by dielectrics.
// - KHR_materials_pbrSpecularGlossiness by its reference: specular color head-on, diffuse color
//   times 1 less that color's strongest channel, roughness 1 less glossiness.
// - Light falloff by KHR_lights_punctual's reference.
// - Roughness widened by how far the normal turns across the pixel (`filtered_roughness`), so a
//   highlight narrower than a pixel spreads over it rather than flickering as it moves.
// - The sky's prefiltered light by the split sum, looked up toward the normal as roughness grows.
//   Its diffuse loses to Fresnel what glTF's BRDF does (`diffuse_fresnel`).
// - GGX's light scattered more than once, which glTF's BRDF loses, fitted to trace.slang's walk:
//   the sky's in its reflectance (`scattered`), the lights' by Kulla and Conty's lobe
//   (`reflected`).
// - Occlusion, the map's and the view's, darkens only the sky's light (`ambient`).
// - The view's reflections replace the sky's specular light where found (`reflection`).
// - The shadow-casting light lights only what its shadow maps see.
// - Light times exposure, plus emission, then an atmosphere's haze (aerial.slang).
//
// A point shades by the lights that light everywhere, then its cluster's (clusters.slang), in the
// frame's order; in no cluster, or without clusters, by every light.

#include "common.slang"
#include "mesh.slang"
#include "aerial.slang"

// `PackedVertex` in src/packed.rs, as words; `position_z_sign` holds Z and the bitangent's sign.
struct PackedVertex {
    uint position_xy;
    uint position_z_sign;
    uint normal;
    uint tangent;
    uint uv;
    uint color;
};

// `Header` in src/packed.rs, in `HEADER` vertices' room at a mesh buffer's start: how vertices
// unpack, and where meshlets' parts and the skin (skin.slang) start, in words.
struct Header {
    float3 offset;
    float step;
    float2 uv_offset;
    float2 uv_scale;
    uint meshlets;
    uint meshlet_vertices;
    uint triangles;
    uint skin;
};

// `HEADER_VERTICES` in src/packed.rs.
static const uint HEADER = 2;

struct Vertex {
    float3 position;
    float2 uv;
    // Linear RGBA.
    float4 color;
    float3 normal;
    // W: the bitangent's sign, cross(normal, tangent) * W.
    float4 tangent;
};

// `word`'s low and high halves, each a signed 16-bit fraction of 1.
float2 snorm_halves(uint word) {
    int2 halves = int2(int(word << 16) >> 16, int(word) >> 16);
    return max(float2(halves) / 32767.0, -1.0);
}

// The unit vector `word` holds, folded onto an octahedron by `octahedral` in src/packed.rs.
float3 unfolded(uint word) {
    float2 e = snorm_halves(word);
    float3 v = float3(e, 1.0 - abs(e.x) - abs(e.y));
    float t = max(-v.z, 0.0);
    v.x += v.x >= 0.0 ? -t : t;
    v.y += v.y >= 0.0 ? -t : t;
    return normalize(v);
}

// sRGB-encoded `c`, decoded.
float linear(float c) {
    return c <= 0.04045 ? c / 12.92 : pow((c + 0.055) / 1.055, 2.4);
}

// `packed`, unpacked as its mesh's `header` says.
Vertex unpack(Header header, PackedVertex packed) {
    float3 steps = float3(packed.position_xy & 0xFFFF, packed.position_xy >> 16,
                          packed.position_z_sign & 0xFFFF);
    float2 uv = float2(packed.uv & 0xFFFF, packed.uv >> 16);
    float4 color = float4(packed.color & 0xFF, (packed.color >> 8) & 0xFF,
                          (packed.color >> 16) & 0xFF, packed.color >> 24) / 255.0;
    Vertex vertex;
    vertex.position = header.offset + steps * header.step;
    vertex.uv = header.uv_offset + uv * header.uv_scale;
    vertex.color = float4(linear(color.r), linear(color.g), linear(color.b), color.a);
    vertex.normal = unfolded(packed.normal);
    float sign = int(packed.position_z_sign) < 0 ? -1.0 : 1.0;
    vertex.tangent = float4(unfolded(packed.tangent), sign);
    return vertex;
}

// A vertex's normal and tangent in the world; `side` -1 where its transform mirrors it, else 1.
struct Turned {
    float3 normal;
    float4 tangent;
    float side;
};

// `vertex`'s normal and tangent in the world; the normal unnormalized, as interpolation
// denormalizes it anyway.
Turned turned(Vertex vertex, float4x4 transform) {
    float3x3 linear = (float3x3)transform;
    // Normals go by the inverse transpose: up to the determinant, the cofactor matrix, whose
    // columns are cross products of the transform's. Times the determinant's sign, so mirrored
    // normals still face out; the tangent's W flips with the bitangent.
    float3x3 columns = transpose(linear);
    float3 a = columns[0];
    float3 b = columns[1];
    float3 c = columns[2];
    float side = dot(a, cross(b, c)) < 0.0 ? -1.0 : 1.0;
    float3 normal = vertex.normal.x * cross(b, c) + vertex.normal.y * cross(c, a)
        + vertex.normal.z * cross(a, b);
    Turned out;
    out.normal = normal * side;
    out.tangent = float4(mul(linear, vertex.tangent.xyz), vertex.tangent.w * side);
    out.side = side;
    return out;
}

// The least roughness shading takes, Filament's: below it the specular lobe is too narrow to
// filter; at 0 its peak divides by 0.
static const float LEAST_ROUGHNESS = 0.045;
static const uint UNLIT = 1;
static const uint NORMAL_MAPPED = 2;
static const uint SPECULAR_GLOSSINESS = 4;
// `MARGIN` in src/shadow.rs: the texels inside a map's edges its cascade keeps its points to.
static const float MARGIN = 8.0;
// The normal offset against self-shadowing (Holbert, "Saying Goodbye to Shadow Acne", GDC 2011),
// in texels of the cascade times the sine of the light's angle to the normal. On a sloped plane
// the filter's texels, up to 3 away, are up to 3 tangents nearer the light: 3 sines along the
// normal are 3 tangents nearer.
static const float NORMAL_OFFSET = 3.0;
// The offset toward the light too, in texels of the cascade. Facing the light, the normal offset is
// 0, and the rasterizer's and shader's depths differ by rounding alone, which would self-shadow.
static const float LIGHT_OFFSET = 1.0;

// Non-uniform: one draw's instances can carry different materials.
float4 sampled(Material material, uint which, float2 uv) {
    uint2 at = material.textures[which];
    Texture2D texture = textures[NonUniformResourceIndex(at.x)];
    return texture.Sample(samplers[NonUniformResourceIndex(at.y)], uv);
}

// `sampled`, its level picked by `uv`'s change a pixel `across` and `down`, where nothing finds
// derivatives.
float4 sampled_by(Material material, uint which, float2 uv, float2 across, float2 down) {
    uint2 at = material.textures[which];
    Texture2D texture = textures[NonUniformResourceIndex(at.x)];
    return texture.SampleGrad(samplers[NonUniformResourceIndex(at.y)], uv, across, down);
}

float pow5(float x) {
    float x2 = x * x;
    return x2 * x2 * x;
}

// `scattered`'s q and w, by roughness's powers 0 to 2 down and the cosine's across.
static const float3x3 AGAIN = float3x3(-0.0282046, 1.94727, -2.29303,
                                       -0.78294, -1.61628, 3.18146,
                                       0.935368, 0.218868, -1.10924);
static const float3x3 FLOOR = float3x3(1.05864, -1.45373, 0.426679,
                                       -2.12867, 3.427, -1.24309,
                                       1.15171, -2.08858, 0.874591);

// What GGX reflects past its first scattering at a cosine of `n_dot` from all ways in, given
// `missed`, that at F0 1: 1 less the single scattering's albedo there. Kulla and Conty's form
// ("Revisiting Physically Based Shading at Imageworks", 2017), missed (1 - q) F² / (1 - q F) with
// F = F0 + (1 - F0) w, where they take q as 1 less the albedo's mean and w as 1/21. Here q and w
// are quadratics fitted to trace.slang's walk: within 0.31% of the albedo for F0 0.2 and up, and
// 0.19% of the single scattering's below. At F0 1 it's `missed`, so nothing is lost.
float3 scattered(float3 f0, float roughness, float n_dot, float missed) {
    float3 by_roughness = float3(1.0, roughness, roughness * roughness);
    float3 by_cosine = float3(1.0, n_dot, n_dot * n_dot);
    float q = dot(by_roughness, mul(AGAIN, by_cosine));
    float3 f = f0 + (1.0 - f0) * dot(by_roughness, mul(FLOOR, by_cosine));
    return missed * (1.0 - q) * f * f / (1.0 - q * f);
}

// `missed`'s fit, by the powers of s from 0 to 4, each by alpha's from 0 to 3.
static const float4 MISSED[5] = {
    float4(0.0474549, -0.241842, 0.396059, -0.180501),
    float4(-0.483469, 2.70171, 0.827328, -0.0301215),
    float4(3.33921, -9.51835, -3.18978, 1.48813),
    float4(-5.56297, 12.0909, 18.9256, -11.6931),
    float4(2.65828, -4.88738, -19.527, 12.4488),
};

// 1 less the single scattering's albedo at F0 1, at a cosine of `n_dot` and `alpha`: the split
// sum's 1 - (A + B), without its table, which only a sky makes. A polynomial in alpha and
// s = n_dot / (n_dot + alpha), by which Smith's masking goes, fitted to prefilter.slang's sum:
// within 0.004.
float missed(float n_dot, float alpha) {
    float s = n_dot / (n_dot + alpha);
    float4 by_s = MISSED[0] + s * (MISSED[1] + s * (MISSED[2] + s * (MISSED[3] + s * MISSED[4])));
    float4 by_alpha = float4(1.0, alpha, alpha * alpha, alpha * alpha * alpha);
    return max(dot(by_alpha, by_s), 0.0);
}

// The least roughness the lights' lobe of light scattered more than once takes its shape at.
// Below it, the true lobe is a spike at grazing ways that `missed` doesn't hold. Taken at it, the
// lobe adds at most 0.016 of a light, at views within a degree of grazing.
static const float LEAST_LOBE_ROUGHNESS = 0.25;

// `missed`'s cosine-weighted mean over the hemisphere, from LEAST_LOBE_ROUGHNESS to 1: a quartic in
// `roughness` fitted to it, within 0.54%.
float missed_mean(float roughness) {
    return 0.00804913 + roughness * (-0.0594239 + roughness * (-0.117123
        + roughness * (1.97411 + roughness * -1.21803)));
}

// How much of the sky's light GGX reflects at a cosine of `n_dot_v`, by the split sum's `table`
// there: once, plus `scattered`.
float3 sky_reflectance(float3 f0, float roughness, float n_dot_v, float2 table) {
    float3 once = f0 * table.x + table.y;
    return once + scattered(f0, roughness, n_dot_v, 1.0 - (table.x + table.y));
}

// What Fresnel takes of glTF's diffuse, (1 - F(v.h)) c / pi, past F0, from a uniform sky seen at a
// cosine of `n_dot`, over 1 - F0: the cosine-weighted mean of (1 - v.h)^5 over the ways in. A
// quartic fitted to it, within 1e-4; 0.028 at grazing, 0 head-on.
float diffuse_fresnel(float n_dot) {
    return 0.0282312 + n_dot * (-0.0800283 + n_dot * (0.0865912 + n_dot * (-0.0436652
        + n_dot * 0.00895321)));
}

// Lagarde and de Rousiers' specular occlusion ("Moving Frostbite to Physically Based Rendering",
// 2014, listing 26), of GGX's alpha, as Filament takes it: a rough lobe takes `occlusion`, as
// diffuse does; a smooth one less at grazing angles, more head-on.
float specular_occlusion(float n_dot_v, float occlusion, float roughness) {
    float alpha = roughness * roughness;
    return saturate(pow(n_dot_v + occlusion, exp2(-16.0 * alpha - 1.0)) - 1.0 + occlusion);
}

// The sky's light diffused, as luminance before the exposure, and reflected, in `specular_light`:
// a dielectric (`diffuse_color`, `f0`) mixed by `metallic` with a metal, which diffuses none.
// Specular takes `specular_occlusion` and 1 less `taken`, the reflection's share. `reflectance`:
// how much of a reflection's light it reflects. Diffuse loses to Fresnel what glTF's BRDF does
// (`diffuse_fresnel`), as lights and paths take it: 1 less the reflectance, as Fdez-Agüera has it,
// is a third darker at grazing views.
float3 sky_light(Frame frame, float3 diffuse_color, float3 f0, float3 metal, float metallic,
                 float roughness, float3 normal, float3 view, float n_dot_v, float occlusion,
                 float taken, out float3 reflectance, out float3 specular_light) {
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 diffuse = cubes[frame.irradiance].SampleLevel(clamp_, normal, 0.0).rgb * diffuse_color
        * occlusion;
    float level = roughness * float(frame.specular_levels - 1);
    // The cube's lobes are about their texels' ways, but a rough surface's isn't about the mirror
    // way: it leans toward the normal, where the mirror way's would reach below the horizon.
    // Filament's dominant direction (getSpecularDominantDirection) takes the mirror way toward the
    // normal by alpha squared.
    float alpha = roughness * roughness;
    float3 reflected = lerp(reflect(-view, normal), normal, alpha * alpha);
    float3 specular = cubes[frame.specular].SampleLevel(clamp_, reflected, level).rgb
        * (specular_occlusion(n_dot_v, occlusion, roughness) * (1.0 - taken));
    float2 table = textures[frame.brdf].SampleLevel(clamp_, float2(n_dot_v, roughness), 0.0).rg;
    float3 dielectric_reflectance = sky_reflectance(f0, roughness, n_dot_v, table);
    float3 metal_reflectance = sky_reflectance(metal, roughness, n_dot_v, table);
    reflectance = lerp(dielectric_reflectance, metal_reflectance, metallic);
    specular_light = specular * reflectance * frame.sky_luminance;
    float3 kept = (1.0 - f0) * ((1.0 - diffuse_fresnel(n_dot_v)) * (1.0 - metallic));
    return diffuse * kept * frame.sky_luminance;
}

// How lit `uv` at `depth` is in shadow map `map`, `size` texels across: texels 1 where `depth` is
// no farther from the light, weighted by Castaño's kernel of 5 ("Shadow Mapping Summary", 2013, as
// The Witness filters shadows): 4 at the point to 0 three texels away, in three straight pieces.
// It's continuous, so edges move smoothly. Nine comparison-filtered samples of four texels, placed
// so their weights sum to the kernel.
float filtered(uint map, float2 uv, float depth, float size) {
    Texture2D texture = textures[NonUniformResourceIndex(map)];
    SamplerComparisonState compare = comparisons[COMPARISON_SAMPLER];
    float2 texels = uv * size;
    // The texel corner nearest the point, and how far past the texel center before it the point
    // is, 0 to 1.
    float2 corner = floor(texels + 0.5);
    float2 s = texels + 0.5 - corner;
    float2 base = corner - 0.5;
    // Each sample's weight and place, from the texel center before the corner, across then down.
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

// How much of the light casting `shadows` reaches `world`, 0 to 1; `normal` is before the normal
// map. The nearest cascade holding the offset point and the filter's texels shadows it. Past them
// all, or the shadows' reach, it's lit, and more and more so toward the reach.
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

// What a point's lights shade.
struct Surface {
    float3 world;
    float3 normal;
    // Before its normal map, where its shadows are sampled: the map's bumps cast none.
    float3 surface_normal;
    float3 view;
    float n_dot_v;
    // Diffuse and head-on colors; `alpha2`, roughness to the fourth power.
    float3 diffuse_color;
    float3 f0;
    float alpha2;
    // What specular light is scaled by: 1, or 0 where the view shows diffuse light alone (a
    // diffuse color of 0 shows specular alone).
    float3 specular_weight;
    // The lights' lobe of light scattered more than once (`reflected`) but its shape, `missed` at
    // the light's cosine and `lobe_alpha`.
    float3 lobe;
    float lobe_alpha;
};

// The light the frame's `i`th light shines on `world`, before its shadows and the exposure, and
// which way it comes from, `to_light`.
float3 arriving(Frame frame, uint i, float3 world, out float3 to_light) {
    // Loaded here: passed in as a `Light`, it lost the lights' address in `lit`'s cluster loop on
    // AMD's driver (25.10.30) under slangc 2026.20.
    Light light = frame.lights[i];
    float3 radiance = light.color;
    if (light.kind == 0) {
        to_light = -light.direction;
    } else {
        float3 between = light.position - world;
        // Kept off 0 at the light itself.
        float distance2 = max(dot(between, between), 1e-4);
        to_light = between * rsqrt(distance2);
        // The inverse square, eased to 0 at the range by KHR_lights_punctual's README's window,
        // max(min(1 - (distance / range)^4, 1), 0), not squared.
        float reach = sqrt(distance2) * light.inverse_range;
        float reach2 = reach * reach;
        float window = saturate(1.0 - reach2 * reach2);
        radiance *= window / distance2;
        float cone = saturate(dot(light.direction, -to_light) * light.angle_scale
            + light.angle_offset);
        radiance *= cone * cone;
    }
    return radiance;
}

// What `surface` reflects toward its view of light from `to_light`, at a cosine of `n_dot_l` to its
// normal, per unit of that light: its BRDF, Lambertian diffuse and GGX specular.
float3 brdf(Surface surface, float3 to_light, float n_dot_l) {
    float3 normal = surface.normal;
    float3 view = surface.view;
    float n_dot_v = surface.n_dot_v;
    float alpha2 = surface.alpha2;
    float3 f0 = surface.f0;
    float3 halfway = normalize(to_light + view);
    float n_dot_h = saturate(dot(normal, halfway));
    float v_dot_h = saturate(dot(view, halfway));
    float3 fresnel = f0 + (1.0 - f0) * pow5(1.0 - v_dot_h);
    float spread = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    float distribution = alpha2 / (PI * spread * spread);
    float visibility = 0.5
        / (n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2)
            + n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2));
    float3 specular = fresnel * (distribution * visibility * surface.specular_weight);
    float3 diffuse = (1.0 - fresnel) * surface.diffuse_color / PI;
    return diffuse + specular;
}

// The light the frame's `i`th light reflects from `surface` toward the view, before the exposure:
// `brdf`'s single scattering, and Kulla and Conty's lobe of what scatters more than once.
// `scattered` at the view times `missed` at the light, over pi and `missed`'s mean, so the lights
// of every way sum to `scattered`. Against trace.slang's walk, which paths take in its place:
// within 12% at roughness 0.75 and 1. Smoother surfaces glow with lights behind them, where they
// reflect little: 37% over at 0.5.
float3 reflected(Frame frame, Surface surface, uint i) {
    float3 to_light;
    float3 radiance = arriving(frame, i, surface.world, to_light);
    float n_dot_l = dot(surface.normal, to_light);
    if (n_dot_l <= 0.0) {
        return float3(0.0);
    }
    if (i == frame.shadows.light) {
        radiance *= shadowed(frame.shadows, surface.world, surface.surface_normal, to_light);
    }
    float3 lobe = surface.lobe * surface.specular_weight * missed(n_dot_l, surface.lobe_alpha);
    return (brdf(surface, to_light, n_dot_l) + lobe) * radiance * n_dot_l;
}

// The ambient light's share reaching `pixel`: the less of the view's ambient occlusion and the
// material's `occlusion`, as Filament takes them; their product would darken twice where both see
// one occluder.
float ambient(Frame frame, uint2 pixel, float occlusion) {
    if (frame.ambient_occlusion == 0) {
        return occlusion;
    }
    return min(occlusion, textures[frame.ambient_occlusion].Load(int3(int2(pixel), 0)).r);
}

// `material`'s roughness where its surface map holds `surface`.
float roughness_of(Material material, float4 surface) {
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        return clamp(1.0 - material.glossiness * surface.a, LEAST_ROUGHNESS, 1.0);
    }
    return clamp(material.roughness * surface.g, LEAST_ROUGHNESS, 1.0);
}

// Geometric specular antialiasing's screen-space variance of the pixel's filter, and the most it
// adds to GGX's alpha squared, against its derivatives' error: Filament's defaults
// (`specularAntiAliasingVariance` and `specularAntiAliasingThreshold`).
static const float SPECULAR_VARIANCE = 0.15;
static const float SPECULAR_THRESHOLD = 0.2;

// `roughness` widened for a normal that turns by `across` and `down` a pixel across and down, the
// interpolated normal's change: the pixel's spread of normals added to the lobe's alpha squared
// (Tokuyoshi and Kaplanyan, "Improved Geometric Specular Antialiasing", 2019, as Filament's
// `normalFiltering` has it). A normal map's own spread within the pixel isn't counted.
float filtered_roughness(float roughness, float3 across, float3 down) {
    float spread = min(2.0 * SPECULAR_VARIANCE * (dot(across, across) + dot(down, down)),
                       SPECULAR_THRESHOLD);
    float alpha = roughness * roughness;
    return sqrt(sqrt(saturate(alpha * alpha + spread)));
}

// The share of `Reflected::roughness`, below it, over which the view's reflections fade into the
// sky's specular light.
static const float ROUGHNESS_FADE = 0.25;
// The reflections' levels: `LEVELS` in src/reflections.rs.
static const uint REFLECTED_LEVELS = 7;
// A reflection's blur in pixels, as a standard deviation, per pixel its lobe's tangent spreads and
// per unit of GGX's alpha: a Gaussian's as wide at half its height as GGX's narrow lobe about the
// mirror way, whose half width there is 2 alpha sqrt(sqrt(2) - 1).
static const float BLUR_PER_ALPHA = 1.093;

// What a level of the reflections holds round a pixel: sums of light times its trust, of trust, of
// the share covered, and of spread times trust.
struct Around {
    float3 light;
    float trust;
    float covered;
    float spread;
};

// Level `level` of the reflections (`colors`) and their surfaces round `pixel`, read smoothly: a
// quadratic B-spline over the 3 by 3 texels nearest it, each weighed by how alike the normal it
// holds is to `normal`, by the eighth power of their cosine: low enough that a ball's own
// reflection blurs over its curve. Weighed texel by texel, it stays continuous as the texels it
// reads change, however the weights differ.
Around around(Texture2D colors, Texture2D surfaces, uint2 pixel, uint level, float3 normal) {
    uint width;
    uint height;
    colors.GetDimensions(width, height);
    int2 size = int2(uint2(width, height) >> level);
    // The pixel's middle in the level's texels, from the first texel's middle, and how far it is
    // from the nearest texel's.
    float2 at = (float2(pixel) + 0.5) / exp2(float(level)) - 0.5;
    float2 nearest = floor(at + 0.5);
    float2 t = at - nearest;
    float2 weights[3] = { 0.5 * (0.5 - t) * (0.5 - t), 0.75 - t * t, 0.5 * (0.5 + t) * (0.5 + t) };
    Around sum = { float3(0.0), 0.0, 0.0, 0.0 };
    for (int y = 0; y < 3; y++) {
        for (int x = 0; x < 3; x++) {
            int2 texel = int2(nearest) + int2(x - 1, y - 1);
            if (any(texel < 0) || any(texel >= size)) {
                continue;
            }
            float4 color = colors.Load(int3(texel, int(level)));
            float4 surface = surfaces.Load(int3(texel, int(level)));
            // The normals' mean is as long as the share covered, where they're alike.
            float covered = length(surface.xyz);
            float alike = covered > 0.0 ? saturate(dot(surface.xyz, normal) / covered) : 0.0;
            alike *= alike;
            alike *= alike;
            float weight = weights[x].x * weights[y].y * alike * alike;
            sum.light += weight * color.rgb;
            sum.trust += weight * color.a;
            sum.covered += weight * covered;
            sum.spread += weight * surface.w;
        }
    }
    return sum;
}

// `around`'s light times its trust, and its trust, per pixel covered; `own` where too little alike
// is round it.
float4 mean_of(Around around, float4 own) {
    return around.covered > 1.0e-4 ? float4(around.light, around.trust) / around.covered : own;
}

// The variance, in pixels squared, of a point's blur at level `level` past the first: of the
// binomial means down to it, a quarter of 4^level less 1, and of the quadratic B-spline over its
// texels, a quarter of 4^level.
float level_variance(uint level) {
    float texels = exp2(2.0 * float(level));
    return 0.25 * (texels - 1.0) + 0.25 * texels;
}

// The level of the surfaces `curve` reads: texels of 4 pixels, whose mean normals smooth out a
// normal map's finer bumps but follow a ball a few dozen pixels across.
static const uint CURVE_LEVEL = 2;

// How fast the surface at `pixel`, of normal `normal`, turns, in radians a pixel across and down:
// from its mean normals a texel of `CURVE_LEVEL` to either side, `normal` where none is.
float2 curve(Texture2D surfaces, uint2 pixel, float3 normal) {
    uint width;
    uint height;
    surfaces.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float2 uv = (float2(pixel) + 0.5) * texel;
    float span = exp2(float(CURVE_LEVEL));
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    static const float2 WAYS[4] = { float2(-1.0, 0.0), float2(1.0, 0.0), float2(0.0, -1.0),
                                    float2(0.0, 1.0) };
    float3 sides[4];
    for (uint i = 0; i < 4; i++) {
        float2 side = uv + WAYS[i] * span * texel;
        float3 mean = surfaces.SampleLevel(clamp_, side, float(CURVE_LEVEL)).xyz;
        sides[i] = dot(mean, mean) > 1.0e-8 ? normalize(mean) : normal;
    }
    // Chords, as near the angles as a pixel's turn needs.
    return float2(length(sides[1] - sides[0]), length(sides[3] - sides[2])) / (2.0 * span);
}

// The view's reflection at `pixel` (reflections.slang): RGB the light its mirror ray found,
// exposed, blurred by the lobe; alpha the share of the sky's specular light it replaces, fading out
// toward `Reflected::roughness`; 0 for none. Where `Frame::show` asks, the first level as it is.
//
// The blur has the variance of a Gaussian as wide as GGX's lobe at the ray's hit or, where it found
// nothing, at those round it that did: the pixel's own reflection and the levels, mixed to that
// variance. On a curve the lobe spans fewer pixels: a ball's reflection stays on its side facing
// what it shows, read from levels fine enough not to swim as the view turns. It takes in only
// surfaces facing as the pixel's does, and its share is their mean trust.
float4 reflection(Frame frame, uint2 pixel, float roughness) {
    Reflected reflected = frame.reflections;
    if (reflected.colors == 0) {
        return float4(0.0);
    }
    // Shown as traced, whatever the roughness: the first level, light times trust, unblurred, or
    // each ray's outcome.
    if (frame.show > SHOW_SPECULAR_LIGHT) {
        return textures[reflected.colors].Load(int3(int2(pixel), 0));
    }
    float fade = saturate((reflected.roughness - roughness) / (reflected.roughness * ROUGHNESS_FADE));
    if (!(fade > 0.0)) {
        return float4(0.0);
    }
    Texture2D colors = textures[reflected.colors];
    Texture2D surfaces = textures[reflected.surfaces];
    int3 at = int3(int2(pixel), 0);
    float4 own = colors.Load(at);
    float4 surface = surfaces.Load(at);
    // None where the trace found nothing opaque.
    if (!(dot(surface.xyz, surface.xyz) > 0.0)) {
        return float4(0.0);
    }
    float3 normal = normalize(surface.xyz);
    float spread;
    if (own.a > 0.0) {
        spread = surface.w / own.a;
    } else {
        // As far as those round it that found something, as the last level holds them.
        uint width;
        uint height;
        colors.GetDimensions(width, height);
        float2 uv = (float2(pixel) + 0.5) / float2(width, height);
        float last = float(REFLECTED_LEVELS - 1);
        SamplerState clamp_ = samplers[LINEAR_CLAMP];
        float trust = colors.SampleLevel(clamp_, uv, last).a;
        if (!(trust > 0.0)) {
            return float4(0.0);
        }
        spread = surfaces.SampleLevel(clamp_, uv, last).w / trust;
    }
    // The trace's spread is a flat mirror's. On a curve, each pixel spans more of the lobe's tangent:
    // twice the normal's turn more, as a mirror's rays turn twice as fast. The blur, round, takes the
    // geometric mean of the spreads across and down, as wide as the lobe's footprint's area.
    float2 spreads = 1.0 / (1.0 / spread + 2.0 * curve(surfaces, pixel, normal));
    spread = sqrt(spreads.x * spreads.y);
    float blur = BLUR_PER_ALPHA * roughness * roughness * spread;
    float variance = blur * blur;
    // The coarser of the two blurs mixed; the finer is the level before, or the pixel's own
    // reflection, of no variance, before the second.
    uint upper = 1;
    while (upper < REFLECTED_LEVELS - 1 && level_variance(upper) < variance) {
        upper++;
    }
    float below = upper == 1 ? 0.0 : level_variance(upper - 1);
    float f = saturate((variance - below) / (level_variance(upper) - below));
    float4 mean = float4(0.0);
    for (uint i = 0; i < 2; i++) {
        uint level = upper - 1 + i;
        float share = i == 0 ? 1.0 - f : f;
        // Each read only where it adds a visible share.
        if (share > 1.0 / 1024.0) {
            float4 blurred = level == 0
                ? own
                : mean_of(around(colors, surfaces, pixel, level, normal), own);
            mean += share * blurred;
        }
    }
    if (!(mean.a > 0.0)) {
        return float4(0.0);
    }
    return float4(mean.rgb / mean.a, mean.a * fade);
}

// A point in no cluster.
static const uint NO_CLUSTER = 0xFFFFFFFF;

// The index of `world`'s cluster, as `Clusters` numbers them; `NO_CLUSTER` without clusters, or
// where the view doesn't see it.
uint cluster(Clusters clusters, float3 world) {
    if (clusters.words == 0) {
        return NO_CLUSTER;
    }
    float4 p = float4(world, 1.0);
    float w = dot(clusters.w, p);
    float2 tile = float2(dot(clusters.x, p), dot(clusters.y, p)) / w;
    float depth = dot(clusters.depth, p);
    float slice = (clusters.perspective != 0 ? log2(depth) : depth) * clusters.slice_scale
        + clusters.slice_bias;
    // So that NaN, from a point behind a perspective's eye, is outside too.
    bool inside = w > 0.0 && tile.x >= 0.0 && tile.y >= 0.0 && tile.x < float(clusters.across)
        && tile.y < float(clusters.down) && slice >= 0.0;
    if (!inside) {
        return NO_CLUSTER;
    }
    uint3 at = uint3(uint2(tile), min(uint(slice), clusters.slices - 1));
    return (at.z * clusters.down + at.y) * clusters.across + at.x;
}

// Which way a point of `material` faces, as `lit` shades it: its `interpolated` normal, flipped
// toward the view on a back face (a double-sided material's), then bent along `tangent` by its
// normal map's X and Y, `mapped`, -1 to 1, if it has one. `surface_normal`: before the normal map.
float3 shading_normal(Material material, float3 interpolated, float4 tangent, bool front,
                      float2 mapped, out float3 surface_normal) {
    // A double-sided material's back face faces the other way, its tangent space too.
    float facing = front ? 1.0 : -1.0;
    float3 normal = normalize(interpolated) * facing;
    surface_normal = normal;
    if ((material.flags & NORMAL_MAPPED) != 0) {
        // Square to the normal again: interpolation doesn't keep it so.
        float3 along = tangent.xyz * facing;
        along = normalize(along - normal * dot(normal, along));
        // Flipped with them, as glTF flips all three; their cross product alone isn't.
        float3 bitangent = cross(normal, along) * tangent.w * facing;
        // Z from the map's X and Y, all a BC5 map holds; then scaled, as glTF scales X and Y.
        float z = sqrt(saturate(1.0 - dot(mapped, mapped)));
        float3 local = float3(mapped * material.normal_scale, z);
        normal = normalize(along * local.x + bitangent * local.y + normal * local.z);
    }
    return normal;
}

// What the view shows in place of its light, `Frame::show`, numbered as `Channel` in src/show.rs:
// 0 its light; below `SHOW_DIRECT_LIGHT`, an input of shading (`surface_shown`); from there, a
// part of its light; past `SHOW_SPECULAR_LIGHT`, what `reflection` returns, as it says.
static const uint SHOW_LIGHT = 0;
static const uint SHOW_BASE_COLOR = 1;
static const uint SHOW_NORMAL = 2;
static const uint SHOW_SURFACE_NORMAL = 3;
static const uint SHOW_METALLIC = 4;
static const uint SHOW_ROUGHNESS = 5;
static const uint SHOW_OCCLUSION = 6;
static const uint SHOW_EMISSION = 7;
static const uint SHOW_SPECULAR_COLOR = 8;
static const uint SHOW_DIRECT_LIGHT = 9;
static const uint SHOW_SKY_LIGHT = 10;
static const uint SHOW_REFLECTION = 11;
static const uint SHOW_DIFFUSE_LIGHT = 12;
static const uint SHOW_SPECULAR_LIGHT = 13;
static const uint SHOW_TRACED_REFLECTION = 14;
static const uint SHOW_REFLECTION_RAYS = 15;

// What `material` reflects head-on, of base color `base` where its surface map holds `surface`: a
// specular-glossiness material's specular color, or 0.04 mixed with `base` by how metallic it is.
float3 specular_color(Material material, float3 base, float4 surface) {
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        return material.specular * surface.rgb;
    }
    return lerp(float3(0.04), base, saturate(material.metallic * surface.b));
}

// The input of shading that `show`, below `SHOW_DIRECT_LIGHT`, names, of `lit`'s arguments: colors
// as they are; values, and normals' axes from -1 to 1 as 0 to 1, by `stored_as`. An unlit
// material's base color, and black for the rest, which it doesn't read: a glTF one without
// normals has +Z everywhere.
float4 surface_shown(uint show, Material material, float3 interpolated, float4 tangent, bool front,
                     float4 color, float4 surface, float roughness, float2 mapped,
                     float occlusion, float3 emission) {
    if (show == SHOW_BASE_COLOR) {
        return color;
    }
    if ((material.flags & UNLIT) != 0) {
        return float4(0.0, 0.0, 0.0, color.a);
    }
    if (show == SHOW_EMISSION) {
        return float4(emission, color.a);
    }
    if (show == SHOW_SPECULAR_COLOR) {
        return float4(specular_color(material, color.rgb, surface), color.a);
    }
    float3 value;
    if (show == SHOW_NORMAL || show == SHOW_SURFACE_NORMAL) {
        float3 surface_normal;
        float3 normal = shading_normal(material, interpolated, tangent, front, mapped, surface_normal);
        value = (show == SHOW_NORMAL ? normal : surface_normal) * 0.5 + 0.5;
    } else if (show == SHOW_METALLIC) {
        bool metal_rough = (material.flags & SPECULAR_GLOSSINESS) == 0;
        value = float3(metal_rough ? saturate(material.metallic * surface.b) : 0.0);
    } else if (show == SHOW_ROUGHNESS) {
        value = float3(roughness);
    } else {
        value = float3(occlusion);
    }
    return float4(stored_as(value), color.a);
}

// What the lights shade at `world` of a point of `material` seen from `view`, with `lit`'s
// arguments (`base`, its color's RGB); `metallic`: how metallic it is, 0 for specular-glossiness.
Surface surface_of(Material material, float3 world, float3 view, float3 interpolated,
                   float4 tangent, bool front, float3 base, float4 surface, float roughness,
                   float2 mapped, out float metallic) {
    Surface out;
    out.world = world;
    out.normal = shading_normal(material, interpolated, tangent, front, mapped, out.surface_normal);
    out.view = view;
    out.n_dot_v = max(dot(out.normal, view), 1e-4);
    out.f0 = specular_color(material, base, surface);
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        metallic = 0.0;
        out.diffuse_color = base * (1.0 - max(max(out.f0.r, out.f0.g), out.f0.b));
    } else {
        metallic = saturate(material.metallic * surface.b);
        out.diffuse_color = base * (1.0 - metallic);
    }
    float alpha = roughness * roughness;
    out.alpha2 = alpha * alpha;
    out.specular_weight = float3(1.0);
    float lobe_roughness = max(roughness, LEAST_LOBE_ROUGHNESS);
    out.lobe_alpha = lobe_roughness * lobe_roughness;
    out.lobe = scattered(out.f0, roughness, out.n_dot_v, missed(out.n_dot_v, alpha))
        / (PI * missed_mean(lobe_roughness));
    return out;
}

// The color of a point of `material`, lit or unlit. `front`: whether its triangle shows the side
// its mesh winds counterclockwise, which a mirroring transform reverses. `color` (vertices' and
// factor's in), `surface`, `mapped` (X and Y, -1 to 1), `occlusion`, `emission`: its maps there.
// `roughness`: the surface's, as `filtered_roughness` widens it. Where `show` asks: an input
// (`surface_shown`), or a part of its light, exposed, unhazed, none for what's unlit.
float4 lit(Frame frame, Material material, float3 world, float3 interpolated, float4 tangent,
           bool front, float4 color, float4 surface, float roughness, float2 mapped,
           float occlusion, float3 emission, float4 reflection) {
    if (frame.show != SHOW_LIGHT && frame.show < SHOW_DIRECT_LIGHT) {
        return surface_shown(frame.show, material, interpolated, tangent, front, color, surface,
                             roughness, mapped, occlusion, emission);
    }
    if (frame.show > SHOW_SPECULAR_LIGHT) {
        bool unlit = (material.flags & UNLIT) != 0;
        return float4(unlit ? float3(0.0) : min(reflection.rgb, BRIGHTEST), color.a);
    }
    if ((frame.light_count == 0 && frame.specular_levels == 0) || (material.flags & UNLIT) != 0) {
        return frame.show == SHOW_LIGHT ? color : float4(0.0, 0.0, 0.0, color.a);
    }
    float3 view = normalize(frame.eye.xyz - world * frame.eye.w);
    float metallic;
    Surface lit = surface_of(material, world, view, interpolated, tangent, front, color.rgb,
                             surface, roughness, mapped, metallic);
    float3 normal = lit.normal;
    // For the sky's light, a dielectric's colors, which `sky_light` mixes with a metal's by how
    // metallic it is.
    bool glossy = (material.flags & SPECULAR_GLOSSINESS) != 0;
    float3 sky_diffuse = glossy ? lit.diffuse_color : color.rgb;
    float3 sky_f0 = glossy ? lit.f0 : float3(0.04);
    // Showing diffuse or specular light alone, the lights shade without the other.
    if (frame.show == SHOW_SPECULAR_LIGHT) {
        lit.diffuse_color = float3(0.0);
    }
    if (frame.show == SHOW_DIFFUSE_LIGHT) {
        lit.specular_weight = float3(0.0);
    }
    float3 light_sum = float3(0.0);
    Clusters clusters = frame.clusters;
    for (uint i = 0; i < clusters.everywhere; i++) {
        light_sum += reflected(frame, lit, i);
    }
    uint found = cluster(clusters, world);
    if (found == NO_CLUSTER) {
        for (uint i = clusters.everywhere; i < frame.light_count; i++) {
            light_sum += reflected(frame, lit, i);
        }
    } else {
        // Through the lights' address, which the bits follow (`Clusters` says why), by a pointer
        // made once: AMD's driver drops the address of an index past them made in the loop. A
        // change here needs AMD's ISA checked again, under both slangc releases the build may use.
        uint* at = (uint*)frame.lights + clusters.bits;
        uint first = at[2 * found];
        uint end = at[2 * found + 1];
        uint words = 2 * clusters.across * clusters.down * clusters.slices + found * clusters.words;
        for (uint word = first; word < end; word++) {
            uint bits = at[words + word];
            while (bits != 0) {
                uint bit = firstbitlow(bits);
                bits &= bits - 1;
                light_sum += reflected(frame, lit, clusters.everywhere + word * 32 + bit);
            }
        }
    }
    float n_dot_v = lit.n_dot_v;
    float3 direct = light_sum;
    // The view's reflection, already exposed (the trace takes it to this frame's exposure), so
    // neither the sky's luminance nor the exposure scales it: in place of its share of the sky's
    // specular light, times that light's reflectance.
    float3 mirrored = float3(0.0);
    float3 sky_diffused = float3(0.0);
    float3 sky_reflected = float3(0.0);
    if (frame.specular_levels > 0) {
        float3 reflectance;
        sky_diffused = sky_light(frame, sky_diffuse, sky_f0, color.rgb, metallic, roughness, normal,
                                 view, n_dot_v, occlusion, reflection.a, reflectance,
                                 sky_reflected);
        mirrored = reflection.rgb * reflection.a * reflectance;
    }
    float3 sky = sky_diffused + sky_reflected;
    if (frame.show != SHOW_LIGHT) {
        float3 part = frame.show == SHOW_DIRECT_LIGHT ? direct * frame.exposure
            : frame.show == SHOW_SKY_LIGHT ? sky * frame.exposure
            : frame.show == SHOW_DIFFUSE_LIGHT ? (direct + sky_diffused) * frame.exposure
            : frame.show == SHOW_SPECULAR_LIGHT ? (direct + sky_reflected) * frame.exposure + mirrored
            : mirrored;
        return float4(min(part, BRIGHTEST), color.a);
    }
    float3 shaded = hazed(frame, world, (direct + sky) * frame.exposure + mirrored + emission);
    return float4(min(shaded, BRIGHTEST), color.a);
}
