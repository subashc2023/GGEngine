// Draws what a texture's mip level holds at a point, over all of the viewport, so that a test can
// read back what the GPU makes of a texture's format. The push constants give the texture and the
// sampler by their indices in the bindless set, the level, and the point.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Push {
    uint texture;
    uint sampler;
    float level;
    float u;
    float v;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    // One triangle that covers clip space.
    float2 corner = float2(float((index << 1) & 2), float(index & 2));
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 fragment_main() : SV_Target0 {
    Texture2D texture = textures[NonUniformResourceIndex(push.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(push.sampler)];
    return texture.SampleLevel(sampler, float2(push.u, push.v), push.level);
}
