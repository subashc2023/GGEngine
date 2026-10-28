// A shader that takes 8 bytes of parameters, declaring its instances' IDs after their address
// rather than before, which the contract refuses: it would read the IDs as its parameters, and its
// parameters as the IDs, whose sizes alone are the same.

struct Vertex {
    float3 position;
};

struct Params {
    uint red;
    uint green;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    Params* params;
    uint2* ids;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

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
    Params params = push.params[input.instance];
    Targets out;
    out.color = float4(float(params.red) / 255.0, float(params.green) / 255.0, 0.0, 1.0);
    out.id = input.id;
    return out;
}
