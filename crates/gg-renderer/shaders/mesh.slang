// Meshes in the colors of their vertices, whose positions are in clip space. The colors are
// linear, and the sRGB target encodes them.

struct Push {
    // The mesh's vertices, seven floats each: the position, then the color as RGBA.
    float* vertices;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float4 color;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    float* vertex = push.vertices + index * 7;
    Varyings out;
    out.position = float4(vertex[0], vertex[1], vertex[2], 1.0);
    out.color = float4(vertex[3], vertex[4], vertex[5], vertex[6]);
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    return input.color;
}
