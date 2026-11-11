// How models' meshes are shaded, which model.slang's fragments and visibility.slang's pixels share:
// as glTF's metallic-roughness materials are, by the frame's lights and its sky; or, in a frame
// without either and for a material that's unlit, unlit, as KHR_materials_unlit defines it, its
// material's base color times its texture's, sampled at the vertices' texture coordinates, times
// the vertices' color. A mesh's vertices are packed, as src/packed.rs packs them, behind a header
// that says how they unpack; the material itself is in the table the frame's data points at.
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
// which the occlusion map and the view's ambient occlusion darken, as they darken nothing else, by
// `ambient`: its diffuse light by that, and its specular light by the specular occlusion Lagarde and
// de Rousiers derive from it and the way to the eye. Where the view's reflections found what a
// point's mirror shows, they take the place of the sky's specular light, by `reflection`. The light
// that casts the frame's
// shadows lights only what its shadow maps see of it. Their light, times the frame's exposure,
// then the material's emission, as the view shows it, make the color, which an atmosphere's haze
// dims and adds its light to, by how far it is (aerial.slang).
//
// Each point shades by the lights that light everywhere, then by those binned into its cluster
// of the game's view, as clusters.slang bins them, each in the frame's order: those a cluster
// leaves out reach none of it, so they'd add nothing. Where it's in no cluster, as what the canvas
// draws under its own projection may be, or the frame has none, it shades by every light, in the
// same order.

#include "mesh.slang"
#include "aerial.slang"

// A vertex as a model's mesh holds it, `PackedVertex` in src/packed.rs, read as words: its X and Y;
// its Z and its bitangent's sign; its normal; its tangent; its texture coordinates; and its color.
struct PackedVertex {
    uint position_xy;
    uint position_z_sign;
    uint normal;
    uint tangent;
    uint uv;
    uint color;
};

// What a mesh's buffer starts with, in `HEADER` vertices' room, `Header` in src/packed.rs: how its
// vertices unpack, and where its meshlets' parts start and its skin, as skin.slang reads it, in
// words from the buffer's start.
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

// A vertex unpacked.
struct Vertex {
    float3 position;
    float2 uv;
    // Linear RGBA.
    float4 color;
    float3 normal;
    // Its W the sign of the bitangent, cross(normal, tangent) * W.
    float4 tangent;
};

// The low and the high halves of `word`, each a signed 16-bit fraction of 1.
float2 snorm_halves(uint word) {
    int2 halves = int2(int(word << 16) >> 16, int(word) >> 16);
    return max(float2(halves) / 32767.0, -1.0);
}

// The unit vector `word` holds folded onto an octahedron, as `octahedral` in src/packed.rs folds
// it, unfolded.
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

// `packed`, a vertex of a mesh whose buffer starts with `header`, unpacked as it says.
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

// A vertex's normal and tangent, in the world, and -1 where its transform mirrors it, 1 elsewhere.
struct Turned {
    float3 normal;
    float4 tangent;
    float side;
};

// `vertex`'s normal and tangent, as `transform` turns them into the world: the normal not of unit
// length, as interpolation leaves it anyway.
Turned turned(Vertex vertex, float4x4 transform) {
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
    Turned out;
    out.normal = normal * side;
    out.tangent = float4(mul(linear, vertex.tangent.xyz), vertex.tangent.w * side);
    out.side = side;
    return out;
}

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

// What `material`'s texture `which` holds at `uv`, by its sampler: the instances of one draw can
// carry different materials.
float4 sampled(Material material, uint which, float2 uv) {
    uint2 at = material.textures[which];
    Texture2D texture = textures[NonUniformResourceIndex(at.x)];
    return texture.Sample(samplers[NonUniformResourceIndex(at.y)], uv);
}

