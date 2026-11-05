// Draws a triangle's depth alone into a shadow map, and samples a shadow map by comparison, so that
// a test can read back what the GPU drew and how it compares. The push constants give the
// triangle's corners in clip space, for the first; and for the second, the shadow map and the
// sampler by their indices in the bindless set, the point, and the depth compared there.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerComparisonState samplers[];

struct Push {
    float4 corners[3];
    uint texture;
    uint sampler;
    float u;
    float v;
    float depth;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("vertex")]
float4 depth_main(uint index: SV_VulkanVertexID) : SV_Position {
    return push.corners[index];
}

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    // One triangle that covers clip space.
    float2 corner = float2(float((index << 1) & 2), float(index & 2));
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

// 1 where the depth compared is as near as the map's or nearer, 0 where it's farther.
[shader("fragment")]
float4 fragment_main() : SV_Target0 {
    Texture2D texture = textures[NonUniformResourceIndex(push.texture)];
    SamplerComparisonState sampler = samplers[NonUniformResourceIndex(push.sampler)];
    float lit = texture.SampleCmpLevelZero(sampler, float2(push.u, push.v), push.depth);
    return float4(lit, lit, lit, 1.0);
}
