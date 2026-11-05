// Writes a cube's levels in compute passes, and draws what it holds in a direction over all of the
// viewport, so that a test can read back what the GPU makes of storage images, cubes and compute
// passes. Each face's first level is a color of its own, by its index's bits in red, green and
// blue; each next level is the one before's halved.

[[vk::binding(0, 0)]] TextureCube cubes[];
[[vk::binding(1, 0)]] SamplerState samplers[];
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2DArray<float4> storage[];

struct Push {
    // Filling: the level's storage slot; halving: the level read's. Drawing: the cube's slot.
    uint image;
    // Halving: the level written's storage slot. Drawing: the sampler's index.
    uint other;
    // Filling and halving: the side of the level written.
    uint size;
    // Drawing: the level sampled.
    float level;
    // Drawing: the direction sampled.
    float4 direction;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

[shader("compute")]
[numthreads(8, 8, 1)]
void fill_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    uint face = id.z;
    storage[push.image][id] = float4(face & 1, (face >> 1) & 1, (face >> 2) & 1, 1.0);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void halve_main(uint3 id: SV_DispatchThreadID) {
    if (id.x >= push.size || id.y >= push.size) {
        return;
    }
    float4 above = storage[push.image][uint3(id.xy * 2, id.z)];
    storage[push.other][id] = float4(above.rgb * 0.5, 1.0);
}

[shader("vertex")]
float4 vertex_main(uint index: SV_VulkanVertexID) : SV_Position {
    // One triangle that covers clip space.
    float2 corner = float2(float((index << 1) & 2), float(index & 2));
    return float4(corner * 2.0 - 1.0, 0.0, 1.0);
}

[shader("fragment")]
float4 fragment_main() : SV_Target0 {
    return cubes[push.image].SampleLevel(samplers[push.other], push.direction.xyz, push.level);
}