// What `sampled` gives, of a texture coordinate that changes by `across` a pixel to the right and
// by `down` a pixel down, which pick its level, where nothing finds their derivatives.
float4 sampled_by(Material material, uint which, float2 uv, float2 across, float2 down) {
    uint2 at = material.textures[which];
    Texture2D texture = textures[NonUniformResourceIndex(at.x)];
    return texture.SampleGrad(samplers[NonUniformResourceIndex(at.y)], uv, across, down);
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

// How much of the sky's specular light reaches a surface of `roughness`, seen at a cosine of
// `n_dot_v`, of which `occlusion` of its light from all around does: Lagarde and de Rousiers'
// ("Moving Frostbite to Physically Based Rendering", 2014, listing 26), of GGX's alpha, as Filament
// takes it. A rough lobe, which gathers from all around as the diffuse does, takes `occlusion`; a
// smooth one less where it looks along the surface, toward what hides its light, and more head-on.
float specular_occlusion(float n_dot_v, float occlusion, float roughness) {
    float alpha = roughness * roughness;
    return saturate(pow(n_dot_v + occlusion, exp2(-16.0 * alpha - 1.0)) - 1.0 + occlusion);
}

// The sky's light from all around reflected by a surface of `roughness`, facing `normal` and seen
// along `view` at a cosine of `n_dot_v`, as luminance before the exposure: a surface that diffuses
// `diffuse_color` and reflects `f0` head-on, mixed by `metallic` with a metal of `metal` color, of
// which `occlusion` of the light reaches it, its specular light by `specular_occlusion`, and that
// by 1 less `taken`, the share of it the view's reflection takes the place of. And, in
// `reflectance`, how much of the light a reflection brings it reflects: the dielectric's specular
// reflectance and the metal's, mixed by `metallic`, as its specular light is.
float3 sky_light(Frame frame, float3 diffuse_color, float3 f0, float3 metal, float metallic,
                 float roughness, float3 normal, float3 view, float n_dot_v, float occlusion,
                 float taken, out float3 reflectance) {
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
    float3 dielectric = lerp(diffuse, specular, dielectric_reflectance);
    float3 metal_light = metal_reflectance * specular;
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

// What a point's lights shade: where it is, which way it faces, as its normal map has it and
// before, where it's seen from, and its material's terms.
struct Surface {
    float3 world;
    float3 normal;
    // Before its normal map, where its shadows are sampled: the map's bumps cast none.
    float3 surface_normal;
    float3 view;
    float n_dot_v;
    // What it diffuses and reflects head-on, and its roughness squared, squared.
    float3 diffuse_color;
    float3 f0;
    float alpha2;
};

// The light that the frame's `i`th light reflects toward the view from `surface`, before the
// exposure.
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
        // The inverse square, eased to 0 at the range by the window KHR_lights_punctual's README
        // recommends, max(min(1 - (distance / range)^4, 1), 0), not squared.
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
    float3 specular = fresnel * distribution * visibility;
    float3 diffuse = (1.0 - fresnel) * surface.diffuse_color / PI;
    return (diffuse + specular) * radiance * n_dot_l;
}

// How much of the light from all around reaches what the game's view shows at `pixel`, as its
// ambient occlusion has it, with `occlusion`, a material's map's: the less of the two, as both
// estimate how much of the same light reaches the point, at their own scales, where their product
// would darken twice where both see the same occluder, as Filament takes them.
float ambient(Frame frame, uint2 pixel, float occlusion) {
    if (frame.ambient_occlusion == 0) {
        return occlusion;
    }
    return min(occlusion, textures[frame.ambient_occlusion].Load(int3(int2(pixel), 0)).r);
}

// How rough a point of `material` is, its `surface`'s map's there, as `lit` shades it.
float roughness_of(Material material, float4 surface) {
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        return clamp(1.0 - material.glossiness * surface.a, LEAST_ROUGHNESS, 1.0);
    }
    return clamp(material.roughness * surface.g, LEAST_ROUGHNESS, 1.0);
}

// The share of the roughness past which a surface takes the sky's specular light alone, below it,
// over which the view's reflections fade into it.
static const float ROUGHNESS_FADE = 0.25;

