// flat_color.slang writing to the mesh's vertices through their address, which the contract
// refuses: stores from the vertex and fragment stages need features the engine doesn't enable,
// and every instance of a draw reads the same vertices.

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
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float3 position = push.vertices[index].position;
    push.vertices[index].position = position * 2.0;
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
