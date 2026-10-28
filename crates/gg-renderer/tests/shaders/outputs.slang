// flat_color.slang returning a float3 color, and writing a third target the view doesn't have,
// both of which the contract refuses.

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
    float3 color : SV_Target0;
    uint2 id : SV_Target1;
    float4 glow : SV_Target2;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    float4 color = push.params[input.instance].color;
    Targets out;
    out.color = color.rgb;
    out.id = input.id;
    out.glow = color;
    return out;
}