// The view's reflection at `pixel`, as reflections.slang traced it, for a surface of `roughness`
// there: the light its mirror ray found, exposed, in RGB, blurred as the lobe spreads it, and how
// much of the sky's specular light it takes the place of, in alpha, faded out toward the roughness
// past which it takes none; 0 where its ray found nothing, and in a frame without reflections. The
// lobe is a cone, of the angle Uludag gives Blinn-Phong's of the power GGX's alpha matches
// ("Hi-Z Screen-Space Cone-Traced Reflections", GPU Pro 5, 2014), whose footprint where the ray
// found what it reflects is as wide, on screen, as the level of the reflections it reads.
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

// Where a point is in no cluster.
static const uint NO_CLUSTER = 0xFFFFFFFF;

// The index of the cluster of the game's view that `world` is in, as `Clusters` has them; or
// `NO_CLUSTER` in a frame without clusters, or for a point the view doesn't see.
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
    // Written so that NaN, as a point behind a perspective's eye gives, is outside too.
    bool inside = w > 0.0 && tile.x >= 0.0 && tile.y >= 0.0 && tile.x < float(clusters.across)
        && tile.y < float(clusters.down) && slice >= 0.0;
    if (!inside) {
        return NO_CLUSTER;
    }
    uint3 at = uint3(uint2(tile), min(uint(slice), clusters.slices - 1));
    return (at.z * clusters.down + at.y) * clusters.across + at.x;
}

// Which way a point of `material` faces, as `lit` shades it: its `interpolated` normal, turned to
// face the view where its triangle shows its back, as a double-sided material's does, then bent by
// its normal map, whose X and Y are `mapped`, from -1 to 1, along its `tangent`, if it has one; and
// in `surface_normal`, which way it faces before its normal map.
float3 shading_normal(Material material, float3 interpolated, float4 tangent, bool front,
                      float2 mapped, out float3 surface_normal) {
    // A back face seen of a double-sided material faces the other way, its tangent space too.
    float facing = front ? 1.0 : -1.0;
    float3 normal = normalize(interpolated) * facing;
    surface_normal = normal;
    if ((material.flags & NORMAL_MAPPED) != 0) {
        // Kept square to the normal, which interpolation doesn't keep them.
        float3 along = tangent.xyz * facing;
        along = normalize(along - normal * dot(normal, along));
        // Flipped with them, as glTF flips all three, which their cross product alone isn't.
        float3 bitangent = cross(normal, along) * tangent.w * facing;
        // Z from X and Y as the map holds them, which a BC5 map holds alone; then scaled, as
        // glTF scales a normal map's X and Y.
        float z = sqrt(saturate(1.0 - dot(mapped, mapped)));
        float3 local = float3(mapped * material.normal_scale, z);
        normal = normalize(along * local.x + bitangent * local.y + normal * local.z);
    }
    return normal;
}

// What the view shows in place of its light, `Frame::show`, as `Channel` in src/show.rs numbers it:
// at 0, its light; below `SHOW_DIRECT_LIGHT`, a part of what shading takes, as `surface_shown`
// gives it; from there, a part of the light it makes.
static const uint SHOW_LIGHT = 0;
static const uint SHOW_BASE_COLOR = 1;
static const uint SHOW_NORMAL = 2;
static const uint SHOW_SURFACE_NORMAL = 3;
static const uint SHOW_METALLIC = 4;
static const uint SHOW_ROUGHNESS = 5;
static const uint SHOW_OCCLUSION = 6;
static const uint SHOW_EMISSION = 7;
static const uint SHOW_DIRECT_LIGHT = 8;
static const uint SHOW_SKY_LIGHT = 9;
static const uint SHOW_REFLECTION = 10;

// `value`, each channel from 0 to 1, as the view's colors hold it so that the view's sRGB image,
// which the tonemap pass writes them into clamped, stores it as its byte over 255: decoded from
// sRGB, which the image encodes it in again.
float3 stored_as(float3 value) {
    float3 v = saturate(value);
    return select(v <= 0.04045, v / 12.92, pow((v + 0.055) / 1.055, 2.4));
}

