// Meshes in the colors of their vertices, blended across each triangle, whose positions are in the
// mesh's own space: `Colored` in main.rs, which the struct here matches field for field. The
// colors are linear, and the sRGB target encodes them.

struct Vertex {
    float3 position;
    float4 color;
};

// What the engine pushes for each instanced draw: the matrix from the world to clip space, which
// is the camera's view and projection, then the addresses of the mesh's vertices, of the
// instances' transforms, each from the mesh's space to the world's, and of their IDs, each its
// entity's or 0. It takes no parameters, so it leaves out their address, which comes last.
struct Push {
    float4x4 clip_from_world;
    Vertex* vertices;
    float4x4* transforms;
    uint2* ids;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float4 color;
    nointerpolation uint2 id;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Vertex vertex = push.vertices[index];
    float4 world = mul(push.transforms[instance], float4(vertex.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.color = vertex.color;
    out.id = push.ids[instance];
    return out;
}

// The color, and the instance's ID, which picks read back where it shows.
struct Targets {
    float4 color : SV_Target0;
    uint2 id : SV_Target1;
};

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Targets out;
    out.color = input.color;
    out.id = input.id;
    return out;
}
