// Sprites: the engine's quad, whose corners are positions in its own space and where they are in
// the sprite's rectangle, showing that rectangle of a texture, tinted by the sprite's color, and
// drawn by its alpha over what's behind it. Each instance is a sprite, with its own transform and
// parameters. `Corner` in src/sprite.rs, which `Corner` here matches field for field, and
// `Params`, which `Sprite` here matches byte for byte, its `rect` being `top_left` then
// `bottom_right`.

[[vk::binding(0, 0)]] Texture2D textures[];
[[vk::binding(1, 0)]] SamplerState samplers[];

struct Corner {
    float3 position;
    float2 uv;
};

struct Sprite {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In texture coordinates.
    float2 top_left;
    float2 bottom_right;
    uint texture;
    uint sampler;
};

struct Push {
    float4x4 clip_from_world;
    Corner* corners;
    // Each instance's, from the quad's space to the world's.
    float4x4* transforms;
    Sprite* sprites;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
    nointerpolation uint instance;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.corners[index];
    Sprite sprite = push.sprites[instance];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.uv = lerp(sprite.top_left, sprite.bottom_right, corner.uv);
    out.instance = instance;
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    Sprite sprite = push.sprites[input.instance];
    // The sprites of one draw sample textures of their own, so the index can differ across it.
    // Decoded from sRGB to linear as it's sampled, and the sRGB target encodes it again.
    Texture2D texture = textures[NonUniformResourceIndex(sprite.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(sprite.sampler)];
    return texture.Sample(sampler, input.uv) * sprite.color;
}
