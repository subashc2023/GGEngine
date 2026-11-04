// Meshes that show the frame's data the engine pushes, as colors: left of the camera, its position,
// and right of it, the time and the exposure, as red and green. It takes parameters it doesn't
// read, as a shader that reads the frame's data does, whose address comes after theirs.

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
};

struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
    Params* params;
    float2 view;
    Frame* frame;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    // How far right of the camera the point is, in the world.
    float right;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    float4 world = mul(push.transforms[instance], float4(push.vertices[index].position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.right = world.x - push.frame->camera.x;
    return out;
}

struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Frame frame = *push.frame;
    Targets out;
    if (input.right < 0.0) {
        out.color = float4(frame.camera, 1.0);
    } else {
        out.color = float4(frame.time, frame.exposure, 0.0, 1.0);
    }
    out.id = uint2(0, 0);
    return out;
}
