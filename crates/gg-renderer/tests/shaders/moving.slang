// flat_color.slang returning how far each point it draws moved in the world since the frame
// before, by its instance's transform then, as the contract lets a game's shader.

struct Vertex {
    float3 position;
};

struct Params {
    // Linear, as the view's colors are.
    float4 color;
};

// As much of the frame's data as reaches the push constants past it, which it doesn't read.
struct Frame {
    float3 camera;
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
    float2 view;
    Frame* frame;
    uint2* meshlets;
    uint draw;
    // How many transforms past each instance's its transform the frame before is.
    uint previous;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // Where it is less where it was.
    float3 moved;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4 position = float4(push.vertices[index].position, 1.0);
    float4 world = mul(push.transforms[instance], position);
    float4 was = mul(push.transforms[instance + push.previous], position);
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.moved = world.xyz - was.xyz;
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
    float4 motion : SV_Target2;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Targets out;
    out.color = push.params[input.instance].color;
    out.id = input.id;
    out.motion = float4(input.moved, 0.0);
    return out;
}
