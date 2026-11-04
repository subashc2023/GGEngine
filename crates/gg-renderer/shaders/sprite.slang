// Sprites: the engine's quad, whose corners are positions in its own space and where they are in
// the sprite's rectangle, showing that rectangle of a texture, tinted by the sprite's color, and
// drawn by its alpha over what's behind it. Each instance is a sprite, with its own transform, ID
// and parameters, writing its ID where it shows but where it's fully transparent, so that a pick
// there finds what's behind it. `Params` in src/sprite.rs, which `Sprite` here matches byte for
// byte, its `rect` being `top_left` then `bottom_right`. A sprite samples only the texels its
// rectangle covers, so that the cells of a sprite sheet packed edge to edge show nothing of one
// another: Godot's region filter clip, which Godot makes optional and always insets by half a
// texel.

#include "mesh.slang"

struct Sprite {
    // Linear RGBA, alpha not multiplied in.
    float4 color;
    // In texture coordinates.
    float2 top_left;
    float2 bottom_right;
    uint texture;
    uint sampler;
};

[[vk::push_constant]] ConstantBuffer<Push<Corner, Sprite>> push;

struct Varyings {
    float4 position : SV_Position;
    float2 uv;
    // Where the fragment stage may sample, in texture coordinates: the least corner, then the
    // greatest.
    nointerpolation float4 inside;
    nointerpolation uint instance;
    nointerpolation uint2 id;
};

// How far, in texels, a texture coordinate times its texture's size can round from the number it
// stands for: about a thousandth at 8,192 texels. A rectangle's edge that near a texel's is taken
// to be on it, and the bounds keep that far inside the texels' centers, which a sampler that
// truncates its position within a texel could otherwise cross.
static const float ROUNDING = 1.0 / 256.0;

[shader("vertex")]
Varyings vertex_main(uint index: SV_VulkanVertexID, uint instance: SV_VulkanInstanceID) {
    Corner corner = push.vertices[index];
    Sprite sprite = push.params[instance];
    float4 world = mul(push.transforms[instance], float4(corner.position, 1.0));
    Varyings out;
    out.position = mul(push.clip_from_world, world);
    out.uv = lerp(sprite.top_left, sprite.bottom_right, corner.uv);
    // The centers of the outermost texels the rectangle covers, wholly or in part, between which
    // neither a linear sampler, which reads the 2 by 2 texels around a point, nor a nearest one
    // reaches a texel past the rectangle's edges. A rectangle within one texel samples its center;
    // the max keeps the least bound from passing the greatest, which clamp needs, for one of no
    // width on a texel's edge, and the nudge inward stops halfway between them.
    uint width, height;
    textures[NonUniformResourceIndex(sprite.texture)].GetDimensions(width, height);
    float2 size = float2(width, height);
    float2 least = floor(min(sprite.top_left, sprite.bottom_right) * size + ROUNDING) + 0.5;
    float2 most = ceil(max(sprite.top_left, sprite.bottom_right) * size - ROUNDING) - 0.5;
    float2 nudge = clamp((most - least) / 2.0, 0.0, ROUNDING);
    out.inside = float4(least + nudge, max(least, most) - nudge) / float4(size, size);
    out.instance = instance;
    out.id = push.ids[instance];
    return out;
}

[shader("fragment")]
Targets fragment_main(Varyings input) {
    Sprite sprite = push.params[input.instance];
    // The sprites of one draw sample textures of their own, so the index can differ across it.
    // Decoded from sRGB to linear as it's sampled, as the view's colors are.
    Texture2D texture = textures[NonUniformResourceIndex(sprite.texture)];
    SamplerState sampler = samplers[NonUniformResourceIndex(sprite.sampler)];
    float2 uv = clamp(input.uv, input.inside.xy, input.inside.zw);
    float4 color = texture.Sample(sampler, uv) * sprite.color;
    // Blending what's fully transparent changes no pixel, but its ID would hide what's behind it
    // from picks. It writes no depth, so it keeps early depth testing.
    if (color.a == 0.0) {
        discard;
    }
    Targets out;
    out.color = color;
    out.id = input.id;
    return out;
}
