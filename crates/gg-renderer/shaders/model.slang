// Models' meshes, drawn unlit, as glTF's KHR_materials_unlit defines it: each fragment is its
// material's color times its texture's, sampled at the vertices' texture coordinates, times the
// vertices' color. Each instance is a mesh with its own transform, ID and material, writing its ID
// where it shows. `ModelVertex` in src/model.rs, which `Vertex` here matches field for field, and
// `Params`, which `Material` here matches byte for byte.
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
};

struct Material {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    uint texture;
    uint sampler;
    float cutoff;
};

[[vk::push_constant]] ConstantBuffer<Push<Vertex, Material>> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
    float4 color;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Vertex vertex = push.vertices[index];
    float4 world = mul(push.transforms[instance], float4(vertex.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.uv = vertex.uv;
    out.color = vertex.color;
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Material material = push.params[input.instance];
    // The instances of one draw can carry different textures and samplers. Decoded from sRGB to
    // linear as it's sampled, and the sRGB target encodes it again.
    Texture2D texture = textures[NonUniformResourceIndex(material.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(material.sampler)];
    // Sampled before any discard, which would leave the derivatives that pick its level undefined.
    float4 color = texture.Sample(sampler, input.uv) * material.color * input.color;
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
    out.color = color;
    out.id = input.id;
    return out;
}
