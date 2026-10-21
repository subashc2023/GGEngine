// Meshes in the colors of their vertices, whose positions are in clip space. The colors are
// linear, and the sRGB target encodes them.

struct Vertex {
    float3 position;
    float4 color;
};

struct Push {
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
    out.position = float4(vertex.position, 1.0);
    out.color = vertex.color;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    return input.color;
}
