// flat_color.slang reading the view's size as two integers, where the engine pushes two floats,
// the frame's data as more than the engine's, and a push constant past the 112 bytes the engine
// pushes, which the contract refuses: nothing sets them.

struct Vertex {
    float3 position;
};

struct Params {
    float4 color;
};

struct Frame {
    float3 camera;
    float time;
    float exposure;
    float4 tint;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
    uint2 view;
    Frame* frame;
    float brightness;
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
    Targets out;
    float wide = float(push.view.x);
    out.color = push.params[input.instance].color * push.brightness * wide * push.frame->tint;
    out.id = input.id;
    return out;
}
