// flat_color.slang reading a push constant past the 88 bytes the engine pushes, which the
// contract refuses: nothing sets it.

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
    float brightness;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    nointerpolation uint instance;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4 world = mul(push.transforms[instance], float4(push.vertices[index].position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.instance = instance;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    return push.params[input.instance].color * push.brightness;
}
