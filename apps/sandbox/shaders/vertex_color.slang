// Meshes in the colors of their vertices, blended across each triangle, whose positions are in the
// mesh's own space: `Colored` in main.rs, which the struct here matches field for field. The
// colors are linear, and the sRGB target encodes them.

struct Vertex {
    float3 position;
    float4 color;
};

// What the engine pushes for each draw: the matrix from the mesh's space to clip space, which is
// the camera's view and projection times the draw's transform, then the address of the mesh's
// vertices.
struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float4 color;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    Vertex vertex = push.vertices[index];
    Varyings out;
    out.position = mul(push.clip_from_mesh, float4(vertex.position, 1.0));
    out.color = vertex.color;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    return input.color;
}
