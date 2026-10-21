// A shader without the fragment stage every shader has, so it doesn't compile.

struct Push {
    float3* vertices;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    return float4(push.vertices[index], 1.0);
}