// What the view shows of a point in place of its light, the part of what shading takes that
// `show`, below `SHOW_DIRECT_LIGHT`, names, as `lit` takes its arguments: colors as they are, and
// values, of normals each axis from -1 to 1 as 0 to 1, as `stored_as` stores them.
float4 surface_shown(uint show, Material material, float3 interpolated, float4 tangent, bool front,
                     float4 color, float4 surface, float2 mapped, float occlusion,
                     float3 emission) {
    if (show == SHOW_BASE_COLOR) {
        return color;
    }
    if (show == SHOW_EMISSION) {
        return float4(emission, color.a);
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

// The color of a point of `material`, as the frame lights it, or unlit: where it is in the world,
// `world`, its `normal` and `tangent` as interpolated, whether its triangle shows its front, the
// side its corners wind counterclockwise from in its mesh, which a transform that mirrors shows
// winding the other way, and what its maps hold there: its base `color`, its vertices' and its
// factor's in, its `surface`'s map, its normal map's X and Y, from -1 to 1, its occlusion and its
// emission; and the view's `reflection` there, as `reflection` gives it, or 0 for none. Or what the
// frame's `show` shows in its place: a part of what it takes, as `surface_shown` gives it; or a
// part of its light, exposed, without the haze, none for what's unlit.
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
    // Which way the surface faces where its shadows are sampled: the normal map's bumps cast none.
    float3 surface_normal;
    float3 normal = shading_normal(material, interpolated, tangent, front, mapped, surface_normal);
    // What it diffuses and reflects head-on, which the frame's lights shade; and for the sky's
    // light, a dielectric's, which it mixes with a metal's by how metallic it is.
    float metallic = 0.0;
    float roughness = roughness_of(material, surface);
    float3 diffuse_color;
    float3 f0;
    float3 sky_diffuse;
    float3 sky_f0;
    if ((material.flags & SPECULAR_GLOSSINESS) != 0) {
        f0 = material.specular * surface.rgb;
        diffuse_color = color.rgb * (1.0 - max(max(f0.r, f0.g), f0.b));
        sky_diffuse = diffuse_color;
        sky_f0 = f0;
    } else {
        metallic = saturate(material.metallic * surface.b);
        diffuse_color = color.rgb * (1.0 - metallic);
        f0 = lerp(float3(0.04), color.rgb, metallic);
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
    lit.diffuse_color = diffuse_color;
    lit.f0 = f0;
    lit.alpha2 = alpha * alpha;
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
        // Read through the lights' address, which they follow, as `Clusters` says why; past them
        // by a pointer made once, as AMD's driver drops the address of an index past them made in
        // the loop. A change here needs AMD's ISA of it checked again, under both slangc releases
        // the build may use.
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
    // The view's reflection, exposed already, as the colors it was found in are, the trace taking
    // them to this frame's exposure, so that neither the sky's luminance nor the exposure scales
    // it here: in place of the share of the sky's specular
    // light it takes, as much of it as that light's reflectance reflects.
    float3 mirrored = float3(0.0);
    if (frame.specular_levels > 0) {
        float3 reflectance;
        light_sum += sky_light(frame, sky_diffuse, sky_f0, color.rgb, metallic, roughness, normal,
                               view, n_dot_v, occlusion, reflection.a, reflectance);
        mirrored = reflection.rgb * reflection.a * reflectance;
    }
    if (frame.show != SHOW_LIGHT) {
        float3 part = frame.show == SHOW_DIRECT_LIGHT ? direct * frame.exposure
            : frame.show == SHOW_SKY_LIGHT ? (light_sum - direct) * frame.exposure
            : mirrored;
        return float4(min(part, BRIGHTEST), color.a);
    }
    float3 shaded = hazed(frame, world, light_sum * frame.exposure + mirrored + emission);
    return float4(min(shaded, BRIGHTEST), color.a);
}
