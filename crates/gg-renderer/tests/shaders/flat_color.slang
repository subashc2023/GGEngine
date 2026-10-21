// Meshes in one color, whose vertices are positions in clip space.

struct Vertex {
    float3 position;
};

struct Push {
    Vertex* vertices;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    return float4(push.vertices[index].position, 1.0);
}

[shader("fragment")]
float4 fragment_main() : SV_Target {
    // Linear, and the sRGB target encodes it.
    return float4(0.2, 0.3, 0.8, 1.0);
}
