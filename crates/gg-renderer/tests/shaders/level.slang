// Meshes in one mip level of a texture, drawn by its alpha over what's behind them, whose vertices
// are positions in the mesh's own space and texture coordinates; each draw gives the texture, its
// sampler and the level, which it samples whatever the mesh's size on screen.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Vertex {
    float3 position;
    float2 uv;
};

struct Params {
    uint texture;
    uint sampler;
    float level;
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
    SamplerState sampler = samplers[NonUniformResourceIndex(params.sampler)];
    float4 color = texture.SampleLevel(sampler, input.uv, params.level);
    // What's fully transparent, which picks see through.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = input.id;
    return out;
}
