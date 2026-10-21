// Meshes in a texture, drawn by its alpha over what's behind them: `Textured` in main.rs, which the
// vertex struct here matches field for field, and `Sampling`, which each draw gives. The texture
// is sRGB, so it samples as linear color, and the sRGB target encodes it again.

// The bindless set: every texture, by the handle a draw gives, and the engine's samplers.
[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Vertex {
    float3 position;
    // From (0, 0) at the texture's top left to (1, 1) at its bottom right.
    float2 uv;
};

struct Sampling {
    uint texture;
    uint sampler;
};

// What the engine pushes for each draw: the matrix from the mesh's space to clip space, then the
// addresses of the mesh's vertices and of the draw's `Sampling`.
struct Push {
    float4x4 clip_from_mesh;
    Vertex* vertices;
    Sampling* sampling;
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
    Sampling sampling = *push.sampling;
    return textures[sampling.texture].Sample(samplers[sampling.sampler], input.uv);
}
