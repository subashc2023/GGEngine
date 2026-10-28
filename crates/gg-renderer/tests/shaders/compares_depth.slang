// textured.slang sampling with a depth comparison, through samplers declared to compare, which the
// contract refuses: the bindless set's textures hold colors, and its samplers compare nothing.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerComparisonState samplers[];

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
    Texture2D texture = textures[NonUniformResourceIndex(params.texture)];
    SamplerComparisonState sampler = samplers[NonUniformResourceIndex(params.sampler)];
    float lit = texture.SampleCmp(sampler, input.uv, 0.5);
    Targets out;
    out.color = float4(lit, lit, lit, 1.0);
    out.id = input.id;
    return out;
}
