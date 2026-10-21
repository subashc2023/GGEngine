// Meshes in the one color each draw gives, whose vertices are positions in the mesh's own space:
// `Position` in main.rs, which the struct here matches field for field.

struct Vertex {
    float3 position;
};

// What each draw gives it: `Tint` in main.rs.
struct Tint {
    // Linear RGBA, which the sRGB target encodes.
    float4 color;
};

// What the engine pushes for each draw: the matrix from the mesh's space to clip space, which is
// the camera's view and projection times the draw's transform, then the addresses of the mesh's
// vertices and of the draw's tint.
struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
    Tint* tint;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    return mul(push.clip_from_mesh, float4(push.vertices[index].position, 1.0));
}

[shader("fragment")]
float4 fragment_main() : SV_Target {
    return push.tint->color;
}
