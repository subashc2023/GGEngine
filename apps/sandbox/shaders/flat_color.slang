// Meshes in one color, whose vertices are positions in the mesh's own space: `Position` in
// main.rs, which the struct here matches field for field.

struct Vertex {
    float3 position;
};

// What the engine pushes for each draw: the matrix from the mesh's space to clip space, which is
// the camera's view and projection times the draw's transform, then the address of the mesh's
// vertices.
struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    return mul(push.clip_from_mesh, float4(push.vertices[index].position, 1.0));
}

[shader("fragment")]
float4 fragment_main() : SV_Target {
    // Linear RGBA, which the sRGB target encodes.
    return float4(0.2, 0.3, 0.8, 1.0);
}
