// textured.slang sampling textures of integers, and its samplers at another binding than the
// bindless set has them, both of which the contract refuses.

[[vk::binding(0, 0)]] Texture2D<uint> textures[];
[[vk::binding(2, 0)]] SamplerState samplers[];

struct Vertex {
    float3 position;
    float2 uv;
};

struct Params {
    uint texture;
    uint sampler;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
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
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Params params = push.params[input.instance];
    Texture2D<uint> texture = textures[NonUniformResourceIndex(params.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(params.sampler)];
    Targets out;
    out.color = float4(texture.Load(int3(int2(input.uv * 8.0), 0)), 0.0, 0.0, 1.0)
        + float4(textures[0].GatherRed(sampler, input.uv));
    out.id = input.id;
    return out;
}
