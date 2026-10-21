// Sprites: the engine's quad, whose corners are positions in its own space and where they are in
// the sprite's rectangle, showing that rectangle of a texture, tinted by the sprite's color, and
// drawn by its alpha over what's behind it. `Corner` in src/sprite.rs, which `Corner` here matches
// field for field, and `Params`, which `Sprite` here matches byte for byte, its `rect` being
// `top_left` then `bottom_right`.

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
    float4x4 clip_from_mesh;
    Corner* corners;
    Sprite* sprite;
};
[[vk::push_constant]] ConstantBuffer<Push> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
};

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID) {
    Corner corner = push.corners[index];
    Sprite sprite = *push.sprite;
    Varyings out;
    out.position = mul(push.clip_from_mesh, float4(corner.position, 1.0));
    out.uv = lerp(sprite.top_left, sprite.bottom_right, corner.uv);
    return out;
}

[shader("fragment")]
float4 fragment_main(Varyings input) : SV_Target {
    // Decoded from sRGB to linear as it's sampled, and the sRGB target encodes it again.
    Sprite sprite = *push.sprite;
    return textures[sprite.texture].Sample(samplers[sprite.sampler], input.uv) * sprite.color;
}
