// flat_color.slang reading SV_VertexID, which needs a feature the engine doesn't enable, and a
// vertex attribute, which the pipeline gives nothing, both of which the contract refuses.

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
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    nointerpolation uint instance;
};

[shader("vertex")]
Varyings vertex_main(
    uint index: SV_VertexID,
    float3 offset: OFFSET,
    uint instance: SV_VulkanInstanceID
) {
    float3 position = push.vertices[index].position + offset;
    float4 world = mul(push.transforms[instance], float4(position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.instance = instance;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    return push.params[input.instance].color;
}
