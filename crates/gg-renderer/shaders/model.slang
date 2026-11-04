// Models' meshes, shaded as glTF's metallic-roughness materials are, by every light of the frame;
// or, in a frame without lights and for a material that's unlit, drawn unlit, as
// KHR_materials_unlit defines it: each fragment its material's base color times its texture's,
// sampled at the vertices' texture coordinates, times the vertices' color. Each instance is a mesh
// with its own transform, ID and material, writing its ID where it shows. `ModelVertex` in
// src/model.rs, which `Vertex` here matches field for field, and `Params`, which `Params` here
// matches byte for byte; the material itself is in the table the frame's data points at.
//
// Shading follows glTF 2.0's Appendix B, as Khronos's sample viewer implements it: a Lambertian
// diffuse lobe and a GGX specular lobe, with Smith's height-correlated visibility, Schlick's
// Fresnel, and 0.04 reflected head-on by every dielectric. Lights fall off as
// KHR_lights_punctual's reference does. Their light, times the frame's exposure, then the
// material's emission, as the view shows it, make the color. The occlusion map darkens only light
// from all around, of which there's none until image-based lighting.
//
// As it is, it's for materials that blend by their alpha, and discards what's fully transparent,
// which blending would leave as it was, so that a pick there finds what's behind it; it writes no
// depth, so it keeps early depth testing. model_opaque.slang and model_masked.slang include it
// with OPAQUE or MASKED defined, for materials whose alpha glTF ignores, or uses only to cut out
// what's less opaque than their cutoff: both are drawn fully opaque, so they return an alpha of 1.
// Of those, only the masked one discards, since a shader that can discard while it writes depth
// loses early depth testing on some GPUs.

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

[shader("fragment")]
Targets fragment_main(Varyings input, bool front: SV_IsFrontFace) {
    Frame frame = *push.frame;
    Material material = frame.materials[push.params[input.instance].material];
    // Every texture sampled before any branch or discard, which would leave the derivatives that
    // pick their levels undefined. Base colors decode from sRGB as they're sampled, as the
    // emission's do; the maps are read as stored.
    float4 color = sampled(material, 0, input.uv) * material.color * input.color;
    float4 surface = sampled(material, 1, input.uv);
    float2 mapped = sampled(material, 2, input.uv).xy * 2.0 - 1.0;
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
    if (frame.light_count == 0 || (material.flags & UNLIT) != 0) {
        out.color = color;
        return out;
    }
    // A back face seen of a double-sided material faces the other way, its tangent space too.
    float facing = front ? 1.0 : -1.0;
    float3 normal = normalize(input.normal) * facing;
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
    float metallic = saturate(material.metallic * surface.b);
    float roughness = clamp(material.roughness * surface.g, LEAST_ROUGHNESS, 1.0);
    float alpha = roughness * roughness;
    float alpha2 = alpha * alpha;
    float3 diffuse_color = color.rgb * (1.0 - metallic);
    float3 f0 = lerp(float3(0.04), color.rgb, metallic);
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
    float3 shaded = light_sum * frame.exposure + emission;
    out.color = float4(min(shaded, BRIGHTEST), color.a);
    return out;
}
