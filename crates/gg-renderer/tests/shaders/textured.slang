// Meshes in a texture, drawn by its alpha over what's behind them, whose vertices are positions in
// the mesh's own space and texture coordinates; each draw gives the texture and its sampler.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Vertex {
    float3 position;
    float2 uv;
};

struct Params {
    uint texture;
    uint sampler;
};

struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
    Params* params;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    Vertex vertex = push.vertices[index];
    Varyings out;
    out.position = mul(push.clip_from_mesh, float4(vertex.position, 1.0));
    out.uv = vertex.uv;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    // Decoded from sRGB to linear as it's sampled, and the sRGB target encodes it again.
    Params params = *push.params;
    return textures[params.texture].Sample(samplers[params.sampler], input.uv);
}
