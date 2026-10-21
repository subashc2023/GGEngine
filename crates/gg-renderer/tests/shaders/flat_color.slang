// Meshes in the one color each draw gives, whose vertices are positions in the mesh's own space.

struct Vertex {
    float3 position;
};

struct Params {
    // Linear, and the sRGB target encodes it.
    float4 color;
};

struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    return mul(push.clip_from_mesh, float4(push.vertices[index].position, 1.0));
}

[shader("fragment")]
float4 fragment_main() : SV_Target {
    return push.params->color;
}
