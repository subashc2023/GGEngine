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
// - The sky's prefiltered light by the split sum, with Fdez-Agüera's roughness-dependent Fresnel
//   and multiple scattering ("A Multiple-Scattering Microfacet Model for Real-Time Image-based
//   Lighting", 2019), as glTF-Sample-Renderer's ibl.glsl and pbr.frag have it (at df3e51cb).
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

// How much of the sky's light GGX reflects at a cosine of `n_dot_v`, by the split sum's `table`
// there: once, with roughness-dependent Fresnel, plus multiple scattering.
float3 sky_reflectance(float3 f0, float roughness, float n_dot_v, float2 table) {
    float3 rough = max(float3(1.0 - roughness), f0) - f0;
    float3 once = (f0 + rough * pow5(1.0 - n_dot_v)) * table.x + table.y;
    float missed = 1.0 - (table.x + table.y);
    float3 average = f0 + (1.0 - f0) / 21.0;
    return once + missed * once * average / (1.0 - average * missed);
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
// how much of a reflection's light it reflects.
float3 sky_light(Frame frame, float3 diffuse_color, float3 f0, float3 metal, float metallic,
                 float roughness, float3 normal, float3 view, float n_dot_v, float occlusion,
                 float taken, out float3 reflectance, out float3 specular_light) {
    SamplerState clamp_ = samplers[LINEAR_CLAMP];
    float3 diffuse = cubes[frame.irradiance].SampleLevel(clamp_, normal, 0.0).rgb * diffuse_color
        * occlusion;
    float level = roughness * float(frame.specular_levels - 1);
    float3 reflected = reflect(-view, normal);
    float3 specular = cubes[frame.specular].SampleLevel(clamp_, reflected, level).rgb
        * (specular_occlusion(n_dot_v, occlusion, roughness) * (1.0 - taken));
    float2 table = textures[frame.brdf].SampleLevel(clamp_, float2(n_dot_v, roughness), 0.0).rg;
    float3 dielectric_reflectance = sky_reflectance(f0, roughness, n_dot_v, table);
    float3 metal_reflectance = sky_reflectance(metal, roughness, n_dot_v, table);
    reflectance = lerp(dielectric_reflectance, metal_reflectance, metallic);
    specular_light = specular * reflectance * frame.sky_luminance;
    return diffuse * ((1.0 - dielectric_reflectance) * (1.0 - metallic)) * frame.sky_luminance;
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
    // 1, or 0 where the view shows diffuse light alone; a diffuse color of 0 shows specular alone.
    float specular_weight;
};

// The light the frame's `i`th light reflects from `surface` toward the view, before the exposure.
float3 reflected(Frame frame, Surface surface, uint i) {
    Light light = frame.lights[i];
    float3 radiance = light.color;
    float3 to_light;
    if (light.kind == 0) {
        to_light = -light.direction;
    } else {
        float3 between = light.position - surface.world;
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
    float3 normal = surface.normal;
    float n_dot_l = dot(normal, to_light);
    if (n_dot_l <= 0.0) {
        return float3(0.0);
    }
    if (i == frame.shadows.light) {
        radiance *= shadowed(frame.shadows, surface.world, surface.surface_normal, to_light);
    }
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
    return (diffuse + specular) * radiance * n_dot_l;
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

// The share of `Reflected::roughness`, below it, over which the view's reflections fade into the
// sky's specular light.
static const float ROUGHNESS_FADE = 0.25;

// The view's reflection at `pixel` (reflections.slang): RGB the light its mirror ray found,
// exposed, blurred by the lobe; alpha the share of the sky's specular light it replaces, fading out
// toward `Reflected::roughness`; 0 for none. The lobe: a cone of the angle Uludag gives Blinn-Phong
// of the power matching GGX's alpha ("Hi-Z Screen-Space Cone-Traced Reflections", GPU Pro 5,
// 2014), read at the level as wide as its footprint where the ray hit.
float4 reflection(Frame frame, uint2 pixel, float roughness) {
    Reflected reflected = frame.reflections;
    if (reflected.colors == 0) {
        return float4(0.0);
    }
    float fade = saturate((reflected.roughness - roughness) / (reflected.roughness * ROUGHNESS_FADE));
    float alpha = roughness * roughness;
    float power = 2.0 / (alpha * alpha) - 2.0;
    float cosine = pow(0.244, 1.0 / (power + 1.0));
    float tangent = sqrt(max(1.0 - cosine * cosine, 0.0)) / cosine;
    float spread = textures[reflected.spread].Load(int3(int2(pixel), 0)).r;
    float level = log2(max(2.0 * spread * tangent, 1.0));
    Texture2D colors = textures[reflected.colors];
    uint width;
    uint height;
    colors.GetDimensions(width, height);
    float2 uv = (float2(pixel) + 0.5) / float2(width, height);
    float4 found = colors.SampleLevel(samplers[LINEAR_CLAMP], uv, level);
    if (!(found.a > 0.0)) {
        return float4(0.0);
    }
    return float4(found.rgb / found.a, found.a * fade);
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
// part of its light.
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

// What `material` reflects head-on, of base color `base` where its surface map holds `surface`: a
// specular-glossiness material's specular color, or 0.04 mixed with `base` by how metallic it is.
float3 specular_color(Material material, float3 base, float4 surface) {
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        return material.specular * surface.rgb;
    }
    return lerp(float3(0.04), base, saturate(material.metallic * surface.b));
}

// The input of shading that `show`, below `SHOW_DIRECT_LIGHT`, names, of `lit`'s arguments: colors
// as they are; values, and normals' axes from -1 to 1 as 0 to 1, by `stored_as`.
float4 surface_shown(uint show, Material material, float3 interpolated, float4 tangent, bool front,
                     float4 color, float4 surface, float2 mapped, float occlusion,
                     float3 emission) {
    if (show == SHOW_BASE_COLOR) {
        return color;
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
        value = float3(roughness_of(material, surface));
    } else {
        value = float3(occlusion);
    }
    return float4(stored_as(value), color.a);
}

// The color of a point of `material`, lit or unlit. `front`: whether its triangle shows the side
// its mesh winds counterclockwise, which a mirroring transform reverses. `color` (vertices' and
// factor's in), `surface`, `mapped` (X and Y, -1 to 1), `occlusion`, `emission`: its maps there.
// Where `show` asks: an input (`surface_shown`), or a part of its light, exposed, unhazed, none for
// what's unlit.
float4 lit(Frame frame, Material material, float3 world, float3 interpolated, float4 tangent,
           bool front, float4 color, float4 surface, float2 mapped, float occlusion,
           float3 emission, float4 reflection) {
    if (frame.show != SHOW_LIGHT && frame.show < SHOW_DIRECT_LIGHT) {
        return surface_shown(frame.show, material, interpolated, tangent, front, color, surface,
                             mapped, occlusion, emission);
    }
    if ((frame.light_count == 0 && frame.specular_levels == 0) || (material.flags & UNLIT) != 0) {
        return frame.show == SHOW_LIGHT ? color : float4(0.0, 0.0, 0.0, color.a);
    }
    float3 surface_normal;
    float3 normal = shading_normal(material, interpolated, tangent, front, mapped, surface_normal);
    // What it diffuses and reflects head-on, for the lights; for the sky's light, a dielectric's,
    // which `sky_light` mixes with a metal's by how metallic it is.
    float metallic = 0.0;
    float roughness = roughness_of(material, surface);
    float3 diffuse_color;
    float3 f0 = specular_color(material, color.rgb, surface);
    float3 sky_diffuse;
    float3 sky_f0;
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        diffuse_color = color.rgb * (1.0 - max(max(f0.r, f0.g), f0.b));
        sky_diffuse = diffuse_color;
        sky_f0 = f0;
    } else {
        metallic = saturate(material.metallic * surface.b);
        diffuse_color = color.rgb * (1.0 - metallic);
        sky_diffuse = color.rgb;
        sky_f0 = float3(0.04);
    }
    float alpha = roughness * roughness;
    float3 view = normalize(frame.eye.xyz - world * frame.eye.w);
    Surface lit;
    lit.world = world;
    lit.normal = normal;
    lit.surface_normal = surface_normal;
    lit.view = view;
    lit.n_dot_v = max(dot(normal, view), 1e-4);
    // Showing diffuse or specular light alone, the lights shade without the other.
    lit.diffuse_color = frame.show == SHOW_SPECULAR_LIGHT ? float3(0.0) : diffuse_color;
    lit.f0 = f0;
    lit.alpha2 = alpha * alpha;
    lit.specular_weight = frame.show == SHOW_DIFFUSE_LIGHT ? 0.0 : 1.0;
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
