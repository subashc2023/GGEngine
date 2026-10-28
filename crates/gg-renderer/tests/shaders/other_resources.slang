// flat_color.slang binding a buffer and a storage image of its own, which the contract refuses:
// shaders bind only the bindless set's textures and samplers.

struct Vertex {
    float3 position;
};

struct Params {
    float4 color;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[[vk::binding(2, 0)]] StructuredBuffer<float4> tints;
[[vk::binding(0, 1)]] RWTexture2D<float4> canvas;

struct Varyings {
    float4 position : SV_Position;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4 world = mul(push.transforms[instance], float4(push.vertices[index].position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
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
    float4 color = push.params[input.instance].color * tints[input.instance];
    Targets out;
    out.color = color + canvas.Load(int2(input.position.xy));
    out.id = input.id;
    return out;
}
